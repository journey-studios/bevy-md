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
