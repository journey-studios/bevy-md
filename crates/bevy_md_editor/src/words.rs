//! Words: moving the caret over them and deleting them.
//!
//! The limits of a word come from Parley when the block has a layout (the
//! same call `EditableText` makes, so word breaks follow Unicode, including
//! punctuation and right-to-left text). A block without a layout (scrolled
//! out of the window, so it has no node) falls back to a plain scan by
//! character class, the way `motion_to` falls back to one `char` for Left and
//! Right: the text is still edited, never dropped.
//!
//! Both read the rendered text of the caret's block, so neither crosses into
//! another block: at the edge of a block a word motion stays, and a word
//! deletion joins blocks exactly as Backspace and Delete do (see `blocks`).

use std::time::Duration;

use bevy_md::Markdown;
use parley::{Affinity, Layout, Selection};

use crate::{MarkdownEditor, TextBrush, blocks, edit};

/// Which way along the text a word motion goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Direction {
    /// Toward the start of the text.
    Backward,
    /// Toward the end of the text.
    Forward,
}

/// What a character is for the purpose of finding the end of a word.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Other,
}

fn class(c: char) -> Class {
    if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Other
    }
}

/// Where the word motion `direction` takes `at`, a byte of the rendered
/// `text`: past the spaces next to it and then past the word (or the run of
/// punctuation) behind them.
pub(crate) fn scan(text: &str, at: usize, direction: Direction) -> usize {
    let mut pos = at.min(text.len());
    match direction {
        Direction::Forward => {
            let mut rest = text[pos..].chars().peekable();
            while let Some(c) = rest.next_if(|c| c.is_whitespace()) {
                pos += c.len_utf8();
            }
            if let Some(&first) = rest.peek() {
                let kind = class(first);
                while let Some(c) = rest.next_if(|&c| class(c) == kind) {
                    pos += c.len_utf8();
                }
            }
        }
        Direction::Backward => {
            let mut rest = text[..pos].chars().rev().peekable();
            while let Some(c) = rest.next_if(|c| c.is_whitespace()) {
                pos -= c.len_utf8();
            }
            if let Some(&last) = rest.peek() {
                let kind = class(last);
                while let Some(c) = rest.next_if(|&c| class(c) == kind) {
                    pos -= c.len_utf8();
                }
            }
        }
    }
    pos
}

/// Where the word motion `direction` takes the caret at `at` of `text`, as a
/// rendered-text offset: by the block's layout when it has one.
fn target(
    layout: Option<&Layout<TextBrush>>,
    text: &str,
    at: usize,
    direction: Direction,
) -> usize {
    let Some(layout) = layout else {
        return scan(text, at, direction);
    };
    let selection = Selection::from_byte_index(layout, at, Affinity::Downstream);
    let moved = match direction {
        Direction::Backward => selection.previous_visual_word(layout, false),
        Direction::Forward => selection.next_visual_word(layout, false),
    };
    moved.focus().index().min(text.len())
}

/// Move the caret over a word inside its block. Returns whether it moved.
pub(crate) fn move_word(
    editor: &mut MarkdownEditor,
    markdown: &Markdown,
    layout: Option<&Layout<TextBrush>>,
    direction: Direction,
) -> bool {
    let document = markdown.document();
    let Some((index, at)) = edit::caret_text(document, editor.caret) else {
        return false;
    };
    let Some(block) = document.block(index) else {
        return false;
    };
    let to = target(layout, &block.content.text, at, direction);
    let Some(caret) = edit::source_caret(&block, to).filter(|_| to != at) else {
        return false;
    };
    editor.set_caret(markdown, caret);
    true
}

/// Delete the word before or after the caret, as one step of the history.
/// At the edge of a block there is no word to take: the block is joined, as
/// Backspace and Delete do. Returns whether the source changed.
pub(crate) fn delete_word(
    editor: &mut MarkdownEditor,
    markdown: &mut Markdown,
    layout: Option<&Layout<TextBrush>>,
    direction: Direction,
    now: Duration,
) -> bool {
    let edge = match direction {
        Direction::Backward => blocks::backspace(editor, markdown, now),
        Direction::Forward => blocks::delete(editor, markdown, now),
    };
    if let Some(done) = edge {
        return done;
    }
    let document = markdown.document();
    let Some((index, at)) = edit::caret_text(document, editor.caret) else {
        return false;
    };
    let Some(block) = document.block(index) else {
        return false;
    };
    let to = target(layout, &block.content.text, at, direction);
    if to == at {
        return false;
    }
    let range = at.min(to)..at.max(to);
    // A word is one thing to undo: not the tail of the word typed before it,
    // and not part of the next deletion.
    editor.history.seal();
    let Some(mut change) = edit::delete_text(markdown, &mut editor.caret, index, range) else {
        return false;
    };
    // Deleting the first word of a block leaves blanks the parser drops in
    // front of the caret: it goes after them, into the block.
    let caret = blocks::into_block(markdown.document(), editor.caret);
    editor.set_caret(markdown, caret);
    change.caret_after = editor.caret;
    editor.history.record(change, now);
    editor.history.seal();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scan_forward_ends_at_the_end_of_the_next_word() {
        let text = "hello  big-world 日本語 x";
        let mut at = 0;
        let mut stops = vec![];
        loop {
            let next = scan(text, at, Direction::Forward);
            if next == at {
                break;
            }
            stops.push(&text[..next]);
            at = next;
        }
        assert_eq!(
            stops,
            [
                "hello",
                "hello  big",
                "hello  big-",
                "hello  big-world",
                "hello  big-world 日本語",
                "hello  big-world 日本語 x",
            ]
        );
    }

    #[test]
    fn the_scan_backward_ends_at_the_start_of_the_previous_word() {
        let text = "hello  big-world 日本語 x";
        let mut at = text.len();
        let mut stops = vec![];
        loop {
            let next = scan(text, at, Direction::Backward);
            if next == at {
                break;
            }
            stops.push(&text[next..]);
            at = next;
        }
        assert_eq!(
            stops,
            [
                "x",
                "日本語 x",
                "world 日本語 x",
                "-world 日本語 x",
                "big-world 日本語 x",
                "hello  big-world 日本語 x",
            ]
        );
    }

    #[test]
    fn the_scan_stays_at_the_edges_and_on_character_boundaries() {
        assert_eq!(scan("ab", 2, Direction::Forward), 2);
        assert_eq!(scan("ab", 0, Direction::Backward), 0);
        assert_eq!(scan("  ", 0, Direction::Forward), 2);
        assert_eq!(scan("  ", 2, Direction::Backward), 0);
        assert_eq!(scan("", 0, Direction::Forward), 0);
        assert_eq!(scan("é😀", 2, Direction::Forward), 6);
        assert_eq!(scan("é😀", 6, Direction::Backward), 2);
    }
}
