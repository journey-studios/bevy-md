//! Input must never be lost: keys, committed IME text, shortcuts and paste
//! that arrive while a large change is parsing in the background (or while
//! the layout of an edit is a frame away) wait in order and are applied to
//! the document they were meant for.
//!
//! None of these tests depends on how fast the parse thread is. A parse is
//! installed by the plugin's sync, which runs after the `Update` systems of a
//! frame, so an event written before a frame is read while the document is
//! still behind the source, whatever the thread did meanwhile; where a test
//! needs the parse to be done by a given frame it calls
//! `Markdown::wait_for_parse`, and where it needs it *not* to land it runs
//! only the `Update` schedule.

#[path = "../../bevy_md/tests/common/mod.rs"]
mod common;

use bevy::clipboard::Clipboard;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::text::{TextCursorStyle, TextLayoutInfo};
use bevy::window::{Ime, PrimaryWindow};
use bevy_md::{BlockText, Markdown, Pinned};
use bevy_md_core::{BlockKind, Document, synthetic};
use bevy_md_editor::{EditMetrics, MarkdownEditor, MarkdownEditorPlugin, PlaceCaret};
use common::*;

struct Open {
    app: App,
    area: Entity,
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
        let (area, markdown) = spawn_document(&mut app, source);
        app.world_mut()
            .entity_mut(markdown)
            .insert(MarkdownEditor::new());
        settle(&mut app);
        Self {
            app,
            area,
            markdown,
        }
    }

    fn markdown(&self) -> &Markdown {
        self.app.world().get::<Markdown>(self.markdown).unwrap()
    }

    fn markdown_mut(&mut self) -> Mut<'_, Markdown> {
        let entity = self.markdown;
        self.app
            .world_mut()
            .get_mut::<Markdown>(entity)
            .expect("the markdown")
    }

    fn editor(&self) -> &MarkdownEditor {
        self.app
            .world()
            .get::<MarkdownEditor>(self.markdown)
            .unwrap()
    }

    /// The source after every change, including those still parsing.
    fn source(&self) -> String {
        self.markdown().source().to_owned()
    }

    fn set_source(&mut self, source: &str) {
        self.markdown_mut().set_source(source);
    }

    fn caret_at(&mut self, at: usize) {
        let world = self.app.world_mut();
        let (mut editor, markdown) = world
            .query::<(&mut MarkdownEditor, &Markdown)>()
            .single_mut(world)
            .unwrap();
        editor.set_caret(markdown, at);
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

    fn commit_events(&mut self, text: &str) {
        self.app.world_mut().write_message(Ime::Commit {
            window: Entity::PLACEHOLDER,
            value: text.to_owned(),
        });
    }

    /// Write a chord: the modifiers down, the key down and up, the modifiers
    /// up. No frame runs, so the whole chord can land in one frame.
    fn chord_events(&mut self, shift: bool, key_code: KeyCode, character: &str) {
        self.key(KeyCode::SuperLeft, Key::Super, ButtonState::Pressed, None);
        if shift {
            self.key(KeyCode::ShiftLeft, Key::Shift, ButtonState::Pressed, None);
        }
        self.key(
            key_code,
            Key::Character(character.into()),
            ButtonState::Pressed,
            None,
        );
        self.key(
            key_code,
            Key::Character(character.into()),
            ButtonState::Released,
            None,
        );
        if shift {
            self.key(KeyCode::ShiftLeft, Key::Shift, ButtonState::Released, None);
        }
        self.key(KeyCode::SuperLeft, Key::Super, ButtonState::Released, None);
    }

    fn undo_events(&mut self) {
        self.chord_events(false, KeyCode::KeyZ, "z");
    }

    fn redo_events(&mut self) {
        self.chord_events(true, KeyCode::KeyZ, "z");
    }

    fn paste_events(&mut self) {
        self.chord_events(false, KeyCode::KeyV, "v");
    }

    fn set_clipboard(&mut self, text: &str) {
        self.app
            .world_mut()
            .resource_mut::<Clipboard>()
            .set_text(text.to_owned())
            .expect("the in-process clipboard takes text");
    }

    /// A frame.
    fn frame(&mut self) {
        self.app.update();
    }

    /// A frame that only runs the `Update` systems: the plugin's sync, which
    /// installs a finished parse, does not run in it.
    fn update_only(&mut self) {
        self.app.world_mut().run_schedule(Update);
    }

    fn settle(&mut self) {
        settle(&mut self.app);
    }

    fn pending(&self) -> usize {
        self.editor().pending_actions()
    }

    fn metrics(&self) -> &EditMetrics {
        self.app.world().resource::<EditMetrics>()
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

    /// The only block that carries the editor is the caret's.
    fn assert_focus_follows_the_caret(&mut self) {
        let caret = self.editor().caret();
        let block = self
            .markdown()
            .document()
            .block_at(caret)
            .expect("the caret is in a block");
        let root = self
            .markdown()
            .block_entity(block)
            .expect("the caret's block is shown");
        let world = self.app.world_mut();
        let pinned: Vec<Entity> = world
            .query_filtered::<Entity, With<Pinned>>()
            .iter(world)
            .collect();
        assert_eq!(pinned, vec![root], "the pinned block is the caret's");
        let editing = world
            .query_filtered::<Entity, With<TextCursorStyle>>()
            .iter(world)
            .count();
        assert_eq!(editing, 1, "one block has the editor");
    }
}

/// `source` with `text` inserted at byte `at`.
fn inserted(source: &str, at: usize, text: &str) -> String {
    let mut out = source.to_owned();
    out.insert_str(at, text);
    out
}

/// Some markdown larger than `Markdown::SYNC_LIMIT`, so that a change
/// bringing it in is parsed in the background.
fn large_text() -> String {
    let text = "word ".repeat(2_000);
    assert!(text.len() > Markdown::SYNC_LIMIT);
    text
}

/// A source other than `big` that is not an extension of it either.
fn other_source(blocks: usize) -> String {
    format!("# Another document\n\n{}", synthetic::source(blocks))
}

// ---------------------------------------------------------------------------
// Text and IME while a large change parses
// ---------------------------------------------------------------------------

#[test]
fn text_typed_while_a_large_change_parses_is_applied_in_order_once_it_lands() {
    let _serial = serial();
    let mut doc = Open::new("one\n\ntwo\n");
    doc.caret_at(3);
    let big = synthetic::source(500);
    doc.set_source(&big);
    assert!(doc.markdown().is_parsing());

    // Frame 1: two keys. They are read, not applied; the source the caret
    // refers to is still the replacement, untouched.
    doc.type_events("ab");
    doc.frame();
    assert_eq!(doc.pending(), 2, "kept, not dropped and not applied yet");
    assert_eq!(doc.source(), big);
    assert_eq!(doc.editor().caret(), 3, "the caret did not move");

    // Frame 2: committed IME text, a multi-byte one.
    doc.commit_events("日本");
    doc.frame();
    // Frame 3: another key.
    doc.type_events("c");
    doc.frame();

    doc.settle();
    assert_eq!(doc.pending(), 0);
    assert_eq!(doc.source(), inserted(&big, 3, "ab日本c"));
    assert_eq!(doc.markdown().document().source(), doc.source());
    assert_eq!(doc.editor().caret(), 3 + "ab日本c".len());
    doc.assert_is_a_full_parse();
    doc.assert_focus_follows_the_caret();
}

#[test]
fn input_of_several_frames_waits_while_the_parse_cannot_have_landed() {
    let _serial = serial();
    let mut doc = Open::new("one\n");
    doc.caret_at(3);
    let big = synthetic::source(500);
    doc.set_source(&big);

    // Only `Update` runs: the sync that installs a parse never does, so the
    // document stays behind the source for as many frames as we like.
    for (n, text) in ["a", "b", "c"].into_iter().enumerate() {
        doc.type_events(text);
        doc.update_only();
        assert_eq!(doc.pending(), n + 1);
        assert!(doc.markdown().is_parsing());
        assert_eq!(doc.source(), big, "nothing was applied to the source");
    }
    doc.commit_events("ç");
    doc.update_only();
    assert_eq!(doc.pending(), 4);

    doc.settle();
    assert_eq!(doc.pending(), 0);
    assert_eq!(doc.source(), inserted(&big, 3, "abcç"));
    doc.assert_is_a_full_parse();
}

#[test]
fn input_read_in_the_frame_the_parse_lands_in_is_applied_in_the_next() {
    let _serial = serial();
    let mut doc = Open::new("one\n");
    doc.caret_at(3);
    let big = synthetic::source(500);
    doc.set_source(&big);
    // The parse is done before the frame starts: it lands in the frame's sync,
    // after the `Update` that reads the keys.
    doc.markdown_mut().wait_for_parse();
    assert!(doc.markdown().is_parsing(), "not installed until a frame");

    doc.type_events("xy");
    doc.frame();
    assert!(!doc.markdown().is_parsing(), "it landed in that frame");
    assert_eq!(doc.markdown().document().source(), big);
    assert_eq!(doc.pending(), 2, "the keys were read before it landed");

    doc.frame();
    assert_eq!(doc.pending(), 0);
    assert_eq!(doc.source(), inserted(&big, 3, "xy"));
    doc.assert_is_a_full_parse();
}

#[test]
fn keys_after_the_parse_has_landed_go_behind_the_ones_that_waited() {
    let _serial = serial();
    let mut doc = Open::new("one\n");
    doc.caret_at(3);
    let big = synthetic::source(500);
    doc.set_source(&big);
    doc.type_events("a");
    doc.update_only();
    doc.markdown_mut().wait_for_parse();
    // A frame that lands the parse and also brings a new key: `a` was first.
    doc.type_events("b");
    doc.frame();
    doc.settle();
    assert_eq!(doc.source(), inserted(&big, 3, "ab"));
}

#[test]
fn a_replacement_while_keys_wait_gets_the_keys_and_not_the_one_it_replaced() {
    let _serial = serial();
    let mut doc = Open::new("one\n");
    doc.caret_at(3);
    let first = synthetic::source(500);
    let second = other_source(300);
    doc.set_source(&first);
    doc.type_events("k");
    doc.update_only();
    // A newer large change supersedes the one the key was waiting on.
    doc.set_source(&second);
    doc.type_events("m");
    doc.frame();
    doc.settle();
    assert_eq!(doc.pending(), 0);
    assert_eq!(doc.source(), inserted(&second, 3, "km"));
    doc.assert_is_a_full_parse();
}

#[test]
fn a_shorter_replacement_clamps_the_caret_instead_of_editing_out_of_range() {
    let _serial = serial();
    let long = synthetic::source(600);
    let mut doc = Open::new(&long);
    doc.caret_at(long.len() - 2);
    let short = other_source(200);
    assert!(short.len() < long.len() - 2 && short.len() > Markdown::SYNC_LIMIT);
    doc.set_source(&short);
    doc.type_events("!");
    doc.frame();
    doc.settle();
    assert_eq!(doc.source(), format!("{short}!"));
    assert_eq!(doc.editor().caret(), short.len() + 1);
    doc.assert_is_a_full_parse();
}

#[test]
fn the_caret_and_the_focus_do_not_point_into_the_old_document_while_parsing() {
    let _serial = serial();
    let mut doc = Open::with_window(&synthetic::source(2_000));
    // Inside the text of the first paragraph ("The party ...").
    let at = doc.markdown().document().block(1).unwrap().span().start + 3;
    doc.caret_at(at);
    doc.settle();
    doc.assert_focus_follows_the_caret();

    // Replace the document and type, then run frames that may or may not
    // land the parse: whichever they do, the focus is never on a block of the
    // old document that the caret's byte does not belong to, and nothing
    // panics.
    let smaller = synthetic::source(300);
    assert!(smaller.len() > Markdown::SYNC_LIMIT);
    doc.set_source(&smaller);
    doc.type_events("z");
    for _ in 0..3 {
        doc.frame();
        let world = doc.app.world_mut();
        let pinned = world
            .query_filtered::<Entity, With<Pinned>>()
            .iter(world)
            .count();
        assert!(pinned <= 1, "{pinned} pinned blocks");
    }
    doc.settle();
    assert_eq!(doc.source(), inserted(&smaller, at, "z"));
    assert_eq!(doc.pending(), 0);
    doc.assert_focus_follows_the_caret();

    // And the cursor is drawn, on the block the caret is in only.
    let block = doc.markdown().document().block_at(at + 1).unwrap();
    let root = doc.markdown().block_entity(block).unwrap();
    let world = doc.app.world_mut();
    let with_cursor: Vec<Entity> = world
        .query_filtered::<(Entity, &TextLayoutInfo), With<BlockText>>()
        .iter(world)
        .filter(|(_, info)| info.cursor.is_some())
        .map(|(e, _)| e)
        .collect();
    assert_eq!(with_cursor.len(), 1, "one cursor on screen");
    let mut parent = with_cursor[0];
    while parent != root {
        parent = world
            .get::<ChildOf>(parent)
            .expect("the cursor is under the caret's block")
            .parent();
    }
    assert!(doc.metrics().edits >= 1);
}

// ---------------------------------------------------------------------------
// Shortcuts, undo and redo while a large change parses
// ---------------------------------------------------------------------------

#[test]
fn undo_and_redo_pressed_during_a_parse_are_applied_in_order_after_it() {
    let _serial = serial();
    let mut doc = Open::new("hello\n");
    doc.caret_at(5);
    doc.type_events("abc");
    doc.frame();
    assert_eq!(doc.source(), "helloabc\n");

    // All in one frame: a large paste (the editor's own large change, so the
    // history it keeps stands), then keys that arrive while it parses.
    let pasted = large_text();
    doc.set_clipboard(&pasted);
    doc.paste_events();
    doc.undo_events();
    doc.type_events("q");
    doc.frame();
    assert_eq!(doc.pending(), 2, "the undo and the key wait for the parse");
    assert!(doc.source().contains(&pasted), "the paste is in the source");

    doc.settle();
    // The undo took back the paste (not `abc`), then `q` was typed.
    assert_eq!(doc.pending(), 0);
    assert_eq!(doc.source(), "helloabcq\n");
    // The paste was undone, so `q` dropped the redo of it.
    assert_eq!(doc.editor().history().redo_len(), 0);
    doc.assert_is_a_full_parse();
}

#[test]
fn redo_of_a_large_paste_restarts_the_parse_and_the_keys_after_it_wait_again() {
    let _serial = serial();
    let mut doc = Open::new("hello\n");
    doc.caret_at(5);
    let pasted = large_text();
    doc.set_clipboard(&pasted);
    // Paste, take it back, do it again, and type: the redo is a large change
    // of its own that is applied after the first parse landed, and the key
    // after it waits for that second parse.
    doc.paste_events();
    doc.undo_events();
    doc.redo_events();
    doc.type_events("q");
    doc.frame();
    doc.settle();
    assert_eq!(doc.pending(), 0);
    assert_eq!(doc.source(), format!("hello{pasted}q\n"));
    assert_eq!(doc.editor().caret(), 5 + pasted.len() + 1);
    doc.assert_is_a_full_parse();
}

#[test]
fn undo_after_a_change_made_by_someone_else_does_not_apply_to_the_new_text() {
    let _serial = serial();
    let mut doc = Open::new("one\n");
    doc.caret_at(3);
    doc.type_events("two");
    doc.frame();
    assert_eq!(doc.editor().history().undo_len(), 1);

    // The app replaces the document with a large one and the user presses
    // undo before it is installed.
    let big = synthetic::source(500);
    doc.set_source(&big);
    doc.undo_events();
    doc.frame();
    doc.settle();
    assert_eq!(doc.source(), big, "undo did not touch the new text");
    assert_eq!(doc.editor().history().undo_len(), 0);
    assert_eq!(doc.editor().history().redo_len(), 0);
    doc.assert_is_a_full_parse();
}

#[test]
fn undo_after_a_small_change_by_someone_else_drops_the_history_instead_of_panicking() {
    let _serial = serial();
    let mut doc = Open::new("one\n");
    doc.caret_at(3);
    doc.type_events("two");
    doc.frame();
    // A small replacement does not start a parse and does not bump the
    // revision: the step is stale and only its text tells.
    doc.set_source("abcdefg\n");
    doc.undo_events();
    doc.frame();
    assert_eq!(doc.source(), "abcdefg\n");
    assert_eq!(doc.editor().history().undo_len(), 0);
}

#[test]
fn a_chord_that_goes_down_and_up_inside_one_frame_keeps_its_modifier() {
    let _serial = serial();
    let mut doc = Open::new("one\n");
    doc.caret_at(3);
    doc.type_events("two");
    doc.frame();
    assert_eq!(doc.source(), "onetwo\n");
    // By the end of this frame the command key is already up again.
    doc.undo_events();
    doc.frame();
    assert_eq!(doc.source(), "one\n");
    doc.redo_events();
    doc.frame();
    assert_eq!(doc.source(), "onetwo\n");
}

// ---------------------------------------------------------------------------
// Clipboard
// ---------------------------------------------------------------------------

#[test]
fn the_paste_shortcut_inserts_the_clipboard_at_the_caret_as_one_undo_step() {
    let _serial = serial();
    let mut doc = Open::new("start end\n");
    doc.caret_at(5);
    // Typed right before and right after, within the grouping window: the
    // paste is still a step of its own.
    doc.type_events("ab");
    doc.frame();
    doc.set_clipboard("cd\r\nef\x07g");
    doc.paste_events();
    doc.frame();
    assert_eq!(doc.source(), "startabcd\nefg end\n");
    assert_eq!(doc.editor().caret(), 5 + "abcd\nefg".len());
    doc.type_events("h");
    doc.frame();
    assert_eq!(doc.editor().history().undo_len(), 3, "typed, paste, typed");

    doc.undo_events();
    doc.frame();
    doc.undo_events();
    doc.frame();
    assert_eq!(
        doc.source(),
        "startab end\n",
        "the whole paste went at once"
    );
    doc.assert_is_a_full_parse();
}

#[test]
fn control_v_and_the_paste_key_paste_too() {
    let _serial = serial();
    let mut doc = Open::new("x\n");
    doc.caret_at(1);
    doc.set_clipboard("1");
    doc.key(
        KeyCode::ControlLeft,
        Key::Control,
        ButtonState::Pressed,
        None,
    );
    doc.key(
        KeyCode::KeyV,
        Key::Character("V".into()),
        ButtonState::Pressed,
        None,
    );
    doc.frame();
    assert_eq!(doc.source(), "x1\n");
    doc.key(
        KeyCode::ControlLeft,
        Key::Control,
        ButtonState::Released,
        None,
    );
    doc.set_clipboard("2");
    doc.key(KeyCode::Paste, Key::Paste, ButtonState::Pressed, None);
    doc.frame();
    assert_eq!(doc.source(), "x12\n");
}

#[test]
fn v_without_the_command_key_is_a_letter_and_the_clipboard_stays_untouched() {
    let _serial = serial();
    let mut doc = Open::new("x\n");
    doc.caret_at(1);
    doc.set_clipboard("clip");
    doc.type_events("v");
    doc.frame();
    assert_eq!(doc.source(), "xv\n");
}

#[test]
fn an_empty_clipboard_pastes_nothing_and_is_not_a_failure() {
    let _serial = serial();
    let mut doc = Open::new("x\n");
    doc.caret_at(1);
    doc.set_clipboard("");
    doc.paste_events();
    doc.frame();
    assert_eq!(doc.metrics().paste_failures, 0);
    assert_eq!(doc.source(), "x\n");
    assert_eq!(doc.editor().history().undo_len(), 0);
}

#[test]
fn pasting_a_large_text_with_the_shortcut_parses_in_the_background_and_lands_whole() {
    let _serial = serial();
    let mut doc = Open::new("hello\n\nworld\n");
    doc.caret_at(5);
    let pasted = large_text();
    doc.set_clipboard(&pasted);
    doc.paste_events();
    doc.frame();
    // The caret is already after the pasted text, in the source's own
    // coordinates, whether or not the parse has landed.
    assert_eq!(doc.editor().caret(), 5 + pasted.len());
    doc.settle();
    assert_eq!(doc.source(), format!("hello{pasted}\n\nworld\n"));
    assert_eq!(doc.editor().caret(), 5 + pasted.len());
    doc.assert_is_a_full_parse();
    doc.assert_focus_follows_the_caret();

    doc.undo_events();
    doc.frame();
    doc.settle();
    assert_eq!(doc.source(), "hello\n\nworld\n");
    assert_eq!(doc.editor().caret(), 5);
}

// ---------------------------------------------------------------------------
// Layout, and clicks
// ---------------------------------------------------------------------------

#[test]
fn an_action_that_reads_the_layout_waits_for_the_frame_after_an_edit() {
    let _serial = serial();
    let mut doc = Open::new("hello world\n");
    doc.caret_at(11);
    // Insert, then Backspace, in one frame: the layout Backspace would read
    // is the one from before the insert.
    doc.type_events("!");
    doc.key(
        KeyCode::Backspace,
        Key::Backspace,
        ButtonState::Pressed,
        None,
    );
    doc.frame();
    assert_eq!(doc.source(), "hello world!\n");
    assert_eq!(doc.pending(), 1, "the Backspace waits for the new layout");
    doc.frame();
    assert_eq!(doc.source(), "hello world\n");
    assert_eq!(doc.pending(), 0);
    doc.assert_is_a_full_parse();
}

#[test]
fn many_edits_in_one_frame_stay_in_order_and_in_range() {
    let _serial = serial();
    let mut doc = Open::new("ab\n");
    doc.caret_at(2);
    doc.type_events("cd");
    for _ in 0..3 {
        doc.key(
            KeyCode::Backspace,
            Key::Backspace,
            ButtonState::Pressed,
            None,
        );
    }
    doc.type_events("X");
    doc.key(
        KeyCode::ArrowLeft,
        Key::ArrowLeft,
        ButtonState::Pressed,
        None,
    );
    doc.type_events("Y");
    doc.settle();
    // abcd -> three backspaces -> a -> aX -> left -> a|X -> aYX
    assert_eq!(doc.source(), "aYX\n");
    assert_eq!(doc.pending(), 0);
}

#[test]
fn a_click_during_a_parse_is_discarded_and_counted() {
    let _serial = serial();
    let mut doc = Open::new("one\n\ntwo\n");
    doc.caret_at(3);
    let big = synthetic::source(500);
    doc.set_source(&big);
    doc.app.world_mut().write_message(PlaceCaret {
        position: Vec2::new(100.0, 40.0),
    });
    doc.update_only();
    assert_eq!(doc.metrics().clicks_ignored, 1);
    assert_eq!(doc.editor().caret(), 3, "the caret stayed");
    doc.settle();
    assert_eq!(doc.metrics().clicks_ignored, 1);
}

#[test]
fn text_is_still_edited_in_a_block_that_has_no_layout() {
    let _serial = serial();
    let mut doc = Open::new(&synthetic::source(2_000));
    // A paragraph near the top that starts with three plain letters.
    let (target, start) = doc
        .markdown()
        .document()
        .blocks()
        .enumerate()
        .find_map(|(ix, b)| {
            let start = b.span().start;
            (matches!(b.kind, BlockKind::Paragraph)
                && doc.markdown().document().source()[start..start + 3]
                    .chars()
                    .all(|c| c.is_ascii_alphabetic()))
            .then_some((ix, start))
        })
        .expect("a plain paragraph");
    // Scroll far from it so that it has no node, and put the caret in it.
    set_scroll(&mut doc.app, doc.area, 20_000.0);
    doc.settle();
    assert!(
        doc.markdown().block_entity(target).is_none(),
        "block {target} is out of the window"
    );
    doc.caret_at(start + 3);
    let original = doc.source();

    doc.key(
        KeyCode::Backspace,
        Key::Backspace,
        ButtonState::Pressed,
        None,
    );
    doc.frame();
    let mut expected = original.clone();
    expected.remove(start + 2);
    assert_eq!(doc.source(), expected, "Backspace edited the text");
    assert_eq!(doc.editor().caret(), start + 2);
    assert_eq!(doc.pending(), 0);

    doc.key(
        KeyCode::ArrowRight,
        Key::ArrowRight,
        ButtonState::Pressed,
        None,
    );
    doc.frame();
    assert_eq!(doc.editor().caret(), start + 3);
    doc.assert_is_a_full_parse();
}
