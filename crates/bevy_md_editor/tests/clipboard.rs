//! Copy, cut, and paste over a selection.
//!
//! Selections are made with `MarkdownEditor::set_selection`, so these tests
//! do not depend on how a selection is drawn with the keyboard or the mouse.

#[path = "../../bevy_md/tests/common/mod.rs"]
mod common;

use bevy::clipboard::Clipboard;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::window::Ime;
use bevy_md::Markdown;
use bevy_md_core::Document;
use bevy_md_editor::{EditMetrics, MarkdownEditor, MarkdownEditorPlugin};
use common::*;

struct Open {
    app: App,
    markdown: Entity,
}

impl Open {
    fn new(source: &str) -> Self {
        let mut app = headless_app();
        app.update();
        app.add_message::<Ime>();
        app.add_plugins(MarkdownEditorPlugin);
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

    fn set_source(&mut self, source: &str) {
        let entity = self.markdown;
        self.app
            .world_mut()
            .get_mut::<Markdown>(entity)
            .unwrap()
            .set_source(source);
    }

    fn caret_at(&mut self, at: usize) {
        let world = self.app.world_mut();
        let (mut editor, markdown) = world
            .query::<(&mut MarkdownEditor, &Markdown)>()
            .single_mut(world)
            .unwrap();
        editor.set_caret(markdown, at);
    }

    fn select(&mut self, anchor: usize, caret: usize) {
        let world = self.app.world_mut();
        let (mut editor, markdown) = world
            .query::<(&mut MarkdownEditor, &Markdown)>()
            .single_mut(world)
            .unwrap();
        editor.set_selection(markdown, anchor, caret);
    }

    fn key(&mut self, key_code: KeyCode, logical_key: Key, state: ButtonState) {
        self.app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key,
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    /// A command chord, all in one frame. No frame runs.
    fn chord_events(&mut self, key_code: KeyCode, character: &str) {
        self.key(KeyCode::SuperLeft, Key::Super, ButtonState::Pressed);
        self.key(
            key_code,
            Key::Character(character.into()),
            ButtonState::Pressed,
        );
        self.key(
            key_code,
            Key::Character(character.into()),
            ButtonState::Released,
        );
        self.key(KeyCode::SuperLeft, Key::Super, ButtonState::Released);
    }

    fn copy_events(&mut self) {
        self.chord_events(KeyCode::KeyC, "c");
    }

    fn cut_events(&mut self) {
        self.chord_events(KeyCode::KeyX, "x");
    }

    fn paste_events(&mut self) {
        self.chord_events(KeyCode::KeyV, "v");
    }

    fn undo_events(&mut self) {
        self.chord_events(KeyCode::KeyZ, "z");
    }

    fn redo_events(&mut self) {
        self.key(KeyCode::SuperLeft, Key::Super, ButtonState::Pressed);
        self.key(KeyCode::ShiftLeft, Key::Shift, ButtonState::Pressed);
        for state in [ButtonState::Pressed, ButtonState::Released] {
            self.key(KeyCode::KeyZ, Key::Character("z".into()), state);
        }
        self.key(KeyCode::ShiftLeft, Key::Shift, ButtonState::Released);
        self.key(KeyCode::SuperLeft, Key::Super, ButtonState::Released);
    }

    fn type_events(&mut self, text: &str) {
        for c in text.chars() {
            let s = c.to_string();
            self.app.world_mut().write_message(KeyboardInput {
                key_code: KeyCode::KeyA,
                logical_key: Key::Character(s.as_str().into()),
                state: ButtonState::Pressed,
                text: Some(s.as_str().into()),
                repeat: false,
                window: Entity::PLACEHOLDER,
            });
        }
    }

    fn set_clipboard(&mut self, text: &str) {
        self.app
            .world_mut()
            .resource_mut::<Clipboard>()
            .set_text(text.to_owned())
            .expect("the in-process clipboard takes text");
    }

    /// What is on the clipboard, `None` when it cannot be read as text.
    fn clipboard(&mut self) -> Option<String> {
        self.app
            .world_mut()
            .resource_mut::<Clipboard>()
            .fetch_text()
            .poll_result()
            .and_then(Result::ok)
    }

    fn frame(&mut self) {
        self.app.update();
    }

    fn settle(&mut self) {
        settle(&mut self.app);
    }

    fn metrics(&self) -> &EditMetrics {
        self.app.world().resource::<EditMetrics>()
    }

    fn undo_steps(&self) -> usize {
        self.editor().history().undo_len()
    }

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

    fn assert_selected(&self, anchor: usize, caret: usize) {
        assert_eq!(
            (self.editor().anchor(), self.editor().caret()),
            (anchor, caret)
        );
    }
}

const SOURCE: &str = "Some **bold** and *italic* text\n\nSecond block\n";

// ---------------------------------------------------------------------------
// Copy
// ---------------------------------------------------------------------------

#[test]
fn copy_puts_the_selected_markdown_source_on_the_clipboard_and_changes_nothing_else() {
    let _serial = serial();
    let mut doc = Open::new(SOURCE);
    doc.set_clipboard("before");
    // "**bold** and" with its delimiters.
    doc.select(5, 17);
    doc.copy_events();
    doc.frame();
    assert_eq!(doc.clipboard().as_deref(), Some("**bold** and"));
    assert_eq!(doc.source(), SOURCE);
    doc.assert_selected(5, 17);
    assert_eq!(doc.undo_steps(), 0);
    assert_eq!(doc.metrics().edits, 0);
    assert_eq!(doc.editor().selection(), Some(5..17));
}

#[test]
fn copy_of_a_backward_selection_copies_the_same_text_and_keeps_both_ends() {
    let _serial = serial();
    let mut doc = Open::new(SOURCE);
    doc.select(17, 5);
    doc.copy_events();
    doc.frame();
    assert_eq!(doc.clipboard().as_deref(), Some("**bold** and"));
    doc.assert_selected(17, 5);
}

#[test]
fn copy_spans_blocks_and_takes_the_blank_line_between_them() {
    let _serial = serial();
    let mut doc = Open::new(SOURCE);
    let end = SOURCE.find("Second").unwrap() + "Sec".len();
    doc.select(26, end);
    doc.copy_events();
    doc.frame();
    assert_eq!(doc.clipboard().as_deref(), Some(&SOURCE[26..end]));
    assert!(doc.clipboard().unwrap().contains("\n\n"));
    assert_eq!(doc.source(), SOURCE);
}

#[test]
fn copy_without_a_selection_does_nothing() {
    let _serial = serial();
    let mut doc = Open::new(SOURCE);
    doc.set_clipboard("before");
    doc.caret_at(7);
    doc.copy_events();
    doc.frame();
    assert_eq!(doc.clipboard().as_deref(), Some("before"));
    assert_eq!(doc.source(), SOURCE);
    assert_eq!(doc.editor().caret(), 7);
    assert_eq!(doc.metrics().copy_failures, 0);
}

#[test]
fn copy_does_not_cut_the_word_being_typed_out_of_its_undo_step() {
    let _serial = serial();
    let mut doc = Open::new("start\n");
    doc.caret_at(5);
    doc.type_events("ab");
    doc.frame();
    doc.select(0, 3);
    doc.copy_events();
    doc.frame();
    doc.caret_at(7);
    doc.type_events("c");
    doc.frame();
    assert_eq!(doc.source(), "startabc\n");
    assert_eq!(doc.undo_steps(), 1, "typing went on as one step");
}

#[test]
fn copy_keeps_multibyte_text_whole() {
    let _serial = serial();
    let mut doc = Open::new("aç日😀z\n");
    // ç日😀 is bytes 1..10.
    doc.select(1, 10);
    doc.copy_events();
    doc.frame();
    assert_eq!(doc.clipboard().as_deref(), Some("ç日😀"));
    // An end inside a character is moved back to a boundary.
    doc.select(0, 2);
    doc.copy_events();
    doc.frame();
    assert_eq!(doc.clipboard().as_deref(), Some("a"));
}

#[test]
fn the_copy_and_cut_keys_and_control_work_like_the_chords() {
    let _serial = serial();
    let mut doc = Open::new("abcdef\n");
    doc.select(1, 3);
    doc.key(KeyCode::Copy, Key::Copy, ButtonState::Pressed);
    doc.frame();
    assert_eq!(doc.clipboard().as_deref(), Some("bc"));
    assert_eq!(doc.source(), "abcdef\n");

    doc.key(KeyCode::Cut, Key::Cut, ButtonState::Pressed);
    doc.frame();
    assert_eq!(doc.source(), "adef\n");

    doc.select(0, 1);
    doc.key(KeyCode::ControlLeft, Key::Control, ButtonState::Pressed);
    doc.key(
        KeyCode::KeyC,
        Key::Character("C".into()),
        ButtonState::Pressed,
    );
    doc.frame();
    assert_eq!(doc.clipboard().as_deref(), Some("a"));
    doc.key(
        KeyCode::KeyX,
        Key::Character("x".into()),
        ButtonState::Pressed,
    );
    doc.frame();
    assert_eq!(doc.source(), "def\n");
}

#[test]
fn c_and_x_without_the_command_key_are_letters() {
    let _serial = serial();
    let mut doc = Open::new("abc\n");
    doc.set_clipboard("clip");
    doc.caret_at(3);
    doc.type_events("cx");
    doc.frame();
    assert_eq!(doc.source(), "abccx\n");
    assert_eq!(doc.clipboard().as_deref(), Some("clip"));
}

// ---------------------------------------------------------------------------
// Cut
// ---------------------------------------------------------------------------

#[test]
fn cut_moves_the_selection_to_the_clipboard_and_puts_the_caret_where_it_was() {
    let _serial = serial();
    let mut doc = Open::new(SOURCE);
    doc.select(5, 13);
    doc.cut_events();
    doc.frame();
    assert_eq!(doc.clipboard().as_deref(), Some("**bold**"));
    assert_eq!(doc.source(), SOURCE.replace("**bold**", ""));
    assert_eq!(doc.editor().caret(), 5);
    assert_eq!(doc.editor().selection(), None);
    assert_eq!(doc.undo_steps(), 1);
    doc.assert_is_a_full_parse();
}

#[test]
fn undo_of_a_cut_gives_back_the_text_and_the_selection_byte_for_byte() {
    let _serial = serial();
    for (anchor, caret) in [(5, 13), (13, 5)] {
        let mut doc = Open::new(SOURCE);
        doc.select(anchor, caret);
        doc.cut_events();
        doc.frame();
        assert_ne!(doc.source(), SOURCE);

        doc.undo_events();
        doc.frame();
        assert_eq!(doc.source(), SOURCE);
        doc.assert_selected(anchor, caret);
        doc.assert_is_a_full_parse();

        // Redo cuts again, and a second undo selects again.
        doc.redo_events();
        doc.frame();
        assert_eq!(doc.source(), SOURCE.replace("**bold**", ""));
        assert_eq!(doc.editor().selection(), None);
        doc.undo_events();
        doc.frame();
        assert_eq!(doc.source(), SOURCE);
        doc.assert_selected(anchor, caret);
    }
}

#[test]
fn a_cut_is_one_step_apart_from_the_typing_around_it() {
    let _serial = serial();
    let mut doc = Open::new("one two three\n");
    doc.caret_at(3);
    doc.type_events("x");
    doc.frame();
    doc.select(5, 8);
    doc.cut_events();
    doc.frame();
    doc.type_events("y");
    doc.frame();
    assert_eq!(doc.undo_steps(), 3, "typed, cut, typed");
    assert_eq!(doc.source(), "onex y three\n");
}

#[test]
fn cut_across_blocks_joins_what_is_left_and_undoes_whole() {
    let _serial = serial();
    let mut doc = Open::new(SOURCE);
    let start = SOURCE.find("text").unwrap();
    let end = SOURCE.find("block").unwrap();
    doc.select(start, end);
    doc.cut_events();
    doc.frame();
    assert_eq!(doc.clipboard().as_deref(), Some(&SOURCE[start..end]));
    assert_eq!(
        doc.source(),
        format!("{}{}", &SOURCE[..start], &SOURCE[end..])
    );
    assert_eq!(doc.editor().caret(), start);
    doc.assert_is_a_full_parse();

    doc.undo_events();
    doc.frame();
    assert_eq!(doc.source(), SOURCE);
    doc.assert_selected(start, end);
    doc.assert_is_a_full_parse();
}

#[test]
fn cutting_everything_leaves_an_empty_document_and_undo_restores_it() {
    let _serial = serial();
    let mut doc = Open::new(SOURCE);
    doc.select(0, SOURCE.len());
    doc.cut_events();
    doc.frame();
    assert_eq!(doc.source(), "");
    assert_eq!(doc.clipboard().as_deref(), Some(SOURCE));
    assert_eq!(doc.markdown().document().len(), 0);

    doc.undo_events();
    doc.frame();
    assert_eq!(doc.source(), SOURCE);
    doc.assert_selected(0, SOURCE.len());
    doc.assert_is_a_full_parse();
}

#[test]
fn cut_without_a_selection_does_nothing() {
    let _serial = serial();
    let mut doc = Open::new(SOURCE);
    doc.set_clipboard("before");
    doc.caret_at(7);
    doc.cut_events();
    doc.frame();
    assert_eq!(doc.source(), SOURCE);
    assert_eq!(doc.clipboard().as_deref(), Some("before"));
    assert_eq!(doc.undo_steps(), 0);
}

#[test]
fn a_cut_that_cannot_reach_the_clipboard_deletes_nothing() {
    let _serial = serial();
    let mut doc = Open::new(SOURCE);
    // Only the editor's systems run: Bevy's own text systems need the resource.
    doc.app.world_mut().remove_resource::<Clipboard>();
    doc.select(5, 13);
    doc.cut_events();
    doc.app.world_mut().run_schedule(Update);
    assert_eq!(doc.source(), SOURCE);
    assert_eq!(doc.metrics().copy_failures, 1);
    assert_eq!(doc.undo_steps(), 0);
    doc.assert_selected(5, 13);

    doc.copy_events();
    doc.app.world_mut().run_schedule(Update);
    assert_eq!(doc.metrics().copy_failures, 2);
}

#[test]
fn cut_then_paste_elsewhere_moves_the_text() {
    let _serial = serial();
    let mut doc = Open::new("abc def\n");
    doc.select(0, 4);
    doc.cut_events();
    doc.frame();
    assert_eq!(doc.source(), "def\n");
    doc.caret_at(3);
    doc.paste_events();
    doc.frame();
    assert_eq!(doc.source(), "defabc \n");
    assert_eq!(doc.editor().caret(), 7);
    assert_eq!(doc.editor().selection(), None);
    doc.assert_is_a_full_parse();
}

// ---------------------------------------------------------------------------
// Paste over a selection
// ---------------------------------------------------------------------------

#[test]
fn paste_replaces_the_selection_as_one_step_and_undo_restores_everything() {
    let _serial = serial();
    for (anchor, caret) in [(5, 13), (13, 5)] {
        let mut doc = Open::new(SOURCE);
        doc.set_clipboard("new\r\ntext");
        doc.select(anchor, caret);
        doc.paste_events();
        doc.frame();
        let pasted = SOURCE.replace("**bold**", "new\ntext");
        assert_eq!(doc.source(), pasted);
        assert_eq!(doc.editor().caret(), 5 + "new\ntext".len());
        assert_eq!(doc.editor().selection(), None);
        assert_eq!(doc.undo_steps(), 1);
        doc.assert_is_a_full_parse();

        doc.undo_events();
        doc.frame();
        assert_eq!(doc.source(), SOURCE);
        doc.assert_selected(anchor, caret);
        doc.assert_is_a_full_parse();

        doc.redo_events();
        doc.frame();
        assert_eq!(doc.source(), pasted);
        assert_eq!(doc.editor().caret(), 5 + "new\ntext".len());
        doc.assert_is_a_full_parse();
    }
}

#[test]
fn paste_over_a_selection_does_not_merge_with_the_typing_around_it() {
    let _serial = serial();
    let mut doc = Open::new("one two three\n");
    doc.caret_at(3);
    doc.type_events("x");
    doc.frame();
    doc.set_clipboard("Z");
    doc.select(5, 8);
    doc.paste_events();
    doc.frame();
    doc.type_events("y");
    doc.frame();
    assert_eq!(doc.source(), "onex Zy three\n");
    assert_eq!(doc.undo_steps(), 3, "typed, paste, typed");
}

#[test]
fn paste_over_the_selection_that_was_just_copied_puts_the_same_text_back() {
    let _serial = serial();
    let mut doc = Open::new("abcdef\n");
    doc.select(1, 4);
    doc.copy_events();
    doc.frame();
    // The same selection still stands: the paste puts the same text back.
    doc.paste_events();
    doc.frame();
    assert_eq!(doc.source(), "abcdef\n");
    assert_eq!(doc.editor().caret(), 4);
    assert_eq!(doc.editor().selection(), None);
    assert_eq!(doc.undo_steps(), 1);
}

#[test]
fn an_empty_clipboard_leaves_the_selection_alone() {
    let _serial = serial();
    let mut doc = Open::new("abcdef\n");
    doc.set_clipboard("");
    doc.select(1, 4);
    doc.paste_events();
    doc.frame();
    assert_eq!(doc.source(), "abcdef\n");
    doc.assert_selected(1, 4);
    assert_eq!(doc.metrics().paste_failures, 0);
    assert_eq!(doc.undo_steps(), 0);
}

#[test]
fn paste_without_a_selection_still_inserts_at_the_caret() {
    let _serial = serial();
    let mut doc = Open::new("abc\n");
    doc.set_clipboard("XY");
    doc.caret_at(1);
    doc.paste_events();
    doc.frame();
    assert_eq!(doc.source(), "aXYbc\n");
    assert_eq!(doc.editor().caret(), 3);
    assert_eq!(doc.editor().selection(), None);
}

#[test]
fn pasting_twice_replaces_nothing_the_second_time() {
    let _serial = serial();
    let mut doc = Open::new("abcdef\n");
    doc.set_clipboard("Z");
    doc.select(1, 3);
    doc.paste_events();
    doc.frame();
    doc.paste_events();
    doc.frame();
    assert_eq!(doc.source(), "aZZdef\n");
    assert_eq!(doc.undo_steps(), 2);
}

// ---------------------------------------------------------------------------
// Queued while a large change parses
// ---------------------------------------------------------------------------

/// Some markdown larger than `Markdown::SYNC_LIMIT`, so that a change
/// bringing it in is parsed in the background.
fn large_source() -> String {
    let text = format!("hello world\n\n{}", "word ".repeat(2_000));
    assert!(text.len() > Markdown::SYNC_LIMIT);
    text
}

#[test]
fn copy_and_cut_wait_for_the_parse_and_act_on_the_source_that_landed() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.set_clipboard("old");
    let large = large_source();
    doc.set_source(&large);
    doc.select(0, 5);
    doc.copy_events();
    doc.cut_events();
    doc.app.world_mut().run_schedule(Update);
    assert_eq!(doc.editor().pending_actions(), 2, "both wait for the parse");
    assert_eq!(doc.clipboard().as_deref(), Some("old"));
    assert_eq!(doc.source(), large);

    doc.settle();
    assert_eq!(doc.editor().pending_actions(), 0);
    assert_eq!(doc.clipboard().as_deref(), Some("hello"));
    assert_eq!(doc.source(), large[5..]);
    assert_eq!(doc.editor().caret(), 0);
    doc.assert_is_a_full_parse();
}

#[test]
fn a_paste_queued_behind_a_copy_pastes_what_the_copy_took() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.set_clipboard("old");
    let large = large_source();
    doc.set_source(&large);
    doc.select(0, 5);
    doc.copy_events();
    doc.paste_events();
    doc.app.world_mut().run_schedule(Update);
    assert_eq!(doc.editor().pending_actions(), 2);

    doc.settle();
    // "hello" replaced by "hello": the clipboard the paste read was the
    // copy's, not "old".
    assert_eq!(doc.source(), large);
    let last = doc.editor().history().steps().last().unwrap();
    assert_eq!(last.new(), "hello");
    assert_eq!(last.old(), "hello");
}

#[test]
fn a_paste_queued_behind_nothing_reads_the_clipboard_of_the_key_press() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.set_clipboard("pressed");
    let large = large_source();
    doc.set_source(&large);
    doc.select(0, 5);
    doc.paste_events();
    doc.app.world_mut().run_schedule(Update);
    doc.set_clipboard("changed after");
    doc.settle();
    assert_eq!(doc.source(), format!("pressed{}", &large[5..]));
    assert_eq!(doc.editor().caret(), "pressed".len());
}

#[test]
fn a_selection_made_stale_by_a_shorter_replacement_is_not_copied_out_of_range() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.set_clipboard("before");
    doc.select(2, 11);
    doc.set_source("hi\n");
    doc.copy_events();
    doc.frame();
    assert_eq!(doc.source(), "hi\n");
    assert!(doc.editor().caret() <= 3);
    let copied = doc.clipboard().unwrap();
    assert!(copied == "before" || "hi\n".contains(&copied));
    doc.cut_events();
    doc.frame();
    assert!(doc.markdown().source().len() <= 3);
    doc.assert_is_a_full_parse();
}
