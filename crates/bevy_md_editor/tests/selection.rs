//! Text selection: extending it with the keyboard, collapsing it, replacing
//! it by typing, Backspace or Delete, undoing that, and the rectangles drawn
//! for it. Offsets are bytes of the markdown source; every edit is checked
//! against a full parse of the source it leaves.

#[path = "../../bevy_md/tests/common/mod.rs"]
mod common;

use std::ops::Range;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::text::TextLayoutInfo;
use bevy::window::{Ime, PrimaryWindow};
use bevy_md::{BlockText, Markdown};
use bevy_md_core::{BlockKind, Document};
use bevy_md_editor::{MarkdownEditor, MarkdownEditorPlugin};
use common::*;

struct Open {
    app: App,
    markdown: Entity,
}

impl Open {
    fn new(source: &str) -> Self {
        Self::build(source, false)
    }

    fn with_window(source: &str) -> Self {
        Self::build(source, true)
    }

    fn build(source: &str, window: bool) -> Self {
        let mut app = headless_app();
        app.update();
        app.add_message::<Ime>();
        app.add_plugins(MarkdownEditorPlugin);
        if window {
            app.world_mut().spawn((Window::default(), PrimaryWindow));
        }
        let (_, markdown) = spawn_document(&mut app, source);
        app.world_mut()
            .entity_mut(markdown)
            .insert(MarkdownEditor::new());
        settle(&mut app);
        Self { app, markdown }
    }

    fn markdown(&self) -> &Markdown {
        self.app.world().get::<Markdown>(self.markdown).unwrap()
    }

    fn editor(&self) -> &MarkdownEditor {
        self.app
            .world()
            .get::<MarkdownEditor>(self.markdown)
            .unwrap()
    }

    fn source(&self) -> String {
        self.markdown().source().to_owned()
    }

    /// `(anchor, caret)`.
    fn ends(&self) -> (usize, usize) {
        (self.editor().anchor(), self.editor().caret())
    }

    fn selection(&self) -> Option<Range<usize>> {
        self.editor().selection()
    }

    fn caret_at(&mut self, at: usize) {
        self.select(at, at);
    }

    fn select(&mut self, anchor: usize, caret: usize) {
        let world = self.app.world_mut();
        let (mut editor, markdown) = world
            .query::<(&mut MarkdownEditor, &Markdown)>()
            .single_mut(world)
            .unwrap();
        editor.set_selection(markdown, anchor, caret);
    }

    fn key(&mut self, key_code: KeyCode, logical_key: Key, state: ButtonState, text: Option<&str>) {
        self.app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key,
            state,
            text: text.map(Into::into),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    /// Write a key press and release, with Shift held around it when asked.
    /// No frame runs.
    fn press_events(&mut self, key_code: KeyCode, key: Key, shift: bool) {
        if shift {
            self.key(KeyCode::ShiftLeft, Key::Shift, ButtonState::Pressed, None);
        }
        self.key(key_code, key.clone(), ButtonState::Pressed, None);
        self.key(key_code, key, ButtonState::Released, None);
        if shift {
            self.key(KeyCode::ShiftLeft, Key::Shift, ButtonState::Released, None);
        }
    }

    /// A key, then frames until everything it asked for is applied.
    fn press(&mut self, key_code: KeyCode, key: Key, shift: bool) {
        self.press_events(key_code, key, shift);
        self.settle();
    }

    fn right(&mut self, shift: bool, times: usize) {
        for _ in 0..times {
            self.press(KeyCode::ArrowRight, Key::ArrowRight, shift);
        }
    }

    fn left(&mut self, shift: bool, times: usize) {
        for _ in 0..times {
            self.press(KeyCode::ArrowLeft, Key::ArrowLeft, shift);
        }
    }

    fn home(&mut self, shift: bool) {
        self.press(KeyCode::Home, Key::Home, shift);
    }

    fn end(&mut self, shift: bool) {
        self.press(KeyCode::End, Key::End, shift);
    }

    fn backspace(&mut self) {
        self.press(KeyCode::Backspace, Key::Backspace, false);
    }

    fn delete(&mut self) {
        self.press(KeyCode::Delete, Key::Delete, false);
    }

    fn escape(&mut self) {
        self.press(KeyCode::Escape, Key::Escape, false);
    }

    fn type_text(&mut self, text: &str) {
        self.type_events(text);
        self.settle();
    }

    /// Write the presses of `text`, one key per character. No frame runs.
    fn type_events(&mut self, text: &str) {
        for c in text.chars() {
            let s = c.to_string();
            for state in [ButtonState::Pressed, ButtonState::Released] {
                let text = (state == ButtonState::Pressed).then_some(s.as_str());
                self.key(
                    KeyCode::KeyA,
                    Key::Character(s.as_str().into()),
                    state,
                    text,
                );
            }
        }
    }

    fn commit(&mut self, text: &str) {
        self.app.world_mut().write_message(Ime::Commit {
            window: Entity::PLACEHOLDER,
            value: text.to_owned(),
        });
        self.settle();
    }

    /// A chord with the command key down (Cmd or Ctrl count the same).
    fn chord_events(&mut self, shift: bool, key_code: KeyCode, character: &str) {
        self.key(
            KeyCode::ControlLeft,
            Key::Control,
            ButtonState::Pressed,
            None,
        );
        if shift {
            self.key(KeyCode::ShiftLeft, Key::Shift, ButtonState::Pressed, None);
        }
        for state in [ButtonState::Pressed, ButtonState::Released] {
            self.key(key_code, Key::Character(character.into()), state, None);
        }
        if shift {
            self.key(KeyCode::ShiftLeft, Key::Shift, ButtonState::Released, None);
        }
        self.key(
            KeyCode::ControlLeft,
            Key::Control,
            ButtonState::Released,
            None,
        );
    }

    fn select_all(&mut self) {
        self.chord_events(false, KeyCode::KeyA, "a");
        self.settle();
    }

    fn undo(&mut self) {
        self.chord_events(false, KeyCode::KeyZ, "z");
        self.settle();
    }

    fn redo(&mut self) {
        self.chord_events(true, KeyCode::KeyZ, "z");
        self.settle();
    }

    fn settle(&mut self) {
        settle(&mut self.app);
    }

    fn undo_steps(&self) -> usize {
        self.editor().history().undo_len()
    }

    /// The document equals what a full parse of its source gives.
    fn assert_is_a_full_parse(&self) {
        let document = self.markdown().document();
        document.assert_consistent();
        let full = Document::parse(document.source());
        assert_eq!(document.len(), full.len());
        assert!(
            document.blocks().zip(full.blocks()).all(|(a, b)| a == b),
            "the document is not what a full parse gives"
        );
    }

    /// The selection rectangles of every text that has any, with the block
    /// of the document each belongs to.
    fn selection_rects(&mut self) -> Vec<(usize, Vec<Rect>)> {
        let world = self.app.world_mut();
        let texts: Vec<(Entity, Vec<Rect>)> = world
            .query_filtered::<(Entity, &TextLayoutInfo), With<BlockText>>()
            .iter(world)
            .filter(|(_, info)| !info.selection_rects.is_empty())
            .map(|(entity, info)| (entity, info.selection_rects.clone()))
            .collect();
        let markdown = self.markdown();
        texts
            .into_iter()
            .map(|(text, rects)| {
                let block = (0..markdown.document().len())
                    .find(|ix| {
                        let Some(root) = markdown.block_entity(*ix) else {
                            return false;
                        };
                        let mut at = text;
                        loop {
                            if at == root {
                                return true;
                            }
                            match self.app.world().get::<ChildOf>(at) {
                                Some(parent) => at = parent.parent(),
                                None => return false,
                            }
                        }
                    })
                    .expect("the selection is drawn on a block of the document");
                (block, rects)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Making a selection
// ---------------------------------------------------------------------------

#[test]
fn shift_with_the_arrows_extends_from_a_fixed_anchor() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.caret_at(2);
    assert_eq!(doc.selection(), None);

    doc.right(true, 3);
    assert_eq!(doc.ends(), (2, 5), "the anchor stays, the caret moves");
    assert_eq!(doc.selection(), Some(2..5));

    // Back past the anchor: a selection made backwards.
    doc.left(true, 4);
    assert_eq!(doc.ends(), (2, 1));
    assert_eq!(doc.selection(), Some(1..2));

    // Back onto the anchor: nothing is selected any more.
    doc.right(true, 1);
    assert_eq!(doc.ends(), (2, 2));
    assert_eq!(doc.selection(), None);
    assert_eq!(doc.source(), "hello world\n", "selecting edits nothing");
}

#[test]
fn shift_with_home_and_end_extends_to_the_line_edges() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.caret_at(6);
    doc.end(true);
    assert_eq!(doc.ends(), (6, 11));
    assert_eq!(doc.selection(), Some(6..11));

    doc.caret_at(6);
    doc.home(true);
    assert_eq!(doc.ends(), (6, 0));
    assert_eq!(doc.selection(), Some(0..6));
}

#[test]
fn extending_never_splits_a_character() {
    let _serial = serial();
    let mut doc = Open::new("aç日😀z\n");
    doc.caret_at(0);
    doc.right(true, 4);
    assert_eq!(doc.ends(), (0, 10), "a, ç, 日 and 😀 are 10 bytes");
    doc.left(true, 1);
    assert_eq!(doc.ends(), (0, 6));
}

#[test]
fn left_and_right_collapse_a_selection_to_its_edges() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");

    doc.select(2, 5);
    doc.right(false, 1);
    assert_eq!(doc.ends(), (5, 5), "Right goes to the right edge");

    doc.select(2, 5);
    doc.left(false, 1);
    assert_eq!(doc.ends(), (2, 2), "Left goes to the left edge");

    // The same for a selection made backwards.
    doc.select(5, 2);
    doc.right(false, 1);
    assert_eq!(doc.ends(), (5, 5));
    doc.select(5, 2);
    doc.left(false, 1);
    assert_eq!(doc.ends(), (2, 2));

    // Without a selection the arrows step one character, as before.
    doc.right(false, 1);
    assert_eq!(doc.ends(), (3, 3));
    doc.left(false, 2);
    assert_eq!(doc.ends(), (1, 1));
}

#[test]
fn home_and_end_collapse_a_selection_and_move_the_caret() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.select(2, 5);
    doc.home(false);
    assert_eq!(doc.ends(), (0, 0));

    doc.select(2, 5);
    doc.end(false);
    assert_eq!(doc.ends(), (11, 11));

    // Already at the edge: the selection still goes.
    doc.select(3, 11);
    doc.end(false);
    assert_eq!(doc.ends(), (11, 11));
}

#[test]
fn select_all_takes_the_whole_document_and_escape_drops_it() {
    let _serial = serial();
    let mut doc = Open::new("one\n\ntwo\n\nthree\n");
    doc.caret_at(1);
    doc.select_all();
    let selection = doc.selection().expect("everything is selected");
    assert_eq!(selection.start, 0);
    assert_eq!(doc.editor().anchor(), 0);
    assert!(
        selection.end >= "one\n\ntwo\n\nthree".len(),
        "up to the end of the last block, got {selection:?}"
    );
    assert_eq!(doc.source(), "one\n\ntwo\n\nthree\n");

    let caret = doc.editor().caret();
    doc.escape();
    assert_eq!(doc.selection(), None);
    assert_eq!(doc.ends(), (caret, caret), "Escape keeps the caret");
    assert_eq!(doc.source(), "one\n\ntwo\n\nthree\n");

    // Escape with nothing selected is a no-op.
    doc.escape();
    assert_eq!(doc.ends(), (caret, caret));
}

#[test]
fn select_all_works_with_either_command_key_and_in_an_empty_document() {
    let _serial = serial();
    let mut doc = Open::new("");
    doc.select_all();
    assert_eq!(doc.selection(), None, "nothing to select");

    let mut doc = Open::new("abc\n");
    doc.key(KeyCode::SuperLeft, Key::Super, ButtonState::Pressed, None);
    doc.key(
        KeyCode::KeyA,
        Key::Character("A".into()),
        ButtonState::Pressed,
        None,
    );
    doc.settle();
    assert_eq!(doc.selection().map(|s| s.start), Some(0));
    assert!(doc.selection().unwrap().end >= 3);
}

// ---------------------------------------------------------------------------
// Editing with a selection
// ---------------------------------------------------------------------------

#[test]
fn typing_over_a_selection_replaces_it_and_undo_gives_it_back() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.select(0, 5);
    doc.type_text("X");
    assert_eq!(doc.source(), "X world\n");
    assert_eq!(doc.ends(), (1, 1), "the caret is after the new text");
    assert_eq!(doc.undo_steps(), 1);
    doc.assert_is_a_full_parse();

    doc.undo();
    assert_eq!(doc.source(), "hello world\n", "byte for byte");
    assert_eq!(doc.ends(), (0, 5), "the selection is back");
    assert_eq!(doc.undo_steps(), 0);

    doc.redo();
    assert_eq!(doc.source(), "X world\n");
    assert_eq!(doc.ends(), (1, 1));
    doc.assert_is_a_full_parse();
}

#[test]
fn undo_restores_the_direction_of_a_selection_made_backwards() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.select(8, 2);
    doc.type_text("é");
    assert_eq!(doc.source(), "heérld\n");
    assert_eq!(doc.ends(), (4, 4));
    doc.undo();
    assert_eq!(doc.source(), "hello world\n");
    assert_eq!(doc.ends(), (8, 2), "anchor 8 and caret 2, as they were");
}

#[test]
fn ime_text_replaces_the_selection() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.select(6, 11);
    doc.commit("日本");
    assert_eq!(doc.source(), "hello 日本\n");
    assert_eq!(doc.ends(), (12, 12));
    assert_eq!(doc.undo_steps(), 1);
    doc.undo();
    assert_eq!(doc.source(), "hello world\n");
    assert_eq!(doc.ends(), (6, 11));
}

#[test]
fn backspace_and_delete_remove_only_the_selection() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.select(2, 7);
    doc.backspace();
    assert_eq!(doc.source(), "heorld\n", "not one more character");
    assert_eq!(doc.ends(), (2, 2));
    assert_eq!(doc.undo_steps(), 1);
    doc.assert_is_a_full_parse();

    doc.undo();
    assert_eq!(doc.source(), "hello world\n");
    assert_eq!(doc.ends(), (2, 7));

    // Delete does the same, and so on a backwards selection.
    doc.select(7, 2);
    doc.delete();
    assert_eq!(doc.source(), "heorld\n");
    assert_eq!(doc.ends(), (2, 2));
    assert_eq!(doc.undo_steps(), 1, "a step of its own, as before");
    doc.undo();
    assert_eq!(doc.source(), "hello world\n");
    assert_eq!(doc.ends(), (7, 2));
}

#[test]
fn a_selection_edit_is_never_merged_with_the_typing_around_it() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.caret_at(11);
    doc.type_text("zz");
    assert_eq!(doc.undo_steps(), 1);

    doc.select(0, 5);
    doc.type_text("X");
    assert_eq!(doc.source(), "X worldzz\n");
    assert_eq!(doc.undo_steps(), 2, "its own step, apart from `zz`");

    // Typing on after it, within the grouping window, is a step too.
    doc.type_text("ab");
    assert_eq!(doc.source(), "Xab worldzz\n");
    assert_eq!(doc.undo_steps(), 3);

    doc.undo();
    assert_eq!(doc.source(), "X worldzz\n");
    doc.undo();
    assert_eq!(doc.source(), "hello worldzz\n");
    assert_eq!(doc.ends(), (0, 5));
    doc.undo();
    assert_eq!(doc.source(), "hello world\n");
    assert_eq!(doc.undo_steps(), 0);
}

#[test]
fn an_empty_selection_edits_like_a_caret() {
    let _serial = serial();
    let mut doc = Open::new("hello\n");
    doc.select(3, 3);
    doc.backspace();
    assert_eq!(doc.source(), "helo\n", "one character, as without Shift");
    doc.right(true, 1);
    doc.left(true, 1);
    assert_eq!(doc.selection(), None);
    doc.type_text("L");
    assert_eq!(doc.source(), "heLlo\n");
}

#[test]
fn a_selection_across_blocks_is_one_edit_of_the_source() {
    let _serial = serial();
    let source = "one\n\ntwo\n\nthree\n";
    let mut doc = Open::new(source);
    // From after the `o` of "one" to after the `th` of "three".
    doc.select(1, 12);
    doc.delete();
    assert_eq!(doc.source(), "oree\n", "the two ends joined");
    assert_eq!(doc.markdown().document().len(), 1);
    assert_eq!(doc.ends(), (1, 1));
    assert_eq!(doc.undo_steps(), 1);
    doc.assert_is_a_full_parse();

    doc.undo();
    assert_eq!(doc.source(), source);
    assert_eq!(doc.ends(), (1, 12));
    assert_eq!(doc.markdown().document().len(), 3);
    doc.assert_is_a_full_parse();

    // Typing replaces all of it too, backwards this time.
    doc.select(12, 1);
    doc.type_text("X");
    assert_eq!(doc.source(), "oXree\n");
    assert_eq!(doc.ends(), (2, 2));
    doc.assert_is_a_full_parse();
    doc.undo();
    assert_eq!(doc.source(), source);
    assert_eq!(doc.ends(), (12, 1));
}

#[test]
fn a_selection_that_ends_in_the_blank_line_between_blocks_is_edited_too() {
    let _serial = serial();
    let source = "one\n\n\n\ntwo\n";
    let mut doc = Open::new(source);
    // Some byte in the blank lines is in no block.
    let gap = (0..source.len())
        .find(|at| doc.markdown().document().block_at(*at).is_none())
        .expect("a byte between the blocks");
    assert!(gap > 3 && gap < 8, "{gap}");
    doc.select(1, gap);
    doc.delete();
    assert_eq!(doc.source(), format!("o{}", &source[gap..]));
    doc.assert_is_a_full_parse();
    doc.undo();
    assert_eq!(doc.source(), source);
}

#[test]
fn the_structure_of_the_first_block_survives_a_selection_that_leaves_it() {
    let _serial = serial();
    let mut doc = Open::new("# Title\n\nbody text\n");
    // After `# T` to after `body`.
    doc.select(3, 13);
    doc.delete();
    assert_eq!(doc.source(), "# T text\n");
    assert_eq!(
        doc.markdown().document().block(0).unwrap().kind,
        BlockKind::Heading(1)
    );
    doc.assert_is_a_full_parse();
}

#[test]
fn deleting_everything_leaves_an_empty_document_and_undo_brings_it_back() {
    let _serial = serial();
    for source in [
        "one\n\ntwo\n\nthree\n",
        "# Title\n\nbody **bold**\n",
        "- a\n- b\n\n> quote\n",
        "text",
    ] {
        let mut doc = Open::new(source);
        doc.select_all();
        doc.backspace();
        assert_eq!(doc.markdown().document().len(), 0, "{source:?}");
        assert!(doc.source().trim().is_empty(), "{:?}", doc.source());
        doc.assert_is_a_full_parse();

        doc.undo();
        assert_eq!(doc.source(), source, "undo is exact");
        assert_eq!(doc.selection().map(|s| s.start), Some(0));
        doc.assert_is_a_full_parse();
    }
}

#[test]
fn typing_over_select_all_replaces_the_document() {
    let _serial = serial();
    let mut doc = Open::new("# Title\n\nbody\n");
    doc.select_all();
    doc.type_text("new");
    assert!(doc.source().starts_with("new"), "{:?}", doc.source());
    assert_eq!(doc.markdown().document().len(), 1);
    assert_eq!(
        doc.markdown().document().block(0).unwrap().kind,
        BlockKind::Paragraph
    );
    doc.assert_is_a_full_parse();
}

#[test]
fn select_all_and_typing_in_one_frame_keep_their_order() {
    let _serial = serial();
    let mut doc = Open::new("hello\n");
    doc.chord_events(false, KeyCode::KeyA, "a");
    doc.type_events("hi");
    doc.settle();
    assert!(doc.source().starts_with("hi"), "{:?}", doc.source());
    assert!(!doc.source().contains("hello"));
    doc.assert_is_a_full_parse();
}

// ---------------------------------------------------------------------------
// Selecting and deleting styled text
// ---------------------------------------------------------------------------

#[test]
fn deleting_a_selected_styled_run_takes_its_delimiters() {
    let _serial = serial();
    let mut doc = Open::new("a **b** c\n");
    // The `b`: the caret before the `**` and one character on.
    doc.caret_at(2);
    doc.right(true, 1);
    assert_eq!(doc.selection(), Some(2..5));
    doc.delete();
    assert_eq!(doc.source(), "a  c\n", "no `****` left behind");
    doc.assert_is_a_full_parse();
    doc.undo();
    assert_eq!(doc.source(), "a **b** c\n");
    assert_eq!(doc.ends(), (2, 5));
}

#[test]
fn typing_over_a_selected_styled_run_keeps_its_style() {
    let _serial = serial();
    let mut doc = Open::new("a **b** c\n");
    doc.caret_at(2);
    doc.right(true, 1);
    doc.type_text("x");
    assert_eq!(doc.source(), "a **x** c\n");
    doc.assert_is_a_full_parse();
}

#[test]
fn a_selection_from_a_styled_run_to_the_end_takes_the_delimiters_it_covers() {
    let _serial = serial();
    let mut doc = Open::new("a **b** c\n");
    // "b c": the whole bold run and the text after it.
    doc.caret_at(2);
    doc.right(true, 3);
    doc.delete();
    assert_eq!(doc.source(), "a \n", "no dangling `**`");
    doc.assert_is_a_full_parse();
}

#[test]
fn a_selection_that_covers_a_styled_run_only_in_part_keeps_the_markdown_balanced() {
    let _serial = serial();
    // The tail of a bold run and the plain text after it: the closing `**`
    // is in the deleted bytes and is put back.
    let mut doc = Open::new("a **bold** c\n");
    doc.select(5, 12);
    doc.delete();
    assert_eq!(doc.source(), "a **b**\n");
    doc.assert_is_a_full_parse();
    doc.undo();
    assert_eq!(doc.source(), "a **bold** c\n", "undo is exact");
    assert_eq!(doc.ends(), (5, 12));

    // Typing over it puts the new text inside the bold, where it began.
    doc.type_text("X");
    assert_eq!(doc.source(), "a **bX**\n");
    assert_eq!(doc.ends(), (6, 6), "the caret is after the `X`");
    doc.assert_is_a_full_parse();
    doc.undo();
    assert_eq!(doc.source(), "a **bold** c\n");

    // The text after a bold run, from the end of the run.
    let mut doc = Open::new("a **b** c\n");
    doc.select(5, 9);
    doc.delete();
    assert_eq!(doc.source(), "a **b**\n");
    doc.assert_is_a_full_parse();

    // Inside one run.
    let mut doc = Open::new("a **bold** c\n");
    doc.select(5, 7);
    doc.delete();
    assert_eq!(doc.source(), "a **bd** c\n");
    doc.assert_is_a_full_parse();
}

#[test]
fn a_selection_across_blocks_keeps_the_styles_it_cuts_balanced() {
    let _serial = serial();
    // From the middle of a bold run to the next block: its closing is put back.
    let source = "a **bold**\n\nnext para\n";
    let mut doc = Open::new(source);
    doc.select(6, 17);
    doc.delete();
    assert_eq!(doc.source(), "a **bo**para\n");
    assert_eq!(doc.markdown().document().len(), 1);
    doc.assert_is_a_full_parse();
    doc.undo();
    assert_eq!(doc.source(), source);
    assert_eq!(doc.ends(), (6, 17));

    // From the previous block into the middle of a bold run: its opening is.
    let source = "one\n\n**bold** rest\n";
    let mut doc = Open::new(source);
    doc.select(1, 9);
    doc.delete();
    assert_eq!(doc.source(), "o**ld** rest\n");
    assert_eq!(doc.markdown().document().len(), 1);
    doc.assert_is_a_full_parse();
    doc.undo();
    assert_eq!(doc.source(), source);
}

#[test]
fn nested_styles_keep_the_delimiters_of_a_style_the_selection_leaves() {
    let _serial = serial();
    // Select "b c", inside the bold text "a *b* c": the bold goes on.
    let mut doc = Open::new("**a *b* c**\n");
    // Text "a b c": "b c" is text bytes 2..5 → source after `**a `.
    doc.select(4, 10);
    doc.delete();
    assert_eq!(doc.source(), "**a **\n");
    doc.assert_is_a_full_parse();

    // The whole italic run goes with its delimiters, the bold stays.
    let mut doc = Open::new("**a *b* c**\n");
    doc.select(4, 7);
    doc.delete();
    assert_eq!(doc.source(), "**a  c**\n");
    doc.assert_is_a_full_parse();
}

#[test]
fn a_selected_code_span_goes_with_its_backticks() {
    let _serial = serial();
    let mut doc = Open::new("a `code` b\n");
    doc.select(3, 7);
    doc.delete();
    assert_eq!(doc.source(), "a  b\n");
    doc.assert_is_a_full_parse();
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

#[test]
fn the_selected_text_of_the_caret_block_is_drawn_and_nothing_else() {
    let _serial = serial();
    let mut doc = Open::with_window("hello world\n\nsecond block\n");
    doc.caret_at(0);
    doc.settle();
    assert!(
        doc.selection_rects().is_empty(),
        "nothing selected, none drawn"
    );

    doc.right(true, 2);
    let drawn = doc.selection_rects();
    assert_eq!(drawn.len(), 1, "one block shows a selection");
    let (block, rects) = &drawn[0];
    assert_eq!(*block, 0);
    assert_eq!(rects.len(), 1, "one line, one rectangle");
    let two = rects[0];
    assert!(two.width() > 0.0 && two.height() > 0.0);
    assert!(two.min.x.abs() < 1.0, "it starts at the left edge");

    // Longer selection, wider rectangle.
    doc.right(true, 3);
    let five = doc.selection_rects()[0].1[0];
    assert!(five.width() > two.width());
    assert!((five.min.x - two.min.x).abs() < 0.01);

    // A selection that moves back shrinks it.
    doc.left(true, 3);
    let back = doc.selection_rects()[0].1[0];
    assert!((back.width() - two.width()).abs() < 0.01);

    // Esc drops it from the screen.
    doc.escape();
    assert_eq!(doc.selection_rects().len(), 0);
}

#[test]
fn a_selection_starting_mid_text_is_drawn_from_there() {
    let _serial = serial();
    let mut doc = Open::with_window("hello world\n");
    doc.select(6, 11);
    doc.settle();
    let drawn = doc.selection_rects();
    let rect = drawn[0].1[0];
    assert!(rect.min.x > 10.0, "it starts after `hello `: {rect:?}");
    assert!(rect.width() > 0.0);
}

#[test]
fn a_selection_across_blocks_is_drawn_in_the_block_of_the_caret() {
    let _serial = serial();
    let mut doc = Open::with_window("one\n\ntwo\n\nthree and more\n");
    // Backwards from inside the last block to inside the first: the caret
    // is in the first.
    doc.select(14, 1);
    doc.settle();
    let drawn = doc.selection_rects();
    assert_eq!(drawn.len(), 1, "only the caret's block draws: {drawn:?}");
    assert_eq!(drawn[0].0, 0);
    assert!(
        drawn[0].1[0].min.x > 0.0,
        "the `o` of `one` is not selected"
    );

    // Forwards: the caret is in the last block, and the selection there
    // starts at its left edge.
    doc.select(1, 14);
    doc.settle();
    let drawn = doc.selection_rects();
    assert_eq!(drawn.len(), 1);
    assert_eq!(drawn[0].0, 2);
    assert!(drawn[0].1[0].min.x.abs() < 1.0, "from the block's start");
}

#[test]
fn select_all_is_drawn_over_the_text_of_the_caret_block() {
    let _serial = serial();
    let mut doc = Open::with_window("one\n\nthree and more\n");
    doc.select_all();
    doc.settle();
    let drawn = doc.selection_rects();
    assert_eq!(drawn.len(), 1);
    assert_eq!(drawn[0].0, 1, "the last block holds the caret");
    let rect = drawn[0].1[0];
    assert!(rect.min.x.abs() < 1.0);
    assert!(rect.width() > 40.0, "the whole line: {rect:?}");
}

#[test]
fn the_drawn_selection_follows_the_edit_that_removes_it() {
    let _serial = serial();
    let mut doc = Open::with_window("hello world\n");
    doc.select(0, 5);
    doc.settle();
    assert_eq!(doc.selection_rects().len(), 1);
    doc.backspace();
    doc.settle();
    assert_eq!(doc.selection_rects().len(), 0);
    assert_eq!(doc.source(), " world\n");
}

#[test]
fn a_selection_on_a_wrapped_text_is_drawn_line_by_line() {
    let _serial = serial();
    let long = "word ".repeat(120);
    let mut doc = Open::with_window(&format!("{long}\n"));
    doc.select(0, long.len() - 1);
    doc.settle();
    let drawn = doc.selection_rects();
    assert_eq!(drawn.len(), 1);
    let rects = &drawn[0].1;
    assert!(rects.len() > 1, "wrapped over {} lines", rects.len());
    for pair in rects.windows(2) {
        assert!(
            pair[1].min.y >= pair[0].max.y - 0.01,
            "lines do not overlap"
        );
        assert!(
            (pair[1].min.y - pair[0].max.y).abs() < 0.5,
            "and leave no gap between them"
        );
    }
}
