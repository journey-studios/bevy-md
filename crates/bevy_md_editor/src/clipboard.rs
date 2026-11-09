//! Copy, cut and paste over the selection.
//!
//! The clipboard holds **markdown source**: copying `bold` out of
//! `a **bold** b` puts the selected bytes of the source on the clipboard,
//! delimiters included, exactly as they are in the document.
//!
//! When each thing is read:
//!
//! - **The clipboard, for a paste**, is read when the shortcut is pressed
//!   (what the user copied then), unless a copy or a cut is still waiting
//!   ahead of it in the queue: that one will replace the clipboard before this
//!   paste is applied, so the paste reads it when it is applied.
//! - **The selection, for a copy or a cut**, is read when the action is
//!   applied, like every other action that depends on the state of the
//!   document: a copy that waited behind a parse, or behind typing that
//!   replaced the selection, copies what is selected at its turn, not at the
//!   key press.
//!
//! A cut or a paste over the selection is one splice of the source, so one
//! step of the history (sealed on both sides, so it never merges with typing),
//! and undo gives back the text **and the selection** it had.

use std::ops::Range;
use std::time::Duration;

use bevy::clipboard::Clipboard;
use bevy::text::TextBrush;
use bevy_md::Markdown;
use parley::Layout;

use crate::{
    Action, EditMetrics, MarkdownEditor, clamp_to_boundary, edit, paste_action, selection,
};

impl Action {
    /// Whether the action is applied by [`apply`] rather than by the editor's
    /// own `apply`: it needs the clipboard.
    pub(crate) fn uses_clipboard(&self) -> bool {
        self.writes_clipboard() || matches!(self, Action::PasteClipboard)
    }

    /// Whether the action replaces what is on the clipboard.
    pub(crate) fn writes_clipboard(&self) -> bool {
        matches!(self, Action::Copy | Action::Cut)
    }
}

/// The action for a paste shortcut. `deferred` when a copy or a cut is
/// waiting ahead: the clipboard is read when the paste is applied.
pub(crate) fn paste_shortcut(
    clipboard: Option<&mut Clipboard>,
    metrics: &mut EditMetrics,
    deferred: bool,
) -> Option<Action> {
    if deferred {
        Some(Action::PasteClipboard)
    } else {
        paste_action(clipboard, metrics)
    }
}

/// The selected range of the source now, limited to the source and to
/// character boundaries (a replacement by someone else may have shortened
/// it), or `None` when nothing is selected.
fn selected(editor: &MarkdownEditor, markdown: &Markdown) -> Option<Range<usize>> {
    let source = markdown.source();
    let anchor = clamp_to_boundary(source, editor.anchor);
    let caret = clamp_to_boundary(source, editor.caret);
    (anchor != caret).then(|| anchor.min(caret)..anchor.max(caret))
}

/// Apply any action: the ones that need the clipboard here, the rest by the
/// editor's own `apply`. Returns whether it did anything.
pub(crate) fn dispatch(
    editor: &mut MarkdownEditor,
    markdown: &mut Markdown,
    layout: Option<&Layout<TextBrush>>,
    action: &Action,
    clipboard: Option<&mut Clipboard>,
    metrics: &mut EditMetrics,
    now: Duration,
) -> bool {
    if action.uses_clipboard() {
        return apply(editor, markdown, action, clipboard, metrics, now);
    }
    crate::apply(editor, markdown, layout, action, now)
}

/// Apply a copy, a cut or a paste that read the clipboard late. Returns
/// whether the source changed.
fn apply(
    editor: &mut MarkdownEditor,
    markdown: &mut Markdown,
    action: &Action,
    clipboard: Option<&mut Clipboard>,
    metrics: &mut EditMetrics,
    now: Duration,
) -> bool {
    match action {
        Action::Copy => {
            if let Some(range) = selected(editor, markdown) {
                let text = markdown.source()[range].to_owned();
                put(clipboard, metrics, text);
            }
            false
        }
        Action::Cut => {
            let Some(range) = selected(editor, markdown) else {
                return false;
            };
            let text = markdown.source()[range.clone()].to_owned();
            // Nothing is deleted that did not reach the clipboard.
            if !put(clipboard, metrics, text) {
                return false;
            }
            selection::replace(editor, markdown, "", now).unwrap_or(false)
        }
        Action::PasteClipboard => match paste_action(clipboard, metrics) {
            Some(Action::Paste(text)) => {
                paste(editor, markdown, &text, now);
                true
            }
            _ => false,
        },
        _ => false,
    }
}

/// Write `text` to the clipboard. Returns whether it got there; a clipboard
/// that is missing or refuses is counted.
fn put(clipboard: Option<&mut Clipboard>, metrics: &mut EditMetrics, text: String) -> bool {
    let written = clipboard.is_some_and(|clipboard| clipboard.set_text(text).is_ok());
    if !written {
        metrics.copy_failures += 1;
    }
    written
}

/// Insert `text` at the caret, or in place of the selection, as one step of
/// the history of its own: not the tail of the word typed before it and not
/// the head of the word typed after it.
pub(crate) fn paste(
    editor: &mut MarkdownEditor,
    markdown: &mut Markdown,
    text: &str,
    now: Duration,
) {
    if selection::replace(editor, markdown, text, now).is_none() {
        editor.history.seal();
        let change = edit::insert(markdown, &mut editor.caret, text);
        editor.history.record(change, now);
        editor.history.seal();
        editor.anchor = editor.caret;
    }
}
