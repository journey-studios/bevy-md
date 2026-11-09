//! Selection: making one, replacing it, and showing it.
//!
//! A selection is two byte offsets into the markdown source, the fixed
//! `anchor` and the moving `caret` of the editor, so it can cross blocks and
//! the bytes it names are plain source. What the user sees selected is the
//! rendered text those bytes stand for:
//!
//! - **Moving** with Shift keeps the anchor and moves the caret the way a
//!   plain move does (the layout of the caret's block says where the next
//!   character, or the end of the line, is). Without Shift, a selection
//!   collapses first: Left and Right go to its left and right edge.
//! - **Editing** replaces the selected bytes of the source ([`edit`]) and is
//!   one step of the history that undo takes back with the selection that
//!   was there.
//! - **Drawing** is Bevy's: the rectangles go to
//!   [`TextLayoutInfo::selection_rects`] of the block with the caret, which
//!   `bevy_ui_render` fills with the color of the block's
//!   [`TextCursorStyle`]. Only that block shows the selection: the other
//!   blocks a selection crosses are plain text without editor state (that is
//!   what keeps a large document as cheap with an editor as without), so a
//!   selection that spans blocks is highlighted in the block of its moving
//!   end only.
//!
//! [`TextLayoutInfo::selection_rects`]: bevy::text::TextLayoutInfo::selection_rects
//! [`TextCursorStyle`]: bevy::text::TextCursorStyle

use std::time::Duration;

use bevy::math::Rect;
use bevy::text::TextBrush;
use bevy_md::Markdown;
use bevy_md_core::Document;
use parley::{Affinity, Cursor, Layout, Selection};

use crate::{MarkdownEditor, Motion, edit, motion_to};

/// Select the whole document: the anchor at its start, the caret at its end.
/// Returns whether the selection changed.
pub(crate) fn select_all(editor: &mut MarkdownEditor, markdown: &Markdown) -> bool {
    let document = markdown.document();
    let Some(last) = document
        .len()
        .checked_sub(1)
        .and_then(|ix| document.block(ix))
    else {
        return false;
    };
    // The end of the source can be a blank line after the last block, where
    // no block holds the caret: then the end of the block is the end.
    let end = markdown.source().len();
    let end = if document.block_at(end).is_some() {
        end
    } else {
        last.span().end
    };
    let before = (editor.anchor, editor.caret);
    editor.set_selection(markdown, 0, end);
    (editor.anchor, editor.caret) != before
}

/// Drop the selection and keep the caret where it is. Returns whether there
/// was a selection.
pub(crate) fn collapse(editor: &mut MarkdownEditor) -> bool {
    let selected = editor.selection().is_some();
    editor.anchor = editor.caret;
    selected
}

/// Left or Right with a selection: the caret goes to the left or right edge
/// of it, and the selection is gone. Returns whether it did; the other
/// motions, and no selection, leave everything as it was.
pub(crate) fn collapse_to_edge(
    editor: &mut MarkdownEditor,
    markdown: &Markdown,
    motion: Motion,
) -> bool {
    let Some(selection) = editor.selection() else {
        return false;
    };
    let edge = match motion {
        Motion::Left => selection.start,
        Motion::Right => selection.end,
        Motion::Home | Motion::End => return false,
    };
    editor.set_caret(markdown, edge);
    true
}

/// Shift and a motion: move the caret, keep the anchor. Returns whether the
/// caret moved.
pub(crate) fn extend(
    editor: &mut MarkdownEditor,
    markdown: &Markdown,
    layout: Option<&Layout<TextBrush>>,
    motion: Motion,
) -> bool {
    let document = markdown.document();
    let Some((block, at)) = edit::caret_text(document, editor.caret) else {
        return false;
    };
    let to = motion_to(document, layout, block, at, motion);
    let caret = document
        .block(block)
        .and_then(|block| edit::source_caret(&block, to))
        .filter(|caret| to != at && *caret != editor.caret);
    let Some(caret) = caret else {
        return false;
    };
    editor.set_selection(markdown, editor.anchor, caret);
    true
}

/// Replace the selection with `text`, one step of the history, and put the
/// caret after it. `None` when nothing is selected (the caller inserts at the
/// caret); else whether the source changed.
pub(crate) fn replace(
    editor: &mut MarkdownEditor,
    markdown: &mut Markdown,
    text: &str,
    now: Duration,
) -> Option<bool> {
    let selection = editor.selection()?;
    let anchor_before = editor.anchor;
    let change = if text.is_empty() {
        edit::delete_range(markdown, &mut editor.caret, selection)
    } else {
        edit::replace_range(markdown, &mut editor.caret, selection, text)
    };
    editor.anchor = editor.caret;
    Some(change.is_some_and(|change| {
        editor.history.record_selection(change, anchor_before, now);
        true
    }))
}

/// The rectangles of the selected text of block `block`, in the coordinates
/// of its layout: what part of `selection` (source bytes) lies in the block,
/// as the layout lays it out. Empty when none of it does.
pub(crate) fn rects(
    document: &Document,
    selection: &std::ops::Range<usize>,
    block: usize,
    layout: &Layout<TextBrush>,
) -> Vec<Rect> {
    let Some(block) = document.block(block) else {
        return Vec::new();
    };
    let span = block.span();
    if selection.end <= span.start || selection.start >= span.end {
        return Vec::new();
    }
    let len = block.content.text.len();
    let start = if selection.start <= span.start {
        0
    } else {
        block.text_offset(selection.start).unwrap_or(0)
    };
    let end = if selection.end >= span.end {
        len
    } else {
        block.text_offset(selection.end).unwrap_or(len)
    };
    if start >= end {
        return Vec::new();
    }
    let selection = Selection::new(
        Cursor::from_byte_index(layout, start, Affinity::Downstream),
        Cursor::from_byte_index(layout, end, Affinity::Upstream),
    );
    let mut rects: Vec<Rect> = selection
        .geometry(layout)
        .into_iter()
        .map(|(bounds, _line)| {
            Rect::new(
                bounds.x0 as f32,
                bounds.y0 as f32,
                bounds.x1 as f32,
                bounds.y1 as f32,
            )
        })
        .collect();
    // The lines of a selection touch: no hairline between them.
    for ix in 0..rects.len().saturating_sub(1) {
        let below = rects[ix + 1].min.y;
        if rects[ix].max.y < below {
            rects[ix].max.y = below;
        }
    }
    rects
}
