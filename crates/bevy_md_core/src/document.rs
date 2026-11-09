//! A markdown document that reparses only what an edit can reach.

use std::ops::{Add, Deref, Range};
use std::sync::Arc;

use pulldown_cmark::Options;

use crate::block::{Block, BlockKind, BlockRef};
use crate::definitions::{DefIndex, DefKind, Plan, WindowDefs};
use crate::parse::{Outside, Parsed, Placed, default_options, parse};
use crate::splices::{Splice, Splices};
use crate::sum_tree::{Dimension, Item, Iter, SumTree, Summary};

/// The most pieces [`Document::splice_from`] reports: past it the two
/// documents have too little in common for a list to help.
pub const MAX_DIFF_SPLICES: usize = 256;

/// A block as the sum tree holds it: with the distance from the start of
/// the block before it, instead of an absolute position.
///
/// Starts never decrease along the document, so the gaps are not negative
/// and the source offset of block `i` is the sum of the gaps up to it.
#[derive(Clone, Debug)]
struct Entry {
    /// Bytes from the start of the previous block (from 0 for the first).
    gap: isize,
    block: Block,
}

#[derive(Clone, Copy, Debug, Default)]
struct Gaps(isize);

impl Summary for Gaps {
    fn add(&mut self, other: &Self) {
        self.0 += other.0;
    }
}

impl Item for Entry {
    type Summary = Gaps;

    fn summary(&self) -> Gaps {
        Gaps(self.gap)
    }
}

/// The running sum of the gaps: a source offset.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
struct Start(isize);

impl Add for Start {
    type Output = Start;

    fn add(self, other: Start) -> Start {
        Start(self.0 + other.0)
    }
}

impl Dimension<Gaps> for Start {
    fn from_summary(summary: &Gaps) -> Self {
        Start(summary.0)
    }
}

/// The text of a document.
///
/// It sits behind an `Arc` so that a caller who has to keep reading the text
/// while a document is parsed from it (a renderer that shows the old document
/// until the new one is ready) can share it instead of copying it: see
/// [`Document::parse_shared`]. Writes go through [`Arc::make_mut`], so a text
/// nobody else holds is edited in place, and one that is shared is copied
/// once, on the first write. Cloning a [`Document`] still copies the text, as
/// it copies the blocks: only the `Arc` handed out explicitly is shared.
#[derive(Debug, Default)]
struct Source(Arc<String>);

impl Clone for Source {
    fn clone(&self) -> Self {
        Self(Arc::new(String::clone(&self.0)))
    }
}

impl Deref for Source {
    type Target = str;

    fn deref(&self) -> &str {
        &self.0
    }
}

impl Source {
    fn make_mut(&mut self) -> &mut String {
        Arc::make_mut(&mut self.0)
    }
}

/// Markdown source and the blocks it renders as.
///
/// Edits reparse the top-level constructs around the edit, widening until
/// the blocks at both ends of the reparsed window come out as before. The
/// result always equals a full parse of the new source; the tests check it.
///
/// The blocks live in a [`SumTree`] with relative positions, so an edit costs
/// `O(log n)` in the number of blocks, not `O(n)` (the source itself is one
/// `String`, which the edit shifts with a single `memmove`).
#[derive(Clone, Debug)]
pub struct Document {
    source: Source,
    blocks: SumTree<Entry>,
    options: Options,
    /// Link reference and footnote definitions, and who mentions them: what
    /// lets a window reparse resolve against the rest of the document.
    index: DefIndex,
    stats: Stats,
}

/// How much parsing a [`Document`] has done, for tests and benchmarks.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Bytes of source handed to the parser, in windows and in full parses.
    pub parsed_bytes: u64,
    /// Parses of a window around an edit (refreshes included).
    pub window_parses: u64,
    /// Parses of the whole source.
    pub full_parses: u64,
    /// Full parses forced by a label defined in two places, or by text that
    /// looks like a definition and cannot be keyed.
    pub definition_fallbacks: u64,
    /// Blocks reparsed because a definition they mention changed.
    pub refreshes: u64,
}

/// A definition in the source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Definition<'a> {
    /// `[label]: destination`
    Link {
        /// The label, with whitespace normalized (not case folded).
        label: &'a str,
        /// Where it points.
        destination: &'a str,
    },
    /// `[^label]: text`
    Footnote {
        /// The label, with whitespace normalized (not case folded).
        label: &'a str,
    },
}

impl Default for Document {
    fn default() -> Self {
        Self::with_options(default_options())
    }
}

impl Document {
    /// An empty document reading GFM with the default extensions.
    pub fn new() -> Self {
        Self::default()
    }

    /// An empty document reading the given pulldown-cmark options.
    pub fn with_options(options: Options) -> Self {
        Self {
            source: Source::default(),
            blocks: SumTree::new(),
            options,
            index: DefIndex::default(),
            stats: Stats::default(),
        }
    }

    /// Parse `source` with the default options.
    pub fn parse(source: impl Into<String>) -> Self {
        Self::parse_shared(Arc::new(source.into()))
    }

    /// Parse `source` with the default options, keeping the text where it
    /// is: the document holds the same allocation as `source`, so nothing is
    /// copied however large it is. The caller may keep its own handle to read
    /// the text meanwhile. A later edit of the document copies the text once,
    /// if the caller still holds its handle, and edits in place otherwise.
    pub fn parse_shared(source: Arc<String>) -> Self {
        let mut document = Self {
            source: Source(source),
            ..Self::default()
        };
        document.reparse_all();
        document
    }

    /// The markdown source.
    pub fn source(&self) -> &str {
        &self.source
    }

    /// How many blocks the document has.
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// Whether the document has no blocks.
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// Block `index` with its position in the source, in `O(log n)`.
    pub fn block(&self, index: usize) -> Option<BlockRef<'_>> {
        let (entry, Start(start)) = self.blocks.get_with_prefix::<Start>(index)?;
        Some(BlockRef {
            block: &entry.block,
            start: (start + entry.gap).cast_unsigned(),
        })
    }

    /// The blocks, in document order.
    pub fn blocks(&self) -> Blocks<'_> {
        self.blocks_from(0)
    }

    /// The blocks from `index` on, in document order. Finding the first one
    /// is `O(log n)`; each next one is `O(1)`.
    pub fn blocks_from(&self, index: usize) -> Blocks<'_> {
        Blocks {
            inner: self.blocks.iter_from(index),
            start: self.blocks.prefix::<Start>(index).0,
        }
    }

    /// The index of the block holding the source byte `offset`, in
    /// `O(log n)`. A byte on the boundary between two blocks belongs to the
    /// first. `None` when `offset` is in no block, such as the blank line
    /// between two.
    pub fn block_at(&self, offset: usize) -> Option<usize> {
        let starting = self.blocks.seek(Start(offset.cast_signed()));
        let last = starting.checked_sub(1)?;
        let block = self.block(last)?;
        // The block before may end exactly at `offset`, and then it is the
        // one that holds it.
        if last > 0
            && let Some(before) = self.block(last - 1)
            && before.span().end >= offset
        {
            return Some(last - 1);
        }
        (block.span().end >= offset).then_some(last)
    }

    /// The destination of the link reference definition `label` (compared
    /// without regard to case or runs of whitespace, as links are), when the
    /// document defines it: the first definition of a label wins.
    pub fn link_definition(&self, label: &str) -> Option<&str> {
        self.index.link(&crate::definitions::normalize_label(label))
    }

    /// Whether the document has a footnote definition for `label`.
    pub fn has_footnote_definition(&self, label: &str) -> bool {
        self.index
            .has_note(&crate::definitions::normalize_label(label))
    }

    /// The link and footnote definitions the parser reads, in source order
    /// with their offsets. A second definition of a label the parser ignores
    /// (the first wins) is not listed; footnote definitions are, each.
    pub fn definitions(&self) -> impl Iterator<Item = (usize, Definition<'_>)> {
        self.index.definitions().map(|(offset, def)| {
            let definition = match &def.kind {
                DefKind::Link(destination) => Definition::Link {
                    label: &def.label,
                    destination,
                },
                DefKind::Note => Definition::Footnote { label: &def.label },
            };
            (offset, definition)
        })
    }

    /// How much parsing the document has done. For tests and benchmarks.
    #[doc(hidden)]
    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// The index's records of where text looks like a definition and what
    /// it defines, to compare with another document's. For tests.
    ///
    /// The mentions of labels are in [`Document::reference_uses_snapshot`]:
    /// they depend on how the text was parsed, not only on what it says.
    #[doc(hidden)]
    pub fn definition_index_snapshot(&self) -> String {
        format!(
            "defs {:?}\ncands {:?}\nlabels {:?}",
            self.index.definitions().collect::<Vec<_>>(),
            self.index.candidates().collect::<Vec<_>>(),
            self.index.label_counts(),
        )
    }

    /// Which blocks (by start) mention which labels, as `(start, label)`.
    /// A parse asks about a label it fails to resolve even where the text
    /// turns out not to be a link (`[[a [b][c] d]]` is a wiki link and asks
    /// about `c` only when `c` is not defined in the same text), so a
    /// document edited in pieces may know of more mentions than one parsed
    /// whole, never fewer. For tests.
    #[doc(hidden)]
    pub fn reference_uses_snapshot(&self) -> Vec<(usize, u64)> {
        self.index
            .uses()
            .flat_map(|(pos, labels)| labels.iter().map(move |&label| (pos, label)))
            .collect()
    }

    /// The blocks that differ between `old` and `self`, as the [`Splices`]
    /// that take `old`'s block list to this one. A block is left out when it
    /// is equal to one of `old` at the same place (same start in the source)
    /// or at the same distance from the end of the source, which is what an
    /// edit leaves behind it: the blocks before it, and the ones after it
    /// moved. Blocks *inside* those two stretches that differ anyway (the
    /// paragraph that uses a definition the edit changed) are splices of
    /// their own; the stretch between the two, where the text changed, is one.
    /// Meant for a document parsed from scratch that a renderer shows the
    /// previous version of, so it rebuilds only what changed.
    ///
    /// Costs `O(n)` in the blocks of the larger document, with no allocation
    /// per block: meant for a background thread. More than
    /// [`MAX_DIFF_SPLICES`] pieces (two texts that have little in common) are
    /// reported as one covering splice.
    pub fn splice_from(&self, old: &Document) -> Splices {
        let (old_len, new_len) = (old.len(), self.len());
        let mut out = Splices::new();

        // Blocks at the same place: the ones before the first change in the
        // text, compared for content one by one.
        let mut head = 0;
        let mut differing: Option<usize> = None;
        for (ix, (o, n)) in old.blocks().zip(self.blocks()).enumerate() {
            if o.start != n.start {
                break;
            }
            head = ix + 1;
            if o.block().same_content(n.block()) {
                if let Some(from) = differing.take() {
                    out.push(from..ix, from..ix);
                }
            } else if differing.is_none() {
                differing = Some(ix);
            }
        }
        if let Some(from) = differing {
            out.push(from..head, from..head);
        }

        // Blocks at the same distance from the end, found by walking the
        // pairs `(i, i + shift)` that remain and forgetting the ones before
        // the last pair that is out of place.
        let shift = new_len.cast_signed() - old_len.cast_signed();
        let bytes = self.source.len().cast_signed() - old.source.len().cast_signed();
        let first = head + shift.min(0).unsigned_abs();
        let mut tail_from = first;
        let mut tail: Vec<Range<usize>> = Vec::new();
        let mut differing: Option<usize> = None;
        if first < old_len {
            let pairs = old
                .blocks_from(first)
                .zip(self.blocks_from(first.wrapping_add_signed(shift)));
            for (k, (o, n)) in pairs.enumerate() {
                let ix = first + k;
                if n.start.cast_signed() - o.start.cast_signed() != bytes {
                    tail.clear();
                    differing = None;
                    tail_from = ix + 1;
                } else if o.block().same_content(n.block()) {
                    if let Some(from) = differing.take() {
                        tail.push(from..ix);
                    }
                } else if differing.is_none() {
                    differing = Some(ix);
                }
            }
            if let Some(from) = differing {
                tail.push(from..old_len);
            }
        }

        // Between the two: where the text changed.
        out.push(head..tail_from, head..tail_from.wrapping_add_signed(shift));
        for run in tail {
            out.push(
                run.clone(),
                run.start.wrapping_add_signed(shift)..run.end.wrapping_add_signed(shift),
            );
        }
        if out.len() > MAX_DIFF_SPLICES {
            return Splices::one(out.covering());
        }
        out
    }

    /// Panic unless the internal invariants hold: block starts never go back,
    /// and the sum tree agrees with the blocks. For tests.
    #[doc(hidden)]
    pub fn assert_consistent(&self) {
        let mut previous = 0;
        for (ix, block) in self.blocks().enumerate() {
            assert!(
                block.start >= previous,
                "block {ix} starts before block {}",
                ix.wrapping_sub(1)
            );
            assert_eq!(block.start, self.start_of(ix), "block {ix}");
            previous = block.start;
        }
        let end = self
            .blocks
            .prefix::<Start>(self.blocks.len())
            .0
            .cast_unsigned();
        assert_eq!(end, previous);
    }

    /// Source offset where block `index` starts.
    fn start_of(&self, index: usize) -> usize {
        self.blocks.prefix::<Start>(index + 1).0.cast_unsigned()
    }

    /// Append streamed text, such as the next chunk of a model's answer.
    pub fn push_str(&mut self, text: &str) -> Splices {
        let end = self.source.len();
        self.edit(end..end, text)
    }

    /// Replace the whole source. Text that extends the current source is
    /// treated as an append.
    pub fn set_source(&mut self, text: &str) -> Splices {
        if let Some(delta) = text.strip_prefix(&*self.source) {
            return self.push_str(delta);
        }
        let source = self.source.make_mut();
        source.clear();
        source.push_str(text);
        self.reparse_all()
    }

    /// Replace the bytes `range` of the source with `text`.
    ///
    /// # Panics
    ///
    /// When `range` is out of bounds or not on character boundaries.
    pub fn edit(&mut self, range: Range<usize>, text: &str) -> Splices {
        assert!(
            range.start <= range.end
                && self.source.is_char_boundary(range.start)
                && self.source.is_char_boundary(range.end),
            "edit range {range:?} is not a valid range of the source"
        );
        if range.is_empty() && text.is_empty() {
            return Splices::new();
        }
        self.edit_in(range, text, false).0
    }

    /// An edit, or (`refresh`) a reparse of the roots around `range.start`
    /// with the text as it is, for blocks whose definitions changed.
    /// Returns the splice and the stretch of the new source that was
    /// reparsed.
    fn edit_in(
        &mut self,
        range: Range<usize>,
        text: &str,
        refresh: bool,
    ) -> (Splices, Range<usize>) {
        if !refresh {
            // Front matter only exists at the very start, and whether the
            // text there is front matter can hinge on any later line.
            // A document opening with an unclosed `---` gains front matter
            // as soon as any later line closes it.
            let first = self.block(0);
            let has_frontmatter = first
                .as_ref()
                .is_some_and(|b| b.kind == BlockKind::Frontmatter);
            let touches_frontmatter = range.start < 4
                || (has_frontmatter && first.is_some_and(|b| range.start <= b.span().end))
                || (!has_frontmatter && self.source.starts_with("---"));
            if self.blocks.is_empty() || touches_frontmatter {
                self.source.make_mut().replace_range(range, text);
                return self.everything();
            }
        }

        let delta = text.len().cast_signed() - range.len().cast_signed();
        let count = self.blocks.len();
        let (lo, hi) = self.window(&range);

        if !refresh {
            self.source.make_mut().replace_range(range.clone(), text);
        }
        let shift = |pos: usize| {
            if pos >= range.end {
                pos.wrapping_add_signed(delta)
            } else {
                pos
            }
        };

        let (mut lo, mut hi) = (lo, hi);
        loop {
            let start = if lo == 0 {
                0
            } else {
                line_start(&self.source, self.root_of(lo).start)
            };
            let end = if hi == count {
                self.source.len()
            } else {
                line_end(&self.source, shift(self.root_of(hi - 1).end))
            };
            // The same stretch of the source before the edit.
            let old_range = start..end.wrapping_add_signed(-delta);
            let previous = self.index.window(&old_range);
            let parsed = {
                let index = &self.index;
                let link = |label: &str| index.resolve_outside(label, &previous);
                let note = |label: &str| index.note_outside(label, &previous);
                let outside = Outside {
                    link: &link,
                    note: &note,
                };
                parse(&self.source[start..end], start, self.options, &outside)
            };
            self.stats.window_parses += 1;
            self.stats.parsed_bytes += (end - start) as u64;

            let start_synced =
                lo == 0 || self.same_group(group_at_start(&parsed.blocks), self.root_group(lo), 0);
            let end_synced = hi == count
                || self.same_group(group_at_end(&parsed.blocks), self.root_group(hi - 1), delta);
            if start_synced && end_synced {
                let window = Reparse {
                    old_range,
                    new_range: start..end,
                    previous,
                    delta,
                };
                return self.finish(lo..hi, parsed, &window, refresh);
            }
            if !start_synced {
                lo = self.root_group(lo - 1).start;
            }
            if !end_synced {
                hi = self.root_group(hi).end;
            }
        }
    }

    /// Take in a window that reparsed in sync: its definitions, then its
    /// blocks, then a refresh of the blocks elsewhere that mention a
    /// definition that changed.
    fn finish(
        &mut self,
        group: Range<usize>,
        parsed: Parsed,
        window: &Reparse,
        refresh: bool,
    ) -> (Splices, Range<usize>) {
        let changed = match self.index.plan(&window.previous, &parsed.found) {
            Plan::Full => {
                self.stats.definition_fallbacks += 1;
                return self.everything();
            }
            Plan::Apply { changed } => changed,
        };
        let mut merged = Splices::one(self.splice(group, parsed.blocks, window.delta));
        self.index.replace(
            &window.old_range,
            window.delta,
            &window.previous,
            parsed.found,
        );
        if changed.is_empty() {
            return (merged, window.new_range.clone());
        }
        if refresh {
            // A refresh changes no text, so no definition can change.
            self.stats.definition_fallbacks += 1;
            return self.everything();
        }
        // The blocks that mention a changed definition, wherever they are:
        // each refresh is a splice of its own, and they compose into the
        // list this edit reports, so the blocks between stay out of it.
        let mut done = 0;
        for at in self.index.users(&changed) {
            if window.new_range.contains(&at) || at < done {
                continue;
            }
            self.stats.refreshes += 1;
            let (splices, reparsed) = self.edit_in(at..at, "", true);
            merged.compose(&splices);
            if reparsed.start == 0 && reparsed.end >= self.source.len() {
                break;
            }
            done = reparsed.end;
        }
        (merged, window.new_range.clone())
    }

    /// Reparse the whole source.
    fn everything(&mut self) -> (Splices, Range<usize>) {
        (self.reparse_all(), 0..self.source.len())
    }

    /// The blocks, as `lo..hi`, an edit of `range` can reach: the roots it
    /// touches plus one unchanged root on each side as a sentinel. When the
    /// sentinels reparse identically, nothing beyond them can have changed.
    fn window(&self, range: &Range<usize>) -> (usize, usize) {
        let count = self.blocks.len();
        // First block whose root reaches the start of the edit: seek to the
        // last block starting there, then walk back over its root.
        let mut after = self
            .blocks
            .seek(Start(range.start.cast_signed()))
            .saturating_sub(1);
        while after > 0
            && self
                .block(after - 1)
                .is_some_and(|b| b.root().end >= range.start)
        {
            after -= 1;
        }
        if self.root_of(after).end < range.start {
            after += 1;
        }
        let after = after.min(count - 1);
        // Last block whose root starts at or before the end of the edit. A
        // root can start before its first block (the `>` of a quote), so the
        // block after the last one starting there may belong to it.
        let mut before = self
            .blocks
            .seek(Start(range.end.cast_signed()))
            .saturating_sub(1);
        if before + 1 < count && self.root_of(before + 1).start <= range.end {
            before += 1;
        }
        let mut lo = self.root_group(before.min(after)).start;
        let mut hi = self.root_group(before.max(after)).end;
        if lo > 0 {
            lo = self.root_group(lo - 1).start;
        }
        if hi < count {
            hi = self.root_group(hi).end;
        }
        (lo, hi)
    }

    fn root_of(&self, index: usize) -> Range<usize> {
        self.block(index).map_or(0..0, |b| b.root())
    }

    /// Whether `new`, freshly parsed, is exactly the blocks `old`, once
    /// moved by `delta` bytes.
    fn same_group(&self, new: &[Placed], old: Range<usize>, delta: isize) -> bool {
        new.len() == old.len()
            && new.iter().zip(self.blocks_from(old.start)).all(|(n, o)| {
                n.start == o.start.wrapping_add_signed(delta) && n.block == *o.block()
            })
    }

    /// Put `new` in place of the blocks `old`, leaving out the blocks at
    /// either end that came out unchanged, so a renderer rebuilds only what
    /// differs.
    ///
    /// Two blocks are the same when everything but the extent of their root
    /// matches: the root of a list item grows with every edit anywhere in the
    /// list, which says nothing about the item. The blocks left out have
    /// their root extent brought up to date in place.
    fn splice(&mut self, old: Range<usize>, mut new: Vec<Placed>, delta: isize) -> Splice {
        let (lo, hi) = (old.start, old.end);
        let (prefix, suffix, fixes) = {
            let olds = hi - lo;
            let prefix = new
                .iter()
                .zip(self.blocks_from(lo))
                .take(olds)
                .take_while(|(n, o)| n.start == o.start && n.block.same_content(o.block()))
                .count();
            let room = new.len().min(olds) - prefix;
            let suffix = (0..room)
                .take_while(|&j| {
                    let n = &new[new.len() - 1 - j];
                    self.block(hi - 1 - j).is_some_and(|o| {
                        n.start == o.start.wrapping_add_signed(delta)
                            && n.block.same_content(o.block())
                    })
                })
                .count();
            // Retained blocks whose root extent changed, by old index.
            let mut fixes = Vec::new();
            let kept = (0..prefix).chain(olds - suffix..olds);
            for k in kept {
                let new_ix = if k < prefix {
                    k
                } else {
                    new.len() - (olds - k)
                };
                let Some(o) = self.block(lo + k) else {
                    continue;
                };
                let n = &new[new_ix].block;
                if (n.root_start, n.root_end) != (o.root_start, o.root_end) {
                    fixes.push((lo + k, n.root_start, n.root_end));
                }
            }
            (prefix, suffix, fixes)
        };
        let (a, b) = (lo + prefix, hi - suffix);
        let count = self.blocks.len();
        let next_old_start = (b < count).then(|| self.start_of(b));
        let mut previous = if a == 0 { 0 } else { self.start_of(a - 1) }.cast_signed();

        let inserted = new.len() - prefix - suffix;
        let entries: Vec<Entry> = new
            .drain(prefix..prefix + inserted)
            .map(|placed| {
                let start = placed.start.cast_signed();
                let gap = start - previous;
                debug_assert!(gap >= 0, "blocks start in order");
                previous = start;
                Entry {
                    gap,
                    block: placed.block,
                }
            })
            .collect();
        self.blocks.splice(a..b, entries);
        if let Some(next_start) = next_old_start {
            let gap = next_start.wrapping_add_signed(delta).cast_signed() - previous;
            debug_assert!(gap >= 0, "blocks start in order");
            self.blocks.update(a + inserted, |entry| entry.gap = gap);
        }
        for (ix, root_start, root_end) in fixes {
            // Blocks after the replaced range moved to their new index.
            let ix = if ix < a { ix } else { ix + inserted - (b - a) };
            self.blocks.update(ix, |entry| {
                entry.block.root_start = root_start;
                entry.block.root_end = root_end;
            });
        }
        Splice {
            old: a..b,
            new: a..a + inserted,
        }
    }

    fn reparse_all(&mut self) -> Splices {
        let old = self.blocks.len();
        let parsed = parse(&self.source, 0, self.options, &Outside::NOWHERE);
        self.stats.full_parses += 1;
        self.stats.parsed_bytes += self.source.len() as u64;
        self.blocks = tree_of(parsed.blocks);
        self.index.rebuild(parsed.found);
        Splices::everything(old, self.blocks.len())
    }

    /// Indices of the blocks sharing the root of block `ix`.
    fn root_group(&self, ix: usize) -> Range<usize> {
        let start = self.start_of(ix);
        let Some(entry) = self.blocks.get(ix) else {
            return ix..ix;
        };
        let here = BlockRef {
            block: &entry.block,
            start,
        };
        let root = here.root();
        let (mut lo, mut lo_start, mut gap) = (ix, start, entry.gap);
        while lo > 0 {
            let before = lo_start.wrapping_add_signed(-gap);
            let Some(prev) = self.blocks.get(lo - 1) else {
                break;
            };
            let prev_ref = BlockRef {
                block: &prev.block,
                start: before,
            };
            if prev_ref.root() != root {
                break;
            }
            (lo, lo_start, gap) = (lo - 1, before, prev.gap);
        }
        let hi = ix
            + 1
            + self
                .blocks_from(ix + 1)
                .take_while(|b| b.root() == root)
                .count();
        lo..hi
    }
}

/// A window that reparsed in sync and is about to be taken in.
struct Reparse {
    /// The window in the source before the edit.
    old_range: Range<usize>,
    /// The window in the source now.
    new_range: Range<usize>,
    /// The definitions the index had in `old_range`.
    previous: WindowDefs,
    /// How much the text in the window grew.
    delta: isize,
}

/// The blocks of a [`Document`] with their positions, in order.
pub struct Blocks<'a> {
    inner: Iter<'a, Entry>,
    /// Start of the block before the next one.
    start: isize,
}

impl<'a> Iterator for Blocks<'a> {
    type Item = BlockRef<'a>;

    fn next(&mut self) -> Option<BlockRef<'a>> {
        let entry = self.inner.next()?;
        self.start += entry.gap;
        Some(BlockRef {
            block: &entry.block,
            start: self.start.cast_unsigned(),
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for Blocks<'_> {}

/// A tree of the placed blocks, with each one's gap from the one before.
fn tree_of(placed: Vec<Placed>) -> SumTree<Entry> {
    let mut previous = 0isize;
    let entries = placed
        .into_iter()
        .map(|p| {
            let start = p.start.cast_signed();
            let gap = start - previous;
            debug_assert!(gap >= 0, "blocks start in order");
            previous = start;
            Entry {
                gap,
                block: p.block,
            }
        })
        .collect();
    SumTree::from_vec(entries)
}

fn placed_root(p: &Placed) -> Range<usize> {
    p.start.wrapping_add_signed(p.block.root_start)..p.start.wrapping_add_signed(p.block.root_end)
}

fn group_at_start(blocks: &[Placed]) -> &[Placed] {
    let Some(first) = blocks.first() else {
        return blocks;
    };
    let root = placed_root(first);
    let len = blocks.iter().take_while(|b| placed_root(b) == root).count();
    &blocks[..len]
}

fn group_at_end(blocks: &[Placed]) -> &[Placed] {
    let Some(last) = blocks.last() else {
        return blocks;
    };
    let root = placed_root(last);
    let len = blocks
        .iter()
        .rev()
        .take_while(|b| placed_root(b) == root)
        .count();
    &blocks[blocks.len() - len..]
}

/// Start of the line holding `pos`, so indentation is reparsed too.
fn line_start(source: &str, pos: usize) -> usize {
    source[..pos].rfind('\n').map_or(0, |ix| ix + 1)
}

/// End of the line holding the byte before `pos`, newline included.
fn line_end(source: &str, pos: usize) -> usize {
    let pos = pos.min(source.len());
    if pos == 0 || source.as_bytes()[pos - 1] == b'\n' {
        return pos;
    }
    source[pos..]
        .find('\n')
        .map_or(source.len(), |ix| pos + ix + 1)
}
