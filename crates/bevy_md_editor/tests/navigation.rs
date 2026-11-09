//! Moving the caret by lines and across blocks: Up and Down keep the column,
//! the caret goes on into the previous and next block, Page Up and Page Down
//! move a viewport, and Cmd/Ctrl+Home and End go to the ends of the
//! document. With Shift every one of them grows the selection instead.
//!
//! The lines used to tell columns apart are made of the same letter, so that
//! two lines that should have the caret at the same column have it at the
//! same byte whatever the font is.

#[path = "../../bevy_md/tests/common/mod.rs"]
mod common;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::window::{Ime, PrimaryWindow};
use bevy_md::{Markdown, Pinned};
use bevy_md_core::{BlockKind, synthetic};
use bevy_md_editor::{MarkdownEditor, MarkdownEditorPlugin};
use common::*;

const SHIFT: (KeyCode, Key) = (KeyCode::ShiftLeft, Key::Shift);
const CTRL: (KeyCode, Key) = (KeyCode::ControlLeft, Key::Control);

struct Open {
    app: App,
    area: Entity,
    markdown: Entity,
}

impl Open {
    fn new(source: &str) -> Self {
        let mut app = headless_app();
        app.update();
        app.add_message::<Ime>();
        app.add_plugins(MarkdownEditorPlugin);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
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

    fn editor(&self) -> &MarkdownEditor {
        self.app
            .world()
            .get::<MarkdownEditor>(self.markdown)
            .unwrap()
    }

    fn source(&self) -> String {
        self.markdown().source().to_owned()
    }

    fn caret(&self) -> usize {
        self.editor().caret()
    }

    /// The block the caret is in.
    fn block(&self) -> usize {
        self.markdown()
            .document()
            .block_at(self.caret())
            .expect("the caret is in a block")
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

    /// Press `key` with the `modifiers` held, as one chord, and run a frame.
    fn press_with(&mut self, modifiers: &[(KeyCode, Key)], key_code: KeyCode, key: Key) {
        for (code, modifier) in modifiers {
            self.key(*code, modifier.clone(), ButtonState::Pressed, None);
        }
        self.key(key_code, key.clone(), ButtonState::Pressed, None);
        self.key(key_code, key, ButtonState::Released, None);
        for (code, modifier) in modifiers.iter().rev() {
            self.key(*code, modifier.clone(), ButtonState::Released, None);
        }
        self.frame();
    }

    fn up(&mut self) {
        self.press_with(&[], KeyCode::ArrowUp, Key::ArrowUp);
    }

    fn down(&mut self) {
        self.press_with(&[], KeyCode::ArrowDown, Key::ArrowDown);
    }

    fn left(&mut self) {
        self.press_with(&[], KeyCode::ArrowLeft, Key::ArrowLeft);
    }

    fn right(&mut self) {
        self.press_with(&[], KeyCode::ArrowRight, Key::ArrowRight);
    }

    fn page_up(&mut self) {
        self.press_with(&[], KeyCode::PageUp, Key::PageUp);
    }

    fn page_down(&mut self) {
        self.press_with(&[], KeyCode::PageDown, Key::PageDown);
    }

    fn type_text(&mut self, text: &str) {
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
            self.frame();
            self.frame();
        }
    }

    fn frame(&mut self) {
        self.app.update();
    }

    fn settle(&mut self) {
        settle(&mut self.app);
    }

    /// The block that carries the editor: the pinned one, which is the one
    /// the editor sees the caret in.
    fn focused_block(&mut self) -> usize {
        let world = self.app.world_mut();
        let pinned: Vec<Entity> = world
            .query_filtered::<Entity, With<Pinned>>()
            .iter(world)
            .collect();
        assert_eq!(pinned.len(), 1, "one block is pinned");
        self.markdown()
            .live_blocks()
            .find_map(|(ix, entity)| (entity == pinned[0]).then_some(ix))
            .expect("the pinned block is a block of the document")
    }

    /// The only block that carries the editor is the caret's.
    fn assert_focus_follows_the_caret(&mut self) {
        let block = self.block();
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
    }
}

/// One paragraph of three lines (hard breaks): the first and last have the
/// same text, the middle one is short. (A code block would do, but its text
/// has no source map yet, so the caret cannot be in it.)
const CODE: &str = "abcdefghij\\\nxy\\\nabcdefghij\n";

struct Lines {
    first: usize,
    short: usize,
    last: usize,
}

fn code_lines() -> Lines {
    Lines {
        first: 0,
        short: CODE.find("xy").unwrap(),
        last: CODE.rfind("abcdefghij").unwrap(),
    }
}

// ---------------------------------------------------------------------------
// Up and Down inside a block
// ---------------------------------------------------------------------------

#[test]
fn up_and_down_keep_the_column_through_a_shorter_line() {
    let _serial = serial();
    let mut doc = Open::new(CODE);
    let at = code_lines();
    doc.caret_at(at.first + 6);

    doc.down();
    assert_eq!(doc.caret(), at.short + 2, "the end of the short line");
    doc.down();
    assert_eq!(doc.caret(), at.last + 6, "back to the column it started at");
    doc.up();
    assert_eq!(doc.caret(), at.short + 2);
    doc.up();
    assert_eq!(doc.caret(), at.first + 6);
    assert_eq!(doc.source(), CODE, "moving never edits");
    doc.assert_focus_follows_the_caret();
}

#[test]
fn typing_or_clicking_forgets_the_column() {
    let _serial = serial();
    let mut doc = Open::new(CODE);
    let at = code_lines();
    doc.caret_at(at.first + 6);
    doc.down();
    assert_eq!(doc.caret(), at.short + 2);

    // Typing at the end of the short line: the column is now 3.
    doc.type_text("z");
    assert_eq!(doc.caret(), at.short + 3);
    doc.down();
    assert_eq!(doc.caret(), at.last + 1 + 3, "aims at the column typed at");

    // A click (a caret set from outside) is a new column too.
    doc.caret_at(at.first + 6);
    doc.down();
    assert_eq!(doc.caret(), at.short + 3, "the end of `xyz`");
    doc.caret_at(at.short + 1);
    doc.down();
    assert_eq!(doc.caret(), at.last + 1 + 1);
}

#[test]
fn a_horizontal_move_forgets_the_column() {
    let _serial = serial();
    let mut doc = Open::new(CODE);
    let at = code_lines();
    doc.caret_at(at.first + 6);
    doc.down();
    assert_eq!(doc.caret(), at.short + 2);
    doc.left();
    assert_eq!(doc.caret(), at.short + 1);
    doc.down();
    assert_eq!(doc.caret(), at.last + 1, "column 1, not 6");
}

#[test]
fn up_and_down_move_between_the_lines_of_a_wrapped_paragraph() {
    let _serial = serial();
    let source = format!("{}\n", "word ".repeat(300));
    let mut doc = Open::new(&source);
    doc.caret_at(5);

    doc.down();
    let second = doc.caret();
    assert_eq!(doc.block(), 0, "still the same paragraph");
    assert!(second > 5 + 20, "a whole line further: {second}");
    assert_eq!(second % 5, 0, "the same column of the same word pattern");
    doc.down();
    let third = doc.caret();
    assert!(third > second + 20);
    assert_eq!(doc.block(), 0);
    assert_eq!(third % 5, 0);

    doc.up();
    assert_eq!(doc.caret(), second);
    doc.up();
    assert_eq!(doc.caret(), 5);
    doc.assert_focus_follows_the_caret();
}

// ---------------------------------------------------------------------------
// Across blocks
// ---------------------------------------------------------------------------

const PARAGRAPHS: &str = "aaaaaaaa\n\naa\n\naaaaaaaa\n";

#[test]
fn down_and_up_cross_blocks_at_the_column_of_the_first_line() {
    let _serial = serial();
    let mut doc = Open::new(PARAGRAPHS);
    let starts: Vec<usize> = doc
        .markdown()
        .document()
        .blocks()
        .map(|block| block.span().start)
        .collect();
    assert_eq!(starts.len(), 3);

    doc.caret_at(starts[0] + 5);
    doc.down();
    assert_eq!(doc.block(), 1);
    assert_eq!(doc.caret(), starts[1] + 2, "the end of the short block");
    doc.assert_focus_follows_the_caret();

    doc.down();
    assert_eq!(doc.block(), 2);
    assert_eq!(doc.caret(), starts[2] + 5, "the column it started at");
    doc.assert_focus_follows_the_caret();

    doc.up();
    assert_eq!(doc.block(), 1);
    assert_eq!(doc.caret(), starts[1] + 2);
    doc.up();
    assert_eq!(doc.block(), 0);
    assert_eq!(doc.caret(), starts[0] + 5);
    doc.assert_focus_follows_the_caret();
    assert_eq!(doc.source(), PARAGRAPHS);
}

#[test]
fn a_block_of_several_lines_is_entered_by_its_first_or_its_last() {
    let _serial = serial();
    let source = "aaaaaaaa\n\naaaaaaaaaa\\\nbb\\\naaaaaaaaaa\n\naaaaaaaa\n";
    let mut doc = Open::new(source);
    let starts: Vec<usize> = doc
        .markdown()
        .document()
        .blocks()
        .map(|block| block.span().start)
        .collect();
    assert_eq!(starts.len(), 3);
    let (first, short, last) = (
        starts[1],
        source.find("bb").unwrap(),
        source.rfind("aaaaaaaaaa").unwrap(),
    );

    doc.caret_at(starts[0] + 5);
    doc.down();
    assert_eq!(doc.caret(), first + 5, "the first line of the next block");
    doc.down();
    assert_eq!(doc.caret(), short + 2);
    doc.down();
    assert_eq!(doc.caret(), last + 5, "the column is still the first one");
    doc.down();
    assert_eq!(doc.caret(), starts[2] + 5);
    doc.assert_focus_follows_the_caret();

    doc.up();
    assert_eq!(doc.caret(), last + 5, "the last line of the block above");
    doc.up();
    doc.up();
    assert_eq!(doc.caret(), first + 5);
    doc.up();
    assert_eq!(doc.caret(), starts[0] + 5);
    assert_eq!(doc.source(), source);
    doc.assert_focus_follows_the_caret();

    // Down off the last line of the document goes to the end of its text.
    doc.caret_at(starts[2] + 5);
    doc.down();
    assert_eq!(doc.caret(), starts[2] + 8);
}

#[test]
fn left_and_right_cross_between_blocks() {
    let _serial = serial();
    let mut doc = Open::new("one\n\ntwo\n\nthree\n");
    doc.caret_at(3);
    assert_eq!(doc.block(), 0);

    doc.right();
    assert_eq!(doc.block(), 1);
    assert_eq!(doc.caret(), 5, "the start of `two`");
    doc.assert_focus_follows_the_caret();
    doc.right();
    assert_eq!(doc.caret(), 6, "an ordinary step inside the block");
    doc.right();
    doc.right();
    assert_eq!(doc.caret(), 8, "the end of `two`");
    doc.right();
    assert_eq!(doc.block(), 2);
    assert_eq!(doc.caret(), 10, "the start of `three`");
    doc.assert_focus_follows_the_caret();

    doc.left();
    assert_eq!(doc.block(), 1);
    assert_eq!(doc.caret(), 8, "the end of `two`");
    doc.left();
    doc.left();
    doc.left();
    assert_eq!(doc.caret(), 5);
    doc.left();
    assert_eq!(doc.block(), 0);
    assert_eq!(doc.caret(), 3, "the end of `one`");
    doc.assert_focus_follows_the_caret();

    // The ends of the document stay put.
    doc.caret_at(0);
    doc.left();
    assert_eq!(doc.caret(), 0);
    let end = doc.source().trim_end().len();
    doc.caret_at(end);
    doc.right();
    assert_eq!(doc.caret(), end);
}

#[test]
fn a_block_that_starts_where_the_last_ends_is_a_place_of_its_own() {
    let _serial = serial();
    // No blank line between them: the paragraph starts at byte 8, which is
    // also where the heading's span ends.
    let mut doc = Open::new("# Title\nbody text\n");
    assert_eq!(doc.markdown().document().len(), 2);
    assert_eq!(
        doc.markdown().document().block(0).unwrap().kind,
        BlockKind::Heading(1)
    );
    doc.caret_at(7);
    doc.frame();
    assert_eq!(doc.focused_block(), 0);

    doc.right();
    assert_eq!(doc.caret(), 8);
    assert_eq!(doc.focused_block(), 1, "the caret is in the paragraph");
    doc.right();
    assert_eq!(doc.caret(), 9);
    assert_eq!(doc.focused_block(), 1);

    doc.caret_at(8);
    doc.frame();
    assert_eq!(
        doc.focused_block(),
        1,
        "a click at the start of the paragraph"
    );
    doc.left();
    assert_eq!(doc.caret(), 7);
    assert_eq!(doc.focused_block(), 0);

    // And a line move gets there too.
    doc.caret_at(3);
    doc.down();
    assert_eq!(doc.focused_block(), 1);
    doc.up();
    assert_eq!(doc.focused_block(), 0);
    assert_eq!(doc.source(), "# Title\nbody text\n");
}

#[test]
fn blocks_without_text_are_skipped() {
    let _serial = serial();
    let source = "above\n\n---\n\nbelow\n";
    let mut doc = Open::new(source);
    let blocks = doc.markdown().document().len();
    assert_eq!(blocks, 3, "above, the rule, below");
    let below = source.find("below").unwrap();

    doc.caret_at(2);
    doc.down();
    assert_eq!(doc.block(), 2, "the rule is stepped over");
    assert_eq!(doc.caret(), below + 2);
    doc.up();
    assert_eq!(doc.block(), 0);
    assert_eq!(doc.caret(), 2);

    doc.caret_at(5);
    doc.right();
    assert_eq!(doc.caret(), below);
    doc.left();
    assert_eq!(doc.caret(), 5);
}

#[test]
fn the_first_and_last_lines_go_to_the_ends_of_the_document() {
    let _serial = serial();
    let mut doc = Open::new("aaaa\n\nbbbb\n");
    doc.caret_at(2);
    doc.up();
    assert_eq!(doc.caret(), 0, "Up on the first line of the document");
    doc.up();
    assert_eq!(doc.caret(), 0);

    doc.caret_at(8);
    doc.down();
    assert_eq!(doc.caret(), 10, "Down on the last line of the document");
    doc.down();
    assert_eq!(doc.caret(), 10);
    doc.assert_focus_follows_the_caret();
}

// ---------------------------------------------------------------------------
// The ends of the document, and pages
// ---------------------------------------------------------------------------

#[test]
fn control_home_and_end_go_to_the_ends_of_the_document() {
    let _serial = serial();
    let mut doc = Open::new("# Title\n\nmiddle paragraph\n\nlast words\n");
    let middle = doc.source().find("middle").unwrap();
    doc.caret_at(middle + 3);

    doc.press_with(&[CTRL], KeyCode::End, Key::End);
    assert_eq!(doc.caret(), doc.source().trim_end().len());
    assert_eq!(doc.block(), 2);
    doc.assert_focus_follows_the_caret();

    doc.press_with(&[CTRL], KeyCode::Home, Key::Home);
    assert_eq!(doc.caret(), 2, "the start of the title's text");
    assert_eq!(doc.block(), 0);
    doc.assert_focus_follows_the_caret();
}

#[test]
fn command_up_and_down_do_the_same() {
    let _serial = serial();
    let mut doc = Open::new("first\n\nsecond\n");
    doc.caret_at(3);
    doc.press_with(
        &[(KeyCode::SuperLeft, Key::Super)],
        KeyCode::ArrowDown,
        Key::ArrowDown,
    );
    assert_eq!(doc.caret(), doc.source().trim_end().len());
    doc.press_with(
        &[(KeyCode::SuperLeft, Key::Super)],
        KeyCode::ArrowUp,
        Key::ArrowUp,
    );
    assert_eq!(doc.caret(), 0);
}

/// A paragraph of `lines` lines, each `row 000` and a hard break: 9 bytes.
fn rows(lines: usize) -> (String, usize) {
    let source = (0..lines)
        .map(|n| format!("row {n:03}"))
        .collect::<Vec<_>>()
        .join("\\\n");
    (source + "\n", 0)
}

/// The line of the paragraph made by [`rows`] that has byte `caret`.
fn row_of(caret: usize) -> usize {
    caret / 9
}

#[test]
fn page_down_and_up_move_about_a_viewport_of_lines() {
    let _serial = serial();
    let (source, start) = rows(300);
    let mut doc = Open::new(&source);
    doc.caret_at(start);

    doc.page_down();
    let line = row_of(doc.caret());
    // The viewport is 720 px of lines about 20 px tall, less one for context.
    assert!((15..=45).contains(&line), "moved {line} lines");
    doc.page_down();
    let second = row_of(doc.caret());
    assert_eq!(second, 2 * line, "the same distance again");

    doc.page_up();
    assert_eq!(row_of(doc.caret()), line);
    doc.page_up();
    assert_eq!(doc.caret(), start, "back where it began");
    doc.page_up();
    assert_eq!(doc.caret(), start, "the first line is the top");
    assert_eq!(doc.source(), source);
    doc.assert_focus_follows_the_caret();
}

#[test]
fn page_down_past_the_last_line_goes_to_its_end() {
    let _serial = serial();
    let (source, start) = rows(20);
    let mut doc = Open::new(&source);
    doc.caret_at(start + 9 * 15);
    doc.page_down();
    assert_eq!(
        doc.caret(),
        source.trim_end().len(),
        "the end of the last row"
    );
}

#[test]
fn page_down_crosses_blocks() {
    let _serial = serial();
    let source = synthetic::source(300);
    let mut doc = Open::new(&source);
    let first = doc.markdown().document().block(0).unwrap().span().start + 2;
    doc.caret_at(first);
    doc.page_down();
    assert!(
        doc.block() > 2,
        "went past the first blocks: {}",
        doc.block()
    );
    let reached = doc.block();
    doc.page_down();
    let second = doc.block();
    assert!(second > reached);
    doc.page_up();
    assert!(doc.block() < second, "and back up again");
    doc.assert_focus_follows_the_caret();
}

// ---------------------------------------------------------------------------
// Shift
// ---------------------------------------------------------------------------

#[test]
fn shift_extends_the_selection_with_every_movement() {
    let _serial = serial();
    let mut doc = Open::new(CODE);
    let at = code_lines();
    doc.caret_at(at.first + 6);

    doc.press_with(&[SHIFT], KeyCode::ArrowDown, Key::ArrowDown);
    assert_eq!(doc.caret(), at.short + 2);
    assert_eq!(doc.editor().anchor(), at.first + 6, "the anchor stays");
    assert_eq!(doc.editor().selection(), Some(at.first + 6..at.short + 2));

    doc.press_with(&[SHIFT], KeyCode::ArrowDown, Key::ArrowDown);
    assert_eq!(doc.caret(), at.last + 6, "the column is still remembered");
    assert_eq!(doc.editor().selection(), Some(at.first + 6..at.last + 6));

    // Back past the anchor: a selection made backwards.
    for _ in 0..3 {
        doc.press_with(&[SHIFT], KeyCode::ArrowUp, Key::ArrowUp);
    }
    assert_eq!(doc.caret(), at.first, "the first line, column 0");
    assert_eq!(doc.editor().anchor(), at.first + 6);
    assert_eq!(doc.editor().selection(), Some(at.first..at.first + 6));

    // Without Shift the selection collapses onto the caret.
    doc.down();
    assert_eq!(doc.editor().selection(), None);
    assert_eq!(doc.editor().anchor(), doc.caret());
    assert_eq!(doc.source(), CODE);
}

#[test]
fn shift_extends_across_blocks_and_to_the_ends_of_the_document() {
    let _serial = serial();
    let mut doc = Open::new(PARAGRAPHS);
    let starts: Vec<usize> = doc
        .markdown()
        .document()
        .blocks()
        .map(|block| block.span().start)
        .collect();
    doc.caret_at(starts[0] + 5);

    doc.press_with(&[SHIFT], KeyCode::ArrowDown, Key::ArrowDown);
    doc.press_with(&[SHIFT], KeyCode::ArrowDown, Key::ArrowDown);
    assert_eq!(doc.block(), 2);
    assert_eq!(doc.editor().anchor(), starts[0] + 5);
    assert_eq!(doc.editor().selection(), Some(starts[0] + 5..starts[2] + 5));
    doc.assert_focus_follows_the_caret();

    doc.press_with(&[SHIFT, CTRL], KeyCode::End, Key::End);
    assert_eq!(doc.caret(), starts[2] + 8);
    assert_eq!(doc.editor().anchor(), starts[0] + 5);

    doc.press_with(&[SHIFT, CTRL], KeyCode::Home, Key::Home);
    assert_eq!(doc.caret(), 0);
    assert_eq!(
        doc.editor().anchor(),
        starts[0] + 5,
        "still the same anchor"
    );
    assert_eq!(doc.editor().selection(), Some(0..starts[0] + 5));
}

#[test]
fn shift_page_down_selects_a_page() {
    let _serial = serial();
    let (source, start) = rows(300);
    let mut doc = Open::new(&source);
    doc.caret_at(start);
    doc.press_with(&[SHIFT], KeyCode::PageDown, Key::PageDown);
    let selection = doc.editor().selection().expect("a selection");
    assert_eq!(selection.start, start);
    assert!(selection.len() > 9 * 15);
}

// ---------------------------------------------------------------------------
// Blocks that are not laid out
// ---------------------------------------------------------------------------

#[test]
fn moving_in_and_out_of_blocks_with_no_layout_never_fails() {
    let _serial = serial();
    let source = synthetic::source(2_000);
    let mut doc = Open::new(&source);
    let (target, start) = doc
        .markdown()
        .document()
        .blocks()
        .enumerate()
        .skip(10)
        .find_map(|(ix, block)| {
            matches!(block.kind, BlockKind::Paragraph).then_some((ix, block.span().start))
        })
        .expect("a paragraph");
    set_scroll(&mut doc.app, doc.area, 20_000.0);
    doc.settle();
    assert!(
        doc.markdown().block_entity(target).is_none(),
        "block {target} is out of the window"
    );
    doc.caret_at(start + 3);

    // Down goes to a later block, Up comes back, every key lands on a block.
    doc.down();
    assert!(doc.caret() > start + 3);
    assert!(doc.block() > target);
    doc.up();
    assert_eq!(doc.block(), target, "back in the paragraph it left");

    doc.caret_at(start);
    doc.left();
    assert!(doc.block() < target, "the end of an earlier block");
    doc.right();
    assert_eq!(doc.block(), target);
    assert_eq!(doc.caret(), start);

    doc.page_down();
    assert!(doc.block() > target);
    doc.page_up();
    doc.press_with(&[CTRL], KeyCode::End, Key::End);
    assert!(doc.block() >= target);
    doc.press_with(&[CTRL], KeyCode::Home, Key::Home);
    assert_eq!(doc.block(), 0);
    assert_eq!(doc.source(), source, "moving never edits");
}
