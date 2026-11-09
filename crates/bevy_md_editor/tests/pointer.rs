//! The mouse: double and triple clicks, dragging (inside a block and across
//! blocks) and Shift+click. Every test looks at what ends up selected in the
//! markdown source, not at how it is drawn.
//!
//! Pointer input is written as [`PointerInput`] messages, the same ones the
//! mouse system produces, at pixels computed from the layout Bevy made: the
//! point just inside a character of a block's text. One test drives the
//! real path, a window's cursor and the button events.

#[path = "../../bevy_md/tests/common/mod.rs"]
mod common;

use std::time::Duration;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::input::mouse::{MouseButton, MouseButtonInput};
use bevy::math::DVec2;
use bevy::prelude::*;
use bevy::text::ComputedTextBlock;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::{Ime, PrimaryWindow};
use bevy_md::{BlockText, Markdown};
use bevy_md_core::synthetic;
use bevy_md_editor::{
    ClickSettings, EditMetrics, MarkdownEditor, MarkdownEditorPlugin, PlaceCaret, PointerInput,
};
use common::*;
use parley::{Affinity, Cursor};

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
        // Clicks of the tests are as close in time as the test is slow:
        // make them count as repeated, unless a test says otherwise.
        app.world_mut()
            .resource_mut::<ClickSettings>()
            .multi_click_window = Duration::from_secs(600);
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

    fn metrics(&self) -> &EditMetrics {
        self.app.world().resource::<EditMetrics>()
    }

    fn frame(&mut self) {
        self.app.update();
    }

    fn settings(&mut self) -> Mut<'_, ClickSettings> {
        self.app.world_mut().resource_mut::<ClickSettings>()
    }

    fn caret_at(&mut self, at: usize) {
        let world = self.app.world_mut();
        let (mut editor, markdown) = world
            .query::<(&mut MarkdownEditor, &Markdown)>()
            .single_mut(world)
            .unwrap();
        editor.set_caret(markdown, at);
    }

    /// The selected bytes of the source, or `None` when nothing is selected.
    fn selected(&self) -> Option<&str> {
        let range = self.editor().selection()?;
        Some(&self.markdown().source()[range])
    }

    /// The byte of the source where block `block` starts.
    fn start(&self, block: usize) -> usize {
        self.markdown()
            .document()
            .block(block)
            .unwrap()
            .span()
            .start
    }

    /// Window pixels just inside the character that starts at rendered-text
    /// byte `text` of block `block`: a press there puts the caret before it.
    fn point(&mut self, block: usize, text: usize) -> Vec2 {
        let scale = self.app.world().resource::<UiScale>().0;
        let root = self.markdown().block_entity(block).expect("block is shown");
        let world = self.app.world_mut();
        let mut texts = world.query_filtered::<(
            Entity,
            &ComputedTextBlock,
            &ComputedNode,
            &UiGlobalTransform,
        ), With<BlockText>>();
        let (_, computed, node, transform) = texts
            .iter(world)
            .find(|(entity, ..)| is_under(world, *entity, root))
            .expect("the block has a text");
        let layout = computed.buffer();
        let bounds =
            Cursor::from_byte_index(layout, text, Affinity::Downstream).geometry(layout, 1.0);
        let local = node.content_box().min
            + Vec2::new(
                bounds.x0 as f32 + 1.0,
                f64::midpoint(bounds.y0, bounds.y1) as f32,
            );
        transform.transform_point2(local) * scale
    }

    fn send(&mut self, input: PointerInput) {
        self.app.world_mut().write_message(input);
        self.frame();
    }

    fn press(&mut self, position: Vec2) {
        self.send(PointerInput::Press {
            position,
            shift: false,
        });
    }

    fn shift_press(&mut self, position: Vec2) {
        self.send(PointerInput::Press {
            position,
            shift: true,
        });
    }

    fn drag(&mut self, position: Vec2) {
        self.send(PointerInput::Drag { position });
    }

    fn release(&mut self) {
        self.send(PointerInput::Release);
    }

    /// `count` presses on the same point, one frame each.
    fn click(&mut self, position: Vec2, count: usize) {
        for _ in 0..count {
            self.press(position);
        }
    }
}

fn is_under(world: &World, mut entity: Entity, root: Entity) -> bool {
    loop {
        if entity == root {
            return true;
        }
        match world.get::<ChildOf>(entity) {
            Some(parent) => entity = parent.parent(),
            None => return false,
        }
    }
}

const WORDS: &str = "alpha beta gamma delta\n";
const BLOCKS: &str = "first block\n\nsecond block\n\nthird block\n";

// ---------------------------------------------------------------------------
// Clicks
// ---------------------------------------------------------------------------

#[test]
fn a_single_click_puts_the_caret_under_the_pointer() {
    let mut doc = Open::new(WORDS);
    let at = doc.point(0, 8);
    doc.press(at);
    assert_eq!(doc.editor().caret(), 8);
    assert_eq!(doc.editor().anchor(), 8);
    assert_eq!(doc.selected(), None);
}

#[test]
fn a_double_click_selects_the_word_under_the_pointer() {
    let mut doc = Open::new(WORDS);
    let at = doc.point(0, 7);
    doc.click(at, 2);
    assert_eq!(doc.selected(), Some("beta"));
    assert_eq!((doc.editor().anchor(), doc.editor().caret()), (6, 10));

    // Another word, far from the first click, is a first click again, and
    // the next one a double click on it.
    let at = doc.point(0, 18);
    doc.click(at, 2);
    assert_eq!(doc.selected(), Some("delta"));
}

#[test]
fn a_double_click_on_a_bold_word_selects_the_word_and_not_the_delimiters() {
    let mut doc = Open::new("Some **bold** and *italic* text\n");
    // Rendered text: "Some bold and italic text".
    let at = doc.point(0, 6);
    doc.click(at, 2);
    assert_eq!(doc.selected(), Some("bold"));
    let at = doc.point(0, 16);
    doc.click(at, 2);
    assert_eq!(doc.selected(), Some("italic"));
}

#[test]
fn a_triple_click_selects_the_text_of_the_block() {
    let mut doc = Open::new("# Title here\n\nsecond paragraph\n\n- an item\n");
    let at = doc.point(0, 3);
    doc.click(at, 3);
    assert_eq!(doc.selected(), Some("Title here"), "not the `# `");

    let at = doc.point(1, 4);
    doc.click(at, 3);
    assert_eq!(doc.selected(), Some("second paragraph"));

    let at = doc.point(2, 1);
    doc.click(at, 3);
    assert_eq!(doc.selected(), Some("an item"), "not the `- `");

    // A fourth click in a row keeps the block selected.
    doc.press(at);
    assert_eq!(doc.selected(), Some("an item"));
}

#[test]
fn a_triple_click_selects_all_the_lines_of_a_wrapped_block() {
    let text = vec!["lorem"; 400].join(" ");
    let mut doc = Open::new(&format!("{text}\n"));
    let (first, last) = (doc.point(0, 0), doc.point(0, text.len() - 2));
    assert!(last.y > first.y, "the paragraph wraps");
    let at = doc.point(0, 700);
    doc.click(at, 3);
    assert_eq!(doc.selected(), Some(text.as_str()));
}

#[test]
fn clicks_that_are_slow_or_apart_do_not_add_up() {
    let mut doc = Open::new(WORDS);
    let at = doc.point(0, 7);

    // Too slow: with no time window, every click is the first.
    doc.settings().multi_click_window = Duration::ZERO;
    doc.click(at, 3);
    assert_eq!(doc.selected(), None);
    assert_eq!(doc.editor().caret(), 7);

    // Close in time but not in place.
    doc.settings().multi_click_window = Duration::from_secs(600);
    doc.press(at);
    let apart = doc.point(0, 18);
    doc.press(apart);
    assert_eq!(doc.selected(), None, "the second click was somewhere else");
    assert_eq!(doc.editor().caret(), 18);
    // ...and it starts a run of its own.
    doc.press(apart);
    assert_eq!(doc.selected(), Some("delta"));
}

#[test]
fn the_default_click_settings_are_the_usual_ones() {
    let settings = ClickSettings::default();
    assert!(
        (Duration::from_millis(300)..=Duration::from_millis(600))
            .contains(&settings.multi_click_window)
    );
    assert!(settings.multi_click_distance > 0.0);
}

#[test]
fn a_click_that_is_over_no_text_changes_nothing() {
    let mut doc = Open::new(WORDS);
    doc.caret_at(3);
    // Far outside the node of the document.
    doc.press(Vec2::new(-500.0, -500.0));
    assert_eq!(doc.editor().caret(), 3);
    assert_eq!(doc.metrics().clicks_ignored, 0);
}

#[test]
fn only_the_block_under_the_pointer_is_clicked() {
    let mut doc = Open::new(BLOCKS);
    let at = doc.point(1, 3);
    doc.click(at, 2);
    assert_eq!(doc.selected(), Some("second"));
    assert_eq!(doc.start(1), doc.editor().selection().unwrap().start);
}

#[test]
fn place_caret_still_puts_the_caret_and_drops_the_selection() {
    let mut doc = Open::new(WORDS);
    let at = doc.point(0, 7);
    doc.click(at, 2);
    assert_eq!(doc.selected(), Some("beta"));
    let at = doc.point(0, 12);
    doc.app
        .world_mut()
        .write_message(PlaceCaret { position: at });
    doc.frame();
    assert_eq!(doc.editor().caret(), 12);
    assert_eq!(doc.selected(), None);
}

#[test]
fn the_ui_scale_is_respected() {
    let mut doc = Open::new(BLOCKS);
    doc.app.insert_resource(UiScale(2.0));
    settle(&mut doc.app);
    let at = doc.point(1, 7);
    doc.click(at, 2);
    assert_eq!(doc.selected(), Some("block"));
}

#[test]
fn clicks_during_a_parse_are_discarded_and_counted() {
    let mut doc = Open::new("one\n\ntwo\n");
    doc.caret_at(3);
    let at = doc.point(1, 1);
    doc.markdown_mut().set_source(&synthetic::source(500));
    assert!(doc.markdown().is_parsing());
    for _ in 0..2 {
        doc.app.world_mut().write_message(PointerInput::Press {
            position: at,
            shift: false,
        });
        doc.app.world_mut().run_schedule(Update);
    }
    assert_eq!(doc.metrics().clicks_ignored, 2);
    assert_eq!(doc.editor().caret(), 3, "the caret stayed");
    assert_eq!(doc.selected(), None);
    // A drag has no press to follow.
    doc.app
        .world_mut()
        .write_message(PointerInput::Drag { position: at });
    doc.app.world_mut().run_schedule(Update);
    assert_eq!(doc.selected(), None);
    settle(&mut doc.app);
    assert_eq!(doc.metrics().clicks_ignored, 2);
}

impl Open {
    fn markdown_mut(&mut self) -> Mut<'_, Markdown> {
        let entity = self.markdown;
        self.app.world_mut().get_mut::<Markdown>(entity).unwrap()
    }
}

// ---------------------------------------------------------------------------
// Dragging
// ---------------------------------------------------------------------------

#[test]
fn dragging_extends_the_selection_from_the_press_to_the_pointer() {
    let mut doc = Open::new(WORDS);
    let (from, to, back) = (doc.point(0, 6), doc.point(0, 16), doc.point(0, 2));
    doc.press(from);
    assert_eq!(doc.selected(), None);

    doc.drag(to);
    assert_eq!(doc.selected(), Some("beta gamma"));
    assert_eq!((doc.editor().anchor(), doc.editor().caret()), (6, 16));

    // Past the press the other way: the selection is backwards.
    doc.drag(back);
    assert_eq!(doc.selected(), Some("pha "));
    assert_eq!((doc.editor().anchor(), doc.editor().caret()), (6, 2));

    // Back on the press: nothing selected.
    doc.drag(from);
    assert_eq!(doc.selected(), None);
    assert_eq!(doc.editor().caret(), 6);
}

#[test]
fn releasing_ends_the_drag() {
    let mut doc = Open::new(WORDS);
    let (from, to, elsewhere) = (doc.point(0, 6), doc.point(0, 16), doc.point(0, 1));
    doc.press(from);
    doc.drag(to);
    doc.release();
    assert_eq!(doc.selected(), Some("beta gamma"));
    doc.drag(elsewhere);
    assert_eq!(doc.selected(), Some("beta gamma"), "the button is up");
}

#[test]
fn dragging_into_another_block_selects_across_blocks() {
    let mut doc = Open::new(BLOCKS);
    let (from, to) = (doc.point(0, 6), doc.point(2, 5));
    doc.press(from);
    doc.drag(to);
    let range = doc.editor().selection().unwrap();
    assert_eq!(range, 6..doc.start(2) + 5);
    assert_eq!(doc.selected(), Some("block\n\nsecond block\n\nthird"));
    assert_eq!(doc.editor().anchor(), 6);

    // And back, through the middle one, to the start of the document.
    let middle = doc.point(1, 3);
    doc.drag(middle);
    assert_eq!(doc.editor().selection().unwrap(), 6..doc.start(1) + 3);
    let before = doc.point(0, 2);
    doc.drag(before);
    assert_eq!(doc.editor().selection().unwrap(), 2..6);
    assert_eq!(doc.editor().caret(), 2);
}

#[test]
fn dragging_out_of_the_text_goes_to_the_nearest_block() {
    let mut doc = Open::new(BLOCKS);
    let from = doc.point(1, 3);
    doc.press(from);
    let anchor = doc.start(1) + 3;

    // Above everything, left of the text: the start of the first block.
    doc.drag(Vec2::new(2.0, 1.0));
    assert_eq!(doc.editor().anchor(), anchor);
    assert_eq!(doc.editor().caret(), 0);

    // Below everything, right of the text: the end of the last block's text.
    doc.drag(Vec2::new(900.0, 710.0));
    assert_eq!(doc.editor().caret(), doc.start(2) + "third block".len());
    assert_eq!(doc.editor().anchor(), anchor);
}

#[test]
fn a_drag_after_a_double_click_grows_by_words() {
    let mut doc = Open::new(WORDS);
    let (word, later, earlier) = (doc.point(0, 7), doc.point(0, 19), doc.point(0, 1));
    doc.click(word, 2);
    assert_eq!(doc.selected(), Some("beta"));

    doc.drag(later);
    assert_eq!(doc.selected(), Some("beta gamma delta"));
    assert_eq!((doc.editor().anchor(), doc.editor().caret()), (6, 22));

    doc.drag(earlier);
    assert_eq!(doc.selected(), Some("alpha beta"));
    assert_eq!((doc.editor().anchor(), doc.editor().caret()), (10, 0));

    doc.drag(word);
    assert_eq!(doc.selected(), Some("beta"));
}

#[test]
fn a_drag_after_a_triple_click_grows_by_blocks() {
    let mut doc = Open::new("one one\n\ntwo two\n\nthree three\n");
    let middle = doc.point(1, 2);
    doc.click(middle, 3);
    assert_eq!(doc.selected(), Some("two two"));

    let (below, above) = (doc.point(2, 2), doc.point(0, 2));
    doc.drag(below);
    assert_eq!(
        doc.editor().selection().unwrap(),
        doc.start(1)..doc.start(2) + "three three".len()
    );
    doc.drag(above);
    assert_eq!(
        doc.editor().selection().unwrap(),
        0..doc.start(1) + "two two".len()
    );
    assert_eq!(doc.editor().caret(), 0, "the selection was made backwards");
}

#[test]
fn a_drag_during_a_parse_is_dropped_and_does_not_move_the_selection() {
    let mut doc = Open::new(WORDS);
    let (from, to) = (doc.point(0, 6), doc.point(0, 16));
    doc.press(from);
    doc.drag(to);
    assert_eq!(doc.selected(), Some("beta gamma"));
    doc.markdown_mut().set_source(&synthetic::source(500));
    doc.app
        .world_mut()
        .write_message(PointerInput::Drag { position: from });
    doc.app.world_mut().run_schedule(Update);
    assert_eq!(doc.editor().selection(), Some(6..16), "left as it was");
}

// ---------------------------------------------------------------------------
// Shift+click
// ---------------------------------------------------------------------------

#[test]
fn shift_click_extends_the_selection_from_the_anchor() {
    let mut doc = Open::new(WORDS);
    doc.caret_at(2);
    let (right, left) = (doc.point(0, 10), doc.point(0, 0));
    doc.shift_press(right);
    assert_eq!(doc.selected(), Some("pha beta"));
    assert_eq!((doc.editor().anchor(), doc.editor().caret()), (2, 10));

    // The anchor stays: the next one moves only the caret.
    doc.shift_press(left);
    assert_eq!(doc.selected(), Some("al"));
    assert_eq!((doc.editor().anchor(), doc.editor().caret()), (2, 0));

    // A plain click starts over.
    doc.press(right);
    assert_eq!(doc.selected(), None);
    assert_eq!(doc.editor().caret(), 10);
}

#[test]
fn shift_click_in_another_block_selects_across_blocks() {
    let mut doc = Open::new(BLOCKS);
    let from = doc.point(0, 6);
    doc.press(from);
    let to = doc.point(2, 5);
    doc.shift_press(to);
    assert_eq!(doc.editor().selection().unwrap(), 6..doc.start(2) + 5);
}

#[test]
fn shift_click_extends_a_selection_made_by_a_double_click() {
    let mut doc = Open::new(WORDS);
    let (word, later) = (doc.point(0, 7), doc.point(0, 14));
    doc.click(word, 2);
    assert_eq!(doc.editor().anchor(), 6);
    doc.shift_press(later);
    assert_eq!(doc.selected(), Some("beta gam"));
}

#[test]
fn shift_click_then_drag_keeps_extending_from_the_anchor() {
    let mut doc = Open::new(WORDS);
    doc.caret_at(6);
    let (first, second) = (doc.point(0, 11), doc.point(0, 17));
    doc.shift_press(first);
    assert_eq!(doc.selected(), Some("beta "));
    doc.drag(second);
    assert_eq!(doc.selected(), Some("beta gamma "));
}

// ---------------------------------------------------------------------------
// The real mouse
// ---------------------------------------------------------------------------

impl Open {
    fn cursor_to(&mut self, position: Vec2) {
        let world = self.app.world_mut();
        let mut window = world
            .query_filtered::<&mut Window, With<PrimaryWindow>>()
            .single_mut(world)
            .unwrap();
        window.set_physical_cursor_position(Some(DVec2::new(
            f64::from(position.x),
            f64::from(position.y),
        )));
    }

    fn button(&mut self, state: ButtonState) {
        self.app.world_mut().write_message(MouseButtonInput {
            button: MouseButton::Left,
            state,
            window: Entity::PLACEHOLDER,
        });
        self.frame();
    }

    fn shift(&mut self, state: ButtonState) {
        self.app.world_mut().write_message(KeyboardInput {
            key_code: KeyCode::ShiftLeft,
            logical_key: Key::Shift,
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        self.frame();
    }
}

#[test]
fn the_mouse_clicks_double_clicks_and_drags() {
    let mut doc = Open::with_window(WORDS);
    let (word, later, elsewhere) = (doc.point(0, 7), doc.point(0, 19), doc.point(0, 1));

    // A press is a click.
    doc.cursor_to(word);
    doc.frame();
    doc.button(ButtonState::Pressed);
    assert_eq!(doc.editor().caret(), 7);
    assert_eq!(doc.selected(), None);
    doc.button(ButtonState::Released);

    // A second press on the same point is a double click.
    doc.button(ButtonState::Pressed);
    assert_eq!(doc.selected(), Some("beta"));

    // Moving the cursor with the button down drags, by words.
    doc.cursor_to(later);
    doc.frame();
    assert_eq!(doc.selected(), Some("beta gamma delta"));

    // Releasing ends it: moving the cursor does nothing now.
    doc.button(ButtonState::Released);
    doc.cursor_to(elsewhere);
    doc.frame();
    assert_eq!(doc.selected(), Some("beta gamma delta"));
}

#[test]
fn the_mouse_with_shift_down_extends_the_selection() {
    let mut doc = Open::with_window(WORDS);
    doc.caret_at(2);
    let at = doc.point(0, 10);
    doc.cursor_to(at);
    doc.frame();
    doc.shift(ButtonState::Pressed);
    doc.button(ButtonState::Pressed);
    assert_eq!(doc.selected(), Some("pha beta"));
    doc.button(ButtonState::Released);
    doc.shift(ButtonState::Released);

    // Without Shift it is a click again.
    doc.button(ButtonState::Pressed);
    assert_eq!(doc.selected(), None);
    assert_eq!(doc.editor().caret(), 10);
}

// ---------------------------------------------------------------------------
// A click anywhere on the line, the mouse cursor, the caret between blocks
// ---------------------------------------------------------------------------

impl Open {
    /// The window pixels of the document's node: `(left, top, right, bottom)`.
    fn area(&mut self) -> (f32, f32, f32, f32) {
        let markdown = self.markdown;
        let world = self.app.world();
        let node = world.get::<ComputedNode>(markdown).unwrap();
        let transform = world.get::<UiGlobalTransform>(markdown).unwrap();
        let (c, half) = (transform.translation, node.size() / 2.0);
        (c.x - half.x, c.y - half.y, c.x + half.x, c.y + half.y)
    }

    /// The window pixels of the box of block `block`: `(top, bottom)`.
    fn block_rows(&mut self, block: usize) -> (f32, f32) {
        let root = self.markdown().block_entity(block).expect("block is shown");
        let world = self.app.world();
        let node = world.get::<ComputedNode>(root).unwrap();
        let transform = world.get::<UiGlobalTransform>(root).unwrap();
        let (c, half) = (transform.translation.y, node.size().y / 2.0);
        (c - half, c + half)
    }

    fn window_icon(&mut self) -> Option<bevy::window::CursorIcon> {
        let world = self.app.world_mut();
        let window = world
            .query_filtered::<Entity, With<PrimaryWindow>>()
            .single(world)
            .unwrap();
        world.get::<bevy::window::CursorIcon>(window).cloned()
    }

    fn key_press(&mut self, key_code: KeyCode, logical_key: Key, text: Option<&str>) {
        self.app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key,
            state: ButtonState::Pressed,
            text: text.map(Into::into),
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
        self.frame();
        self.frame();
    }
}

#[test]
fn a_click_to_the_right_of_a_line_puts_the_caret_at_its_end() {
    let mut doc = Open::new(WORDS);
    let (_, _, right, _) = doc.area();
    let row = doc.point(0, 3).y;
    doc.press(Vec2::new(right - 2.0, row));
    assert_eq!(doc.editor().caret(), WORDS.trim_end().len());
    assert_eq!(doc.selected(), None);
}

#[test]
fn a_click_to_the_left_of_a_line_puts_the_caret_at_its_start() {
    let mut doc = Open::new(WORDS);
    doc.caret_at(9);
    let (left, ..) = doc.area();
    let row = doc.point(0, 3).y;
    doc.press(Vec2::new(left + 1.0, row));
    assert_eq!(doc.editor().caret(), 0);
}

#[test]
fn a_click_between_two_blocks_goes_to_the_nearest_one() {
    let mut doc = Open::new(BLOCKS);
    let x = doc.point(0, 3).x;
    let (_, first_bottom) = doc.block_rows(0);
    let (second_top, _) = doc.block_rows(1);
    assert!(second_top > first_bottom, "the blocks have space between");
    // Just under the first block, and just over the second.
    doc.press(Vec2::new(x, first_bottom + 0.5));
    assert_eq!(doc.editor().caret(), 3);
    doc.press(Vec2::new(x, second_top - 0.5));
    assert_eq!(doc.editor().caret(), doc.start(1) + 3);
}

#[test]
fn a_click_below_the_last_block_goes_to_the_end_of_the_document() {
    let mut doc = Open::new(BLOCKS);
    let x = doc.point(2, 3).x;
    let (_, _, _, bottom) = doc.area();
    doc.press(Vec2::new(x, bottom - 0.5));
    assert!(doc.editor().caret() >= doc.start(2));
    assert!(doc.editor().caret() <= BLOCKS.trim_end().len());
}

#[test]
fn a_double_click_to_the_right_of_a_line_still_selects_a_word() {
    let mut doc = Open::new(WORDS);
    let (_, _, right, _) = doc.area();
    let row = doc.point(0, 3).y;
    doc.click(Vec2::new(right - 2.0, row), 2);
    // The nearest glyphs to that point are the last word.
    assert_eq!(doc.selected(), Some("delta"));
}

const WITH_LINK: &str = "see [the docs](https://bevy.org) here\n\nsecond\n";

#[test]
fn the_mouse_is_an_i_beam_over_the_document_and_the_default_outside() {
    use bevy::window::{CursorIcon, SystemCursorIcon};
    let mut doc = Open::with_window(WITH_LINK);
    // On the text.
    let on_text = doc.point(0, 1);
    doc.cursor_to(on_text);
    doc.frame();
    assert_eq!(
        doc.window_icon(),
        Some(CursorIcon::System(SystemCursorIcon::Text))
    );
    // On the empty part of the line, still a place to click.
    let (_, _, right, _) = doc.area();
    doc.cursor_to(Vec2::new(right - 2.0, on_text.y));
    doc.frame();
    assert_eq!(
        doc.window_icon(),
        Some(CursorIcon::System(SystemCursorIcon::Text))
    );
    // Outside the document: the window's own.
    doc.cursor_to(Vec2::new(-500.0, -500.0));
    doc.frame();
    assert_eq!(doc.window_icon(), Some(CursorIcon::default()));
}

#[test]
fn the_mouse_is_a_hand_over_a_link_and_only_over_it() {
    use bevy::window::{CursorIcon, SystemCursorIcon};
    let mut doc = Open::with_window(WITH_LINK);
    // "see " is 4 bytes, the link text is "the docs".
    let on_link = doc.point(0, 6);
    doc.cursor_to(on_link);
    doc.frame();
    assert_eq!(
        doc.window_icon(),
        Some(CursorIcon::System(SystemCursorIcon::Pointer))
    );
    // The word after the link, and the empty part of the line.
    let after = doc.point(0, "see the docs here".len() - 1);
    doc.cursor_to(after);
    doc.frame();
    assert_eq!(
        doc.window_icon(),
        Some(CursorIcon::System(SystemCursorIcon::Text))
    );
    let (_, _, right, _) = doc.area();
    doc.cursor_to(Vec2::new(right - 2.0, on_link.y));
    doc.frame();
    assert_eq!(
        doc.window_icon(),
        Some(CursorIcon::System(SystemCursorIcon::Text))
    );
}

fn gap_caret(doc: &mut Open) -> Option<(f32, f32, f32, Visibility)> {
    use bevy_md_editor::GapCaret;
    let world = doc.app.world_mut();
    let mut query = world.query_filtered::<(&Node, &Visibility), With<GapCaret>>();
    let (node, visibility) = query.iter(world).next()?;
    let (Val::Px(left), Val::Px(top), Val::Px(height)) = (node.left, node.top, node.height) else {
        panic!("the gap caret is placed in pixels");
    };
    Some((left, top, height, *visibility))
}

#[test]
fn enter_at_the_end_of_a_block_shows_a_caret_between_the_blocks() {
    let mut doc = Open::with_window("first\n\nsecond\n");
    assert!(gap_caret(&mut doc).is_none(), "nothing to show in a block");

    doc.caret_at(5);
    doc.key_press(KeyCode::Enter, Key::Enter, Some("\r"));
    let (_, first_bottom) = doc.block_rows(0);
    let (second_top, _) = doc.block_rows(1);
    let (left, top, height, visibility) = gap_caret(&mut doc).expect("the gap caret exists");
    assert_eq!(visibility, Visibility::Visible);
    assert!(height > 0.0);
    // The caret is in the gap, between the two blocks.
    let middle = top + height / 2.0;
    assert!(
        (first_bottom..=second_top).contains(&middle),
        "{middle} is not between {first_bottom} and {second_top}"
    );
    let (area_left, ..) = doc.area();
    assert!(left >= area_left, "at the left edge of the text");

    // Back in a block, Bevy draws the caret and the gap one is hidden.
    doc.caret_at(2);
    doc.frame();
    doc.frame();
    let (.., visibility) = gap_caret(&mut doc).expect("the node is kept");
    assert_eq!(visibility, Visibility::Hidden);
}
