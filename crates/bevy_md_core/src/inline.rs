//! Inline content of a block: one flat string plus styled runs over it.
//!
//! A renderer turns each [`Run`] into one styled text span, so a block costs
//! one string allocation however much formatting it carries.
//!
//! # Source map
//!
//! Markdown syntax (`**`, `` ` ``, `[`...) is not part of [`Inline::text`], so a
//! byte of rendered text is not a byte of the source. A [`Run`] therefore
//! remembers where its text starts in the source, which is all an editor
//! needs: the text of a mapped run is a **verbatim, contiguous copy** of
//! the source bytes it points at (a soft line break is the one exception:
//! it renders as a space but stays a one-byte `\n` in the source).
//!
//! Text that is not a verbatim copy has no mapping and gets a run of its
//! own, without a [`Run::source_start`]: entities (`&amp;`), hard breaks,
//! footnote references, CRLF soft breaks and the text of code blocks, HTML
//! and front matter. A backslash escape (`\*`) maps the one escaped byte,
//! so its run ends where the backslash begins and starts after it.

use std::ops::Range;

/// The sentinel of a run without a source mapping.
const UNMAPPED: u32 = u32::MAX;

/// Which side of a run boundary a rendered-text offset belongs to.
///
/// A caret between a plain run and a bold one is one text offset but two
/// places in the source: before the opening `**` or inside it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bias {
    /// The end of the run before the offset: typing there continues the
    /// style of the text on the left, like in most rich text editors.
    Upstream,
    /// The start of the run after the offset.
    Downstream,
}

/// Inline formatting flags. Several can be set at once.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Style(u8);

impl Style {
    /// No formatting.
    pub const PLAIN: Self = Self(0);
    /// `**strong**`
    pub const BOLD: Self = Self(1);
    /// `*emphasis*`
    pub const ITALIC: Self = Self(1 << 1);
    /// `~~strikethrough~~`
    pub const STRIKE: Self = Self(1 << 2);
    /// `` `code` ``
    pub const CODE: Self = Self(1 << 3);
    /// `^superscript^`, and footnote references.
    pub const SUPERSCRIPT: Self = Self(1 << 4);
    /// `~subscript~`
    pub const SUBSCRIPT: Self = Self(1 << 5);
    /// `$math$`
    pub const MATH: Self = Self(1 << 6);
    /// The alt text of an image; the run's link holds the image URL.
    pub const IMAGE: Self = Self(1 << 7);

    /// Whether every flag of `other` is set.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Both sets of flags.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// The raw bits, for hashing or packing.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

/// A stretch of [`Inline::text`] with one style and at most one link.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Run {
    /// Byte offset into [`Inline::text`].
    pub start: u32,
    /// Byte offset into [`Inline::text`], exclusive.
    pub end: u32,
    /// Formatting of the run.
    pub style: Style,
    /// Index into [`Inline::links`].
    pub link: Option<u32>,
    /// Source byte where the text starts, relative to the start of the
    /// block's source span; [`UNMAPPED`] when the text is not a verbatim
    /// copy of the source. Relative so that moving a block (an edit above
    /// it) leaves its runs equal.
    src: u32,
}

impl Run {
    /// The byte range of the run in [`Inline::text`].
    pub fn range(&self) -> Range<usize> {
        self.start as usize..self.end as usize
    }

    /// Where the run's text starts in the source, as a byte offset from the
    /// start of the block's source span, or `None` for text that is not a
    /// verbatim copy of the source.
    pub fn source_start(&self) -> Option<usize> {
        (self.src != UNMAPPED).then_some(self.src as usize)
    }

    /// The run's source bytes, from the start of the block's source span.
    pub fn source_range(&self) -> Option<Range<usize>> {
        let start = self.source_start()?;
        Some(start..start + (self.end - self.start) as usize)
    }
}

/// Text of a block with its formatting.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Inline {
    /// Rendered text, without markdown syntax.
    pub text: String,
    /// Styled runs covering `text` in order, without gaps.
    pub runs: Vec<Run>,
    /// Link and image destinations the runs point into.
    pub links: Vec<Box<str>>,
}

impl Inline {
    /// The text of one run.
    pub fn run_text(&self, run: &Run) -> &str {
        &self.text[run.range()]
    }

    /// The destination of a run's link, if it has one.
    pub fn run_link(&self, run: &Run) -> Option<&str> {
        run.link.map(|ix| &*self.links[ix as usize])
    }

    /// Whether the block has no text.
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Append `text`. `src` is where it sits in the document's source, if
    /// it is a verbatim copy; it is made relative to the block by
    /// [`Inline::rebase`] once the block's span is known.
    pub(crate) fn push(&mut self, text: &str, style: Style, link: Option<u32>, src: Option<usize>) {
        if text.is_empty() {
            return;
        }
        let src = src.map_or(UNMAPPED, |pos| pos as u32);
        let start = self.text.len() as u32;
        self.text.push_str(text);
        let end = self.text.len() as u32;
        if let Some(last) = self.runs.last_mut()
            && last.style == style
            && last.link == link
            && last.end == start
            && continues(last, src)
        {
            last.end = end;
            return;
        }
        self.runs.push(Run {
            start,
            end,
            style,
            link,
            src,
        });
    }

    /// Make the source positions of the runs relative to `base`, the start
    /// of the source span of the block they belong to.
    pub(crate) fn rebase(&mut self, base: usize) {
        for run in &mut self.runs {
            if run.src != UNMAPPED {
                debug_assert!(run.src as usize >= base, "run starts before its block");
                run.src -= base as u32;
            }
        }
    }

    /// The source byte, relative to the start of the block's source span,
    /// that the rendered-text byte `text` stands for. At a run boundary
    /// `bias` picks the side. `None` when no run next to `text` is mapped.
    pub fn source_offset(&self, text: usize, bias: Bias) -> Option<usize> {
        let first = self.runs.partition_point(|run| (run.end as usize) < text);
        let mut candidates = [self.runs.get(first), None];
        if text == candidates[0]?.end as usize {
            candidates[1] = self.runs.get(first + 1);
        }
        if bias == Bias::Downstream {
            candidates.swap(0, 1);
        }
        candidates.into_iter().flatten().find_map(|run| {
            let start = run.source_start()?;
            Some(start + text.saturating_sub(run.start as usize))
        })
    }

    /// The rendered-text byte for the source byte `source` (relative to the
    /// start of the block's source span).
    ///
    /// A source byte inside a run maps exactly. One in the syntax between
    /// runs (a `**`, a `](url)`) maps to the boundary of the text around it,
    /// which is the same text offset from either side.
    pub fn text_offset(&self, source: usize) -> usize {
        let mut offset = 0;
        for run in &self.runs {
            let Some(range) = run.source_range() else {
                continue;
            };
            if source < range.start {
                return run.start as usize;
            }
            if source <= range.end {
                return run.start as usize + (source - range.start);
            }
            offset = run.end as usize;
        }
        offset
    }

    /// The source bytes, relative to the start of the block's source span,
    /// that the rendered text `text` covers: its start belongs to the run
    /// after it and its end to the run before it. Inside a single run this
    /// is exactly the bytes of the text. A range crossing runs also covers
    /// the syntax between them, so deleting it deletes the delimiters too.
    pub fn source_range(&self, text: Range<usize>) -> Option<Range<usize>> {
        if text.is_empty() {
            let at = self.source_offset(text.start, Bias::Upstream)?;
            return Some(at..at);
        }
        let start = self.source_offset(text.start, Bias::Downstream)?;
        let end = self.source_offset(text.end, Bias::Upstream)?;
        (start <= end).then_some(start..end)
    }

    pub(crate) fn add_link(&mut self, dest: &str) -> u32 {
        self.links.push(dest.into());
        (self.links.len() - 1) as u32
    }
}

/// Whether text at source position `src` continues `last` in the source:
/// the two can then be one run, since the source holds them back to back.
fn continues(last: &Run, src: u32) -> bool {
    match (last.src, src) {
        (UNMAPPED, UNMAPPED) => true,
        (UNMAPPED, _) | (_, UNMAPPED) => false,
        (start, next) => start + (last.end - last.start) == next,
    }
}
