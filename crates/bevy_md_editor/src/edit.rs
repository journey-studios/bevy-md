//! Edits of the markdown source, as the caret sees them.
//!
//! Nothing here touches the engine: the caret is a byte offset into the
//! source (decision D3, markdown is the source of truth), rendered-text
//! offsets come from the source map of `bevy_md_core`, and every change is a
//! [`Markdown::edit`].

use std::ops::Range;

use bevy_md::Markdown;
use bevy_md_core::{Bias, BlockRef, Document, Style};

use crate::history::Change;

/// The block under the caret and the rendered-text byte the caret is at.
pub(crate) fn caret_text(document: &Document, caret: usize) -> Option<(usize, usize)> {
    let block = document.block_at(caret)?;
    Some((block, document.block(block)?.text_offset(caret)?))
}

/// The source byte for a caret at rendered-text byte `text` of `block`.
///
/// On a boundary between styled runs the caret goes to the end of the run
/// on the left, so typing continues its style.
pub(crate) fn source_caret(block: &BlockRef, text: usize) -> Option<usize> {
    block.source_offset(text, Bias::Upstream)
}

/// Insert `text` at the caret and move the caret after it. Returns the
/// change, for the history.
pub(crate) fn insert(markdown: &mut Markdown, caret: &mut usize, text: &str) -> Change {
    let change = Change {
        start: *caret,
        old: String::new(),
        new: text.to_owned(),
        caret_before: *caret,
        caret_after: *caret + text.len(),
    };
    markdown.edit(*caret..*caret, text);
    *caret += text.len();
    change
}

/// Delete the rendered text `text` of block `block`, and put the caret where
/// it was. Returns the change when the source changed.
///
/// Deleting a whole styled run takes its delimiters along: removing the `b`
/// of `**b**` leaves nothing, not a literal `****`.
pub(crate) fn delete_text(
    markdown: &mut Markdown,
    caret: &mut usize,
    block: usize,
    text: Range<usize>,
) -> Option<Change> {
    let document = markdown.document();
    let block = document.block(block)?;
    let mut range = block.source_range(text.clone())?;
    let whole_styled_run = block
        .content
        .runs
        .iter()
        .any(|run| run.range() == text && run.style != Style::PLAIN);
    if whole_styled_run {
        range = peel_delimiters(document.source(), range);
    }
    if range.is_empty() {
        return None;
    }
    let change = Change {
        start: range.start,
        old: document.source()[range.clone()].to_owned(),
        new: String::new(),
        caret_before: *caret,
        caret_after: range.start,
    };
    markdown.edit(range.clone(), "");
    *caret = range.start;
    Some(change)
}

/// Delete the selection `range`, a range of source bytes as the editor holds
/// it, and put the caret where the deleted bytes started. Returns the change
/// when the source changed; `caret` is the editor's caret before the edit,
/// which the change keeps for undo.
///
/// The selection may start and end in different blocks, or in the blank
/// lines between them: the bytes in between go and the two ends join. Like
/// [`delete_text`], it takes the delimiters of the styled runs it covers
/// whole along, so removing the `b` of `**b**` leaves nothing, not `****`.
/// A run it covers only in part keeps its markdown balanced: deleting the
/// `ld c` of `a **bold** c` gives `a **bo**`, with the closing `**` put back.
pub(crate) fn delete_range(
    markdown: &mut Markdown,
    caret: &mut usize,
    range: Range<usize>,
) -> Option<Change> {
    replace_range(markdown, caret, range, "")
}

/// Replace the selection `range` with `text` and put the caret after it:
/// what typing does over a selection. See [`delete_range`] for what the
/// selection covers; with a `text` to put in its place, the delimiters of
/// the styled runs it covers whole stay, so typing over the `b` of `**b**`
/// gives `**x**`, and the text takes the style of where the selection began.
/// Returns the change when the source changed.
pub(crate) fn replace_range(
    markdown: &mut Markdown,
    caret: &mut usize,
    range: Range<usize>,
    text: &str,
) -> Option<Change> {
    let cover = covered_source(markdown.document(), &range, text.is_empty());
    let range = cover.range;
    let new = format!("{text}{}", cover.keep);
    if range.is_empty() && new.is_empty() {
        *caret = range.start;
        return None;
    }
    let caret_after = range.start + text.len();
    let change = Change {
        start: range.start,
        old: markdown.document().source()[range.clone()].to_owned(),
        new: new.clone(),
        caret_before: *caret,
        caret_after,
    };
    markdown.edit(range, &new);
    *caret = caret_after;
    Some(change)
}

/// What an edit of a selection takes away and puts back.
struct Cover {
    /// The source bytes that go.
    range: Range<usize>,
    /// Delimiters that go with them but must stay for the markdown to stay
    /// balanced: the closing ones of the styles the selection ends in, then
    /// the opening ones of the styles it ends before the end of.
    keep: String,
}

/// The source bytes an edit of the selection `selection` takes away: the
/// selection itself, widened over the delimiters of the styled runs it
/// covers whole when `peel` is set. A selection of syntax alone (no
/// rendered text) is taken as it is.
fn covered_source(document: &Document, selection: &Range<usize>, peel: bool) -> Cover {
    let end_of_source = document.source().len();
    let selection = selection.start.min(end_of_source)..selection.end.min(end_of_source);
    let first = document.block_at(selection.start);
    let last = document.block_at(selection.end);
    let block = |ix: Option<usize>| ix.and_then(|ix| document.block(ix));
    if first.is_some() && first == last {
        return match block(first) {
            Some(block) => block_cover(document, &block, &selection, peel, Side::Both),
            None => Cover {
                range: selection,
                keep: String::new(),
            },
        };
    }
    // Several blocks, or the blank lines between them: the part in the
    // first block, everything in between, the part in the last.
    let head = block(first).map(|block| {
        block_cover(
            document,
            &block,
            &(selection.start..usize::MAX),
            peel,
            Side::Start,
        )
    });
    let tail = block(last)
        .map(|block| block_cover(document, &block, &(0..selection.end), peel, Side::End));
    let start = head
        .as_ref()
        .map_or(selection.start, |head| head.range.start);
    let end = tail.as_ref().map_or(selection.end, |tail| tail.range.end);
    let keep = [head, tail]
        .into_iter()
        .flatten()
        .map(|cover| cover.keep)
        .collect();
    Cover {
        range: start..end.max(start),
        keep,
    }
}

/// Which end of a selection a block holds.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    /// The selection starts and ends in the block.
    Both,
    /// It starts in the block and goes on past its end.
    Start,
    /// It comes from before the block and ends in it.
    End,
}

/// The part of `selection` that lies in `block`, as source bytes: from the
/// start of the selection (or of the block when the selection begins before
/// it, so that its markers go too) to its end (or the block's).
fn block_cover(
    document: &Document,
    block: &BlockRef,
    selection: &Range<usize>,
    peel: bool,
    side: Side,
) -> Cover {
    let span = block.span();
    let text = &block.content.text;
    let from_start = selection.start <= span.start;
    let to_end = selection.end >= span.end;
    let ts = if from_start {
        0
    } else {
        block.text_offset(selection.start).unwrap_or(0)
    };
    let te = if to_end {
        text.len()
    } else {
        block.text_offset(selection.end).unwrap_or(text.len())
    };
    // The ends of a selection that spans blocks always take the delimiters of
    // the styles they cover whole; typing over one block keeps them.
    let peel = peel || side != Side::Both;
    let raw = Cover {
        range: selection.clone(),
        keep: String::new(),
    };
    let Some(inner) = (ts < te).then(|| block.source_range(ts..te)).flatten() else {
        return raw;
    };
    let inner = if peel {
        widen_over_styles(document.source(), block, &(ts..te), inner)
    } else {
        inner
    };
    let start = if from_start {
        selection.start
    } else {
        inner.start
    };
    let end = if to_end { selection.end } else { inner.end };
    Cover {
        range: start..end.max(start),
        keep: kept_delimiters(block, ts, te, side, peel),
    }
}

/// The delimiters a style puts around its text, and whether the source
/// repeats the byte as many times as the span needs (code and math), which
/// are counted in the source. Code and math are innermost, so they come
/// first.
const DELIMITERS: [(Style, &str, bool); 6] = [
    (Style::CODE, "`", true),
    (Style::MATH, "$", true),
    (Style::BOLD, "**", false),
    (Style::ITALIC, "*", false),
    (Style::STRIKE, "~~", false),
    (Style::SUBSCRIPT, "~", false),
];

/// The delimiters to put back where the text `ts..te` of `block` is
/// deleted: the closing ones of the styles that begin before `ts` and end
/// inside the deleted text (all that end after `ts` when the selection goes
/// on past this block), then the opening ones of the styles that begin
/// inside it and go on past `te` (all that begin before `te` when the
/// selection comes from before this block). With `peel`, a style that begins
/// at `ts` goes whole, delimiters and all; without it, its opening stays.
fn kept_delimiters(block: &BlockRef, ts: usize, te: usize, side: Side, peel: bool) -> String {
    let mut closing = Vec::new();
    let mut opening = Vec::new();
    for (order, (style, delimiter, _)) in DELIMITERS.iter().enumerate() {
        for (from, to) in style_intervals(block, *style) {
            let ends_inside = to > ts && (side == Side::Start || to < te);
            // A style that begins where the selection does is not taken
            // whole when its text is kept, and then its opening stays.
            let opened_before = from < ts || (from == ts && !peel);
            if side != Side::End && opened_before && ends_inside {
                closing.push((to, order, *delimiter));
            }
            let starts_inside = from < te && (side == Side::End || from > ts);
            if side != Side::Start && starts_inside && to > te {
                opening.push((from, order, *delimiter));
            }
        }
    }
    // The innermost closes first and the outermost opens first.
    closing.sort_unstable_by_key(|(to, order, _)| (*to, *order));
    opening.sort_unstable_by_key(|(from, order, _)| (*from, std::cmp::Reverse(*order)));
    closing
        .into_iter()
        .chain(opening)
        .map(|(.., delimiter)| delimiter)
        .collect()
}

/// Widen `range`, the source bytes of the rendered text `text` of `block`,
/// over the delimiters of every style whose text starts or ends `text` and
/// lies wholly inside it.
fn widen_over_styles(
    source: &str,
    block: &BlockRef,
    text: &Range<usize>,
    range: Range<usize>,
) -> Range<usize> {
    let bytes = source.as_bytes();
    let (mut start, mut end) = (range.start, range.end);
    for (style, delimiter, counted) in DELIMITERS {
        let byte = delimiter.as_bytes()[0];
        for (from, to) in style_intervals(block, style) {
            let whole = from >= text.start && to <= text.end;
            if whole && from == text.start {
                let width = if counted {
                    count_backwards(bytes, start, byte)
                } else {
                    delimiter.len()
                };
                start = start.saturating_sub(width);
            }
            if whole && to == text.end {
                let width = if counted {
                    count_forwards(bytes, end, byte)
                } else {
                    delimiter.len()
                };
                end = (end + width).min(bytes.len());
            }
        }
    }
    start.max(block.span().start)..end
}

/// The maximal stretches of rendered text, as `(start, end)` byte offsets,
/// where every run has `style`.
fn style_intervals(block: &BlockRef, style: Style) -> Vec<(usize, usize)> {
    let mut intervals: Vec<(usize, usize)> = Vec::new();
    for run in block
        .content
        .runs
        .iter()
        .filter(|run| run.style.contains(style))
    {
        let range = run.range();
        match intervals.last_mut() {
            Some(last) if last.1 == range.start => last.1 = range.end,
            _ => intervals.push((range.start, range.end)),
        }
    }
    intervals
}

/// How many `byte`s end just before `at`.
fn count_backwards(bytes: &[u8], at: usize, byte: u8) -> usize {
    bytes[..at.min(bytes.len())]
        .iter()
        .rev()
        .take_while(|b| **b == byte)
        .count()
}

/// How many `byte`s start at `at`.
fn count_forwards(bytes: &[u8], at: usize, byte: u8) -> usize {
    bytes
        .get(at..)
        .map_or(0, |rest| rest.iter().take_while(|b| **b == byte).count())
}
/// Widen `range` over the matching emphasis, code or math delimiters on
/// both sides of it.
fn peel_delimiters(source: &str, mut range: Range<usize>) -> Range<usize> {
    let bytes = source.as_bytes();
    while range.start > 0 && range.end < bytes.len() {
        let (before, after) = (bytes[range.start - 1], bytes[range.end]);
        if before != after || !matches!(before, b'*' | b'_' | b'~' | b'`' | b'$') {
            break;
        }
        range = range.start - 1..range.end + 1;
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "Some **bold** and *italic* text with `code`";

    /// A caret at rendered-text byte `text` of the first block.
    fn caret_at(markdown: &Markdown, text: usize) -> usize {
        source_caret(&markdown.document().block(0).unwrap(), text).unwrap()
    }

    fn text(markdown: &Markdown) -> &str {
        &markdown.document().block(0).unwrap().block().content.text
    }

    #[test]
    fn typing_at_a_boundary_continues_the_style_on_the_left() {
        let mut markdown = Markdown::new(SOURCE);
        // The end of "bold", text byte 9.
        let mut caret = caret_at(&markdown, 9);
        insert(&mut markdown, &mut caret, "er");
        assert_eq!(
            markdown.document().source(),
            "Some **bolder** and *italic* text with `code`"
        );
        // The caret keeps its place in the text, two bytes further.
        assert_eq!(caret_text(markdown.document(), caret), Some((0, 11)));
    }

    #[test]
    fn typing_before_a_styled_run_stays_plain() {
        let mut markdown = Markdown::new(SOURCE);
        let mut caret = caret_at(&markdown, 5);
        insert(&mut markdown, &mut caret, "X");
        assert_eq!(
            markdown.document().source(),
            "Some X**bold** and *italic* text with `code`"
        );
        assert_eq!(caret_text(markdown.document(), caret), Some((0, 6)));
    }

    #[test]
    fn typing_inside_code_stays_inside_the_backticks() {
        let mut markdown = Markdown::new(SOURCE);
        let end = text(&markdown).len();
        let mut caret = caret_at(&markdown, end - 2);
        insert(&mut markdown, &mut caret, "_");
        assert_eq!(
            markdown.document().source(),
            "Some **bold** and *italic* text with `co_de`"
        );
    }

    #[test]
    fn typing_markdown_syntax_changes_the_structure() {
        let mut markdown = Markdown::new("plain");
        let mut caret = caret_at(&markdown, 0);
        insert(&mut markdown, &mut caret, "# ");
        assert_eq!(markdown.document().source(), "# plain");
        assert_eq!(
            markdown.document().block(0).unwrap().kind,
            bevy_md_core::BlockKind::Heading(1)
        );
        assert_eq!(caret_text(markdown.document(), caret), Some((0, 0)));
    }

    #[test]
    fn backspace_inside_a_styled_run_edits_only_its_text() {
        let mut markdown = Markdown::new(SOURCE);
        let mut caret = caret_at(&markdown, 8);
        assert!(delete_text(&mut markdown, &mut caret, 0, 7..8).is_some());
        assert_eq!(
            markdown.document().source(),
            "Some **bod** and *italic* text with `code`"
        );
        assert_eq!(text(&markdown), "Some bod and italic text with code");
        assert_eq!(caret_text(markdown.document(), caret), Some((0, 7)));
    }

    #[test]
    fn deleting_a_whole_styled_run_takes_its_delimiters() {
        let mut markdown = Markdown::new("a **b** c");
        let mut caret = 0;
        assert!(delete_text(&mut markdown, &mut caret, 0, 2..3).is_some());
        assert_eq!(markdown.document().source(), "a  c");
        assert_eq!(caret, 2);

        let mut markdown = Markdown::new("a ***b*** c");
        assert!(delete_text(&mut markdown, &mut caret, 0, 2..3).is_some());
        assert_eq!(markdown.document().source(), "a  c");

        let mut markdown = Markdown::new("a `x` c");
        assert!(delete_text(&mut markdown, &mut caret, 0, 2..3).is_some());
        assert_eq!(markdown.document().source(), "a  c");
    }

    #[test]
    fn deleting_the_last_character_of_a_block_removes_the_block() {
        let mut markdown = Markdown::new("a");
        let mut caret = 1;
        assert!(delete_text(&mut markdown, &mut caret, 0, 0..1).is_some());
        assert_eq!(markdown.document().blocks().len(), 0);
        assert_eq!(caret, 0);
    }

    #[test]
    fn deleting_across_an_escape_removes_the_backslash_too() {
        let mut markdown = Markdown::new(r"a \* b");
        let mut caret = 0;
        // Text " *" is source " \*": the backslash sits between the two
        // runs, so a range crossing them takes it along.
        assert!(delete_text(&mut markdown, &mut caret, 0, 1..3).is_some());
        assert_eq!(markdown.document().source(), r"a b");
    }

    #[test]
    fn nothing_to_delete_outside_the_text() {
        let mut markdown = Markdown::new("a");
        let mut caret = 0;
        assert!(delete_text(&mut markdown, &mut caret, 0, 5..6).is_none());
        assert!(delete_text(&mut markdown, &mut caret, 3, 0..1).is_none());
    }

    #[test]
    fn the_caret_finds_its_block() {
        let markdown = Markdown::new("one\n\ntwo **b**\n");
        let document = markdown.document();
        assert_eq!(document.block_at(0), Some(0));
        assert_eq!(document.block_at(3), Some(0));
        assert_eq!(document.block_at(5), Some(1));
        assert_eq!(caret_text(document, 5 + 9), Some((1, 5)));
    }
}
