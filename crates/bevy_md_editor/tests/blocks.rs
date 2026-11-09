//! Enter, Backspace and Delete at the edge of a block, and the word
//! shortcuts: what the source says afterwards, where the caret is, what the
//! blocks are, that the document is what a full parse gives, and that undo
//! and redo take the source back and forth byte for byte.

#[path = "../../bevy_md/tests/common/mod.rs"]
mod common;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::window::Ime;
use bevy_md::Markdown;
use bevy_md_core::{BlockKind, Document, ListMarker, Style, synthetic};
use bevy_md_editor::{MarkdownEditor, MarkdownEditorPlugin};
use common::*;

struct Open {
    app: App,
    area: Entity,
    markdown: Entity,
}

impl Open {
    /// Open `marked`, a source with a `|` where the caret is.
    fn new(marked: &str) -> Self {
        let at = marked.find('|').expect("a | for the caret");
        Self::with_caret(&marked.replacen('|', "", 1), at)
    }

    /// Open `source` with the caret at byte `at`.
    fn with_caret(source: &str, at: usize) -> Self {
        let mut app = headless_app();
        app.update();
        app.add_message::<Ime>();
        app.add_plugins(MarkdownEditorPlugin);
        let (area, markdown) = spawn_document(&mut app, source);
        app.world_mut()
            .entity_mut(markdown)
            .insert(MarkdownEditor::new());
        settle(&mut app);
        let mut open = Self {
            app,
            area,
            markdown,
        };
        open.caret_at(at);
        open.settle();
        open
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

    /// The source with a `|` where the caret is.
    fn marked(&self) -> String {
        let mut source = self.source();
        source.insert(self.editor().caret(), '|');
        source
    }

    fn caret_at(&mut self, at: usize) {
        let world = self.app.world_mut();
        let (mut editor, markdown) = world
            .query::<(&mut MarkdownEditor, &Markdown)>()
            .single_mut(world)
            .unwrap();
        editor.set_caret(markdown, at);
    }

    fn set_source(&mut self, source: &str) {
        let entity = self.markdown;
        self.app
            .world_mut()
            .get_mut::<Markdown>(entity)
            .unwrap()
            .set_source(source);
    }

    fn key_event(&mut self, key_code: KeyCode, logical_key: Key, state: ButtonState) {
        self.app.world_mut().write_message(KeyboardInput {
            key_code,
            logical_key,
            state,
            text: None,
            repeat: false,
            window: Entity::PLACEHOLDER,
        });
    }

    /// Write a key press and release. No frame runs.
    fn press_events(&mut self, key_code: KeyCode, key: Key) {
        self.key_event(key_code, key.clone(), ButtonState::Pressed);
        self.key_event(key_code, key, ButtonState::Released);
    }

    /// Write a key pressed with the Control key down. No frame runs.
    fn ctrl_events(&mut self, key_code: KeyCode, key: Key) {
        self.key_event(KeyCode::ControlLeft, Key::Control, ButtonState::Pressed);
        self.press_events(key_code, key);
        self.key_event(KeyCode::ControlLeft, Key::Control, ButtonState::Released);
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

    fn settle(&mut self) {
        settle(&mut self.app);
    }

    /// A key, applied.
    fn press(&mut self, key_code: KeyCode, key: Key) {
        self.press_events(key_code, key);
        self.settle();
    }

    fn enter(&mut self) {
        self.press(KeyCode::Enter, Key::Enter);
    }

    fn backspace(&mut self) {
        self.press(KeyCode::Backspace, Key::Backspace);
    }

    fn delete(&mut self) {
        self.press(KeyCode::Delete, Key::Delete);
    }

    fn type_text(&mut self, text: &str) {
        self.type_events(text);
        self.settle();
    }

    fn ctrl(&mut self, key_code: KeyCode, key: Key) {
        self.ctrl_events(key_code, key);
        self.settle();
    }

    fn word_left(&mut self) {
        self.ctrl(KeyCode::ArrowLeft, Key::ArrowLeft);
    }

    fn word_right(&mut self) {
        self.ctrl(KeyCode::ArrowRight, Key::ArrowRight);
    }

    fn word_backspace(&mut self) {
        self.ctrl(KeyCode::Backspace, Key::Backspace);
    }

    fn word_delete(&mut self) {
        self.ctrl(KeyCode::Delete, Key::Delete);
    }

    fn undo(&mut self) {
        self.key_event(KeyCode::ControlLeft, Key::Control, ButtonState::Pressed);
        self.press_events(KeyCode::KeyZ, Key::Character("z".into()));
        self.key_event(KeyCode::ControlLeft, Key::Control, ButtonState::Released);
        self.settle();
    }

    fn redo(&mut self) {
        self.key_event(KeyCode::ControlLeft, Key::Control, ButtonState::Pressed);
        self.key_event(KeyCode::ShiftLeft, Key::Shift, ButtonState::Pressed);
        self.press_events(KeyCode::KeyZ, Key::Character("Z".into()));
        self.key_event(KeyCode::ShiftLeft, Key::Shift, ButtonState::Released);
        self.key_event(KeyCode::ControlLeft, Key::Control, ButtonState::Released);
        self.settle();
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

    /// The text of each block.
    fn texts(&self) -> Vec<String> {
        self.markdown()
            .document()
            .blocks()
            .map(|block| block.content.text.clone())
            .collect()
    }

    fn kinds(&self) -> Vec<BlockKind> {
        self.markdown()
            .document()
            .blocks()
            .map(|block| block.kind.clone())
            .collect()
    }

    /// The caret is inside a block, and the block it is in is the focused
    /// one.
    fn assert_caret_is_in_a_block(&self) -> usize {
        self.markdown()
            .document()
            .block_at(self.editor().caret())
            .expect("the caret is in a block")
    }

    /// What `act` did, as `(before, after)` with the caret marked, then
    /// checks the document is a full parse and that undo and redo go back and
    /// forth byte for byte, caret included.
    fn does(&mut self, act: impl FnOnce(&mut Self)) -> (String, String) {
        let before = self.marked();
        let steps = self.editor().history().undo_len();
        act(self);
        let after = self.marked();
        self.assert_is_a_full_parse();
        if before != after {
            assert_eq!(
                self.editor().history().undo_len(),
                steps + 1,
                "one step of the history"
            );
            self.undo();
            assert_eq!(self.marked(), before, "undo restores the source");
            self.assert_is_a_full_parse();
            self.redo();
            assert_eq!(self.marked(), after, "redo does it again");
            self.assert_is_a_full_parse();
        }
        (before, after)
    }

    /// Run `act`; the source with the caret must be `expected` and undo must
    /// give the original back.
    fn check(&mut self, act: impl FnOnce(&mut Self), expected: &str) {
        let (_, after) = self.does(act);
        assert_eq!(after, expected);
    }
}

fn open(marked: &str) -> Open {
    Open::new(marked)
}

fn paragraph() -> BlockKind {
    BlockKind::Paragraph
}

fn bullet() -> BlockKind {
    BlockKind::ListItem {
        marker: ListMarker::Bullet,
        task: None,
    }
}

fn ordered(number: u64) -> BlockKind {
    BlockKind::ListItem {
        marker: ListMarker::Ordered(number),
        task: None,
    }
}

// ---------------------------------------------------------------------------
// Enter: paragraphs and headings
// ---------------------------------------------------------------------------

#[test]
fn enter_splits_a_paragraph_at_the_caret() {
    let _serial = serial();
    let mut doc = open("hel|lo world\n");
    doc.check(Open::enter, "hel\n\n|lo world\n");
    assert_eq!(doc.texts(), ["hel", "lo world"]);
    assert_eq!(doc.kinds(), [paragraph(), paragraph()]);
    doc.assert_caret_is_in_a_block();
}

#[test]
fn what_is_typed_after_enter_goes_into_the_new_block() {
    let _serial = serial();
    let mut doc = open("hel|lo\n");
    doc.enter();
    doc.type_text("X");
    assert_eq!(doc.source(), "hel\n\nXlo\n");
    assert_eq!(doc.texts(), ["hel", "Xlo"]);
    doc.assert_is_a_full_parse();
}

#[test]
fn enter_between_two_words_keeps_every_byte() {
    let _serial = serial();
    let mut doc = open("one |two\n");
    doc.check(Open::enter, "one \n\n|two\n");
    assert_eq!(doc.texts(), ["one", "two"]);
}

#[test]
fn enter_at_the_end_of_a_paragraph_leaves_a_blank_line_and_backspace_takes_it_back() {
    let _serial = serial();
    let mut doc = open("hello|\n");
    doc.check(Open::enter, "hello\n\n|\n");
    assert_eq!(doc.texts(), ["hello"], "an empty paragraph is no block");

    // The caret is between blocks, and typing there starts the next one.
    doc.type_text("x");
    assert_eq!(doc.source(), "hello\n\nx\n");
    assert_eq!(doc.texts(), ["hello", "x"]);
    doc.undo();
    assert_eq!(doc.marked(), "hello\n\n|\n");

    doc.check(Open::backspace, "hello|\n");
    doc.check(Open::enter, "hello\n\n|\n");
}

#[test]
fn enter_at_the_end_of_a_paragraph_before_another_does_not_land_in_it() {
    let _serial = serial();
    let mut doc = open("one|\n\ntwo\n");
    doc.enter();
    assert_eq!(doc.source(), "one\n\n\n\ntwo\n");
    assert_eq!(doc.editor().caret(), 5);
    assert_eq!(doc.markdown().document().block_at(5), None);
    doc.assert_is_a_full_parse();
}

#[test]
fn a_second_enter_on_the_blank_line_does_nothing() {
    let _serial = serial();
    let mut doc = open("hello|\n");
    doc.enter();
    let after = doc.source();
    doc.check(Open::enter, "hello\n\n|\n");
    assert_eq!(doc.source(), after);
}

#[test]
fn enter_at_the_start_of_a_paragraph_or_heading_does_nothing() {
    let _serial = serial();
    for marked in ["|hello\n", "# |Title\n", "para\n\n|second\n"] {
        let mut doc = open(marked);
        doc.check(Open::enter, marked);
        assert_eq!(doc.editor().history().undo_len(), 0, "{marked:?}");
    }
}

#[test]
fn enter_in_a_heading_makes_the_rest_a_paragraph() {
    let _serial = serial();
    let mut doc = open("# Ti|tle\n");
    doc.check(Open::enter, "# Ti\n\n|tle\n");
    assert_eq!(doc.kinds(), [BlockKind::Heading(1), paragraph()]);
    assert_eq!(doc.texts(), ["Ti", "tle"]);
}

#[test]
fn enter_at_the_end_of_a_heading_goes_to_a_new_line_below() {
    let _serial = serial();
    let mut doc = open("## Title|\n\ntext\n");
    doc.enter();
    doc.type_text("new");
    assert_eq!(doc.source(), "## Title\n\nnew\n\ntext\n");
    assert_eq!(
        doc.kinds(),
        [BlockKind::Heading(2), paragraph(), paragraph()]
    );
}

#[test]
fn enter_in_the_middle_of_a_text_between_other_blocks_moves_only_what_follows() {
    let _serial = serial();
    let mut doc = open("first\n\nsec|ond\n\nthird\n");
    doc.check(Open::enter, "first\n\nsec\n\n|ond\n\nthird\n");
    assert_eq!(doc.texts(), ["first", "sec", "ond", "third"]);
}

// ---------------------------------------------------------------------------
// Enter: styled text
// ---------------------------------------------------------------------------

#[test]
fn enter_inside_bold_closes_it_and_opens_it_again() {
    let _serial = serial();
    let mut doc = open("**bo|ld**\n");
    doc.check(Open::enter, "**bo**\n\n**|ld**\n");
    assert_eq!(doc.texts(), ["bo", "ld"]);
    let document = doc.markdown().document();
    for index in 0..2 {
        assert_eq!(
            document.block(index).unwrap().content.runs[0].style,
            Style::BOLD
        );
    }
}

#[test]
fn enter_inside_italics_and_code_keeps_the_style_on_both_sides() {
    let _serial = serial();
    let mut doc = open("a *it|alic* b\n");
    doc.check(Open::enter, "a *it*\n\n*|alic* b\n");
    assert_eq!(doc.texts(), ["a it", "alic b"]);

    let mut doc = open("`co|de`\n");
    doc.check(Open::enter, "`co`\n\n`|de`\n");
    let document = doc.markdown().document();
    assert_eq!(
        document.block(1).unwrap().content.runs[0].style,
        Style::CODE
    );
}

#[test]
fn enter_inside_bold_and_italic_reopens_both() {
    let _serial = serial();
    let mut doc = open("***bo|ld***\n");
    doc.check(Open::enter, "***bo***\n\n***|ld***\n");
    assert_eq!(doc.texts(), ["bo", "ld"]);
}

#[test]
fn enter_at_the_end_of_bold_text_cuts_after_the_closing_delimiters() {
    let _serial = serial();
    // The caret at the end of `bold` is before the closing `**` in the
    // source; the cut goes after it.
    let mut doc = open("plain **bold|** rest\n");
    doc.check(Open::enter, "plain **bold**\n\n |rest\n");
    assert_eq!(doc.texts(), ["plain bold", "rest"]);
}

#[test]
fn enter_at_the_start_of_bold_text_cuts_before_the_opening_delimiters() {
    let _serial = serial();
    let mut doc = open("plain |**bold** rest\n");
    // Between "plain " and "bold" the caret is at the end of the plain run.
    doc.check(Open::enter, "plain \n\n|**bold** rest\n");
    assert_eq!(doc.texts(), ["plain", "bold rest"]);
}

#[test]
fn enter_at_the_very_end_of_bold_text_cuts_after_the_closing_delimiters() {
    let _serial = serial();
    let mut doc = open("**bold|**\n");
    doc.enter();
    assert_eq!(doc.source(), "**bold**\n\n\n");
    assert_eq!(doc.editor().caret(), 10);
    doc.assert_is_a_full_parse();
}

// ---------------------------------------------------------------------------
// Enter: lists
// ---------------------------------------------------------------------------

#[test]
fn enter_in_a_list_item_makes_a_new_item_with_the_same_marker() {
    let _serial = serial();
    let mut doc = open("- on|e\n- two\n");
    doc.check(Open::enter, "- on\n- |e\n- two\n");
    assert_eq!(doc.texts(), ["on", "e", "two"]);
    assert_eq!(doc.kinds(), [bullet(), bullet(), bullet()]);
    doc.assert_caret_is_in_a_block();
}

#[test]
fn enter_at_the_end_of_an_item_makes_an_empty_item_that_shows_its_bullet() {
    let _serial = serial();
    let mut doc = open("- one|\n- two\n");
    doc.check(Open::enter, "- one\n- |\n- two\n");
    assert_eq!(doc.texts(), ["one", "", "two"]);
    assert_eq!(doc.kinds(), [bullet(), bullet(), bullet()]);
    assert_eq!(doc.assert_caret_is_in_a_block(), 1);
    doc.type_text("new");
    assert_eq!(doc.source(), "- one\n- new\n- two\n");
}

#[test]
fn enter_at_the_start_of_an_item_makes_an_empty_item_above_it() {
    let _serial = serial();
    let mut doc = open("- |one\n");
    doc.check(Open::enter, "- \n- |one\n");
    assert_eq!(doc.texts(), ["", "one"]);
}

#[test]
fn enter_in_a_star_and_a_plus_list_keeps_the_symbol() {
    let _serial = serial();
    let mut doc = open("* a|\n");
    doc.check(Open::enter, "* a\n* |\n");
    let mut doc = open("+ a|\n");
    doc.check(Open::enter, "+ a\n+ |\n");
}

#[test]
fn enter_in_an_ordered_list_numbers_the_next_item() {
    let _serial = serial();
    let mut doc = open("1. a|\n2. b\n");
    doc.check(Open::enter, "1. a\n2. |\n2. b\n");
    assert_eq!(doc.kinds(), [ordered(1), ordered(2), ordered(3)]);

    let mut doc = open("9. nine|\n");
    doc.check(Open::enter, "9. nine\n10. |\n");
    assert_eq!(doc.kinds(), [ordered(9), ordered(10)]);

    let mut doc = open("3) c|\n");
    doc.check(Open::enter, "3) c\n4) |\n");
}

#[test]
fn enter_in_a_task_makes_an_unchecked_task() {
    let _serial = serial();
    let mut doc = open("- [x] done|\n");
    doc.check(Open::enter, "- [x] done\n- [ ] |\n");
    assert_eq!(
        doc.kinds()[1],
        BlockKind::ListItem {
            marker: ListMarker::Bullet,
            task: Some(false),
        }
    );
    assert_eq!(doc.kinds()[0], {
        BlockKind::ListItem {
            marker: ListMarker::Bullet,
            task: Some(true),
        }
    });
}

#[test]
fn enter_in_a_nested_item_keeps_the_nesting() {
    let _serial = serial();
    let mut doc = open("- a\n  - n|\n");
    doc.check(Open::enter, "- a\n  - n\n  - |\n");
    let document = doc.markdown().document();
    assert_eq!(document.block(2).unwrap().list_depth, 2);
}

#[test]
fn enter_in_a_list_inside_a_quote_stays_in_the_quote() {
    let _serial = serial();
    let mut doc = open("> - a|\n");
    doc.check(Open::enter, "> - a\n> - |\n");
    let document = doc.markdown().document();
    assert_eq!(document.block(1).unwrap().quote_depth, 1);
    assert_eq!(document.block(1).unwrap().list_depth, 1);
}

#[test]
fn enter_in_a_bold_item_keeps_the_bold_on_both_items() {
    let _serial = serial();
    let mut doc = open("- **bo|ld**\n");
    doc.check(Open::enter, "- **bo**\n- **|ld**\n");
    assert_eq!(doc.texts(), ["bo", "ld"]);
}

#[test]
fn enter_in_an_empty_last_item_leaves_the_list() {
    let _serial = serial();
    let mut doc = open("- a\n- |");
    doc.check(Open::enter, "- a\n\n|");
    assert_eq!(doc.kinds(), [bullet()]);
    // What is typed next is a paragraph, not a part of the item.
    doc.type_text("x");
    assert_eq!(doc.kinds(), [bullet(), paragraph()]);
    assert_eq!(doc.source(), "- a\n\nx");
}

#[test]
fn enter_in_an_empty_item_with_a_line_after_it_keeps_a_blank_line_on_each_side() {
    let _serial = serial();
    let mut doc = open("para\n\n- |\n\nafter\n");
    doc.check(Open::enter, "para\n\n|\n\nafter\n");
    assert_eq!(doc.kinds(), [paragraph(), paragraph()]);

    // The next line is a list of another kind: it must not be glued to the
    // text that is typed on the line the item leaves.
    let mut doc = open("- a\n- |\n2. b\n");
    doc.enter();
    doc.type_text("x");
    assert_eq!(doc.texts(), ["a", "x", "b"]);
    assert_eq!(doc.kinds(), [bullet(), paragraph(), ordered(2)]);
}

#[test]
fn enter_in_an_empty_nested_item_goes_one_level_out() {
    let _serial = serial();
    let mut doc = open("- a\n  - n\n  - |");
    doc.check(Open::enter, "- a\n  - n\n- |");
    let document = doc.markdown().document();
    assert_eq!(document.block(2).unwrap().list_depth, 1);
    assert_eq!(document.block(1).unwrap().list_depth, 2);

    let mut doc = open("1. a\n   - n\n   - |\n");
    doc.check(Open::enter, "1. a\n   - n\n- |\n");
}

#[test]
fn enter_in_an_empty_task_leaves_the_list() {
    let _serial = serial();
    let mut doc = open("- [ ] |");
    doc.check(Open::enter, "|");
    assert!(doc.markdown().document().is_empty());
}

// ---------------------------------------------------------------------------
// Enter: quotes and code
// ---------------------------------------------------------------------------

#[test]
fn enter_in_a_quote_goes_on_with_the_quote_mark() {
    let _serial = serial();
    let mut doc = open("> ab|c\n");
    doc.check(Open::enter, "> ab\n>\n> |c\n");
    let document = doc.markdown().document();
    assert_eq!(doc.texts(), ["ab", "c"]);
    assert_eq!(
        (
            document.block(0).unwrap().quote_depth,
            document.block(1).unwrap().quote_depth
        ),
        (1, 1)
    );
}

#[test]
fn enter_at_the_end_of_a_quote_and_again_leaves_it() {
    let _serial = serial();
    let mut doc = open("> abc|\n");
    doc.check(Open::enter, "> abc\n>\n> |\n");
    doc.check(Open::enter, "> abc\n>\n|\n");
    doc.type_text("x");
    let document = doc.markdown().document();
    assert_eq!(doc.texts(), ["abc", "x"]);
    assert_eq!(
        (
            document.block(0).unwrap().quote_depth,
            document.block(1).unwrap().quote_depth
        ),
        (1, 0)
    );
}

#[test]
fn enter_in_a_nested_quote_keeps_both_marks() {
    let _serial = serial();
    let mut doc = open("> > ab|c\n");
    doc.check(Open::enter, "> > ab\n> >\n> > |c\n");
    let document = doc.markdown().document();
    assert_eq!(document.block(1).unwrap().quote_depth, 2);
}

#[test]
fn enter_in_a_code_block_is_a_literal_line_break() {
    let _serial = serial();
    let mut doc = open("```rust\nlet a|b;\n```\n\nafter\n");
    doc.check(Open::enter, "```rust\nlet a\n|b;\n```\n\nafter\n");
    assert_eq!(doc.texts(), ["let a\nb;\n", "after"]);
    assert_eq!(doc.markdown().document().len(), 2);
}

#[test]
fn enter_at_the_start_and_end_of_the_code_is_a_line_break_too() {
    let _serial = serial();
    let mut doc = open("```\n|code\n```\n");
    doc.check(Open::enter, "```\n\n|code\n```\n");
    assert_eq!(doc.texts(), ["\ncode\n"]);

    let mut doc = open("```\ncode\n|```\n");
    doc.check(Open::enter, "```\ncode\n\n|```\n");
    assert_eq!(doc.texts(), ["code\n\n"]);
}

#[test]
fn enter_on_a_fence_does_not_break_it() {
    let _serial = serial();
    for marked in [
        "```rust|\ncode\n```\n",
        "```\ncode\n```|\n",
        "```\ncode\n``|`\n",
    ] {
        let mut doc = open(marked);
        let before = doc.source();
        doc.enter();
        assert_eq!(doc.source(), before, "{marked:?}");
    }
}

#[test]
fn enter_in_a_code_block_inside_a_list_keeps_the_indentation() {
    let _serial = serial();
    let mut doc = open("- a\n\n  ```\n  c|d\n  ```\n");
    doc.check(Open::enter, "- a\n\n  ```\n  c\n  |d\n  ```\n");
    assert!(matches!(doc.kinds()[1], BlockKind::CodeBlock { .. }));
    assert_eq!(doc.texts()[1], "c\nd\n");
}

#[test]
fn enter_in_a_block_with_no_text_does_nothing() {
    let _serial = serial();
    let mut doc = open("---|\n\nafter\n");
    doc.check(Open::enter, "---|\n\nafter\n");
    let mut doc = open("|");
    doc.check(Open::enter, "|");
}

// ---------------------------------------------------------------------------
// Backspace at the start of a block
// ---------------------------------------------------------------------------

#[test]
fn backspace_at_the_start_of_a_paragraph_joins_it_to_the_one_above() {
    let _serial = serial();
    let mut doc = open("one\n\n|two\n");
    doc.check(Open::backspace, "one|two\n");
    assert_eq!(doc.texts(), ["onetwo"]);
    doc.assert_caret_is_in_a_block();
}

#[test]
fn backspace_in_the_middle_of_a_paragraph_still_deletes_one_character() {
    let _serial = serial();
    let mut doc = open("one\n\nt|wo\n");
    doc.check(Open::backspace, "one\n\n|wo\n");
}

#[test]
fn backspace_joins_with_the_closing_delimiters_of_the_text_above_left_alone() {
    let _serial = serial();
    let mut doc = open("**a**\n\n|b\n");
    doc.check(Open::backspace, "**a**|b\n");
    assert_eq!(doc.texts(), ["ab"]);
    let runs = &doc.markdown().document().block(0).unwrap().content.runs;
    assert_eq!(runs.len(), 2, "bold a, plain b");
}

#[test]
fn backspace_joins_several_blocks_one_after_the_other() {
    let _serial = serial();
    let mut doc = open("a\n\nb\n\n|c\n");
    doc.check(Open::backspace, "a\n\nb|c\n");
    doc.caret_at(3);
    doc.check(Open::backspace, "a|bc\n");
    assert_eq!(doc.texts(), ["abc"]);
}

#[test]
fn backspace_at_the_start_of_the_first_block_does_nothing() {
    let _serial = serial();
    let mut doc = open("|one\n\ntwo\n");
    doc.check(Open::backspace, "|one\n\ntwo\n");
    assert_eq!(doc.editor().history().undo_len(), 0);
}

#[test]
fn backspace_at_the_start_of_a_heading_takes_off_its_marker_first() {
    let _serial = serial();
    let mut doc = open("# |Title\n");
    doc.check(Open::backspace, "|Title\n");
    assert_eq!(doc.kinds(), [paragraph()]);

    let mut doc = open("intro\n\n### |Title\n\nnext\n");
    doc.check(Open::backspace, "intro\n\n|Title\n\nnext\n");
    assert_eq!(doc.kinds(), [paragraph(), paragraph(), paragraph()]);
    doc.check(Open::backspace, "intro|Title\n\nnext\n");
}

#[test]
fn backspace_at_the_start_of_a_heading_that_touches_a_paragraph_keeps_them_apart() {
    let _serial = serial();
    let mut doc = open("above\n# |Title\nbelow\n");
    doc.backspace();
    // `above` and `below` must not become one paragraph with the title.
    assert_eq!(doc.texts(), ["above", "Title", "below"]);
    doc.assert_is_a_full_parse();
    assert_eq!(doc.kinds(), [paragraph(), paragraph(), paragraph()]);
}

#[test]
fn backspace_at_the_start_of_a_list_item_takes_off_the_marker_then_joins() {
    let _serial = serial();
    let mut doc = open("- a\n- |b\n");
    doc.check(Open::backspace, "- a\n\n|b\n");
    assert_eq!(doc.kinds(), [bullet(), paragraph()]);
    doc.check(Open::backspace, "- a|b\n");
    assert_eq!(doc.kinds(), [bullet()]);
    assert_eq!(doc.texts(), ["ab"]);
}

#[test]
fn backspace_in_the_middle_of_a_list_keeps_the_items_on_both_sides() {
    let _serial = serial();
    let mut doc = open("- a\n- |b\n- c\n");
    doc.backspace();
    assert_eq!(doc.texts(), ["a", "b", "c"]);
    assert_eq!(doc.kinds(), [bullet(), paragraph(), bullet()]);
    doc.assert_is_a_full_parse();
}

#[test]
fn backspace_at_the_start_of_an_ordered_item_next_to_one_that_cannot_interrupt_a_paragraph() {
    let _serial = serial();
    let mut doc = open("1. a\n2. |b\n3. c\n");
    doc.backspace();
    // The `3.` item cannot start a list inside a paragraph: without a blank
    // line it would join the text of `b`.
    assert_eq!(doc.texts(), ["a", "b", "c"]);
    assert_eq!(doc.kinds(), [ordered(1), paragraph(), ordered(3)]);
}

#[test]
fn backspace_at_the_start_of_a_task_takes_off_the_box_with_the_marker() {
    let _serial = serial();
    let mut doc = open("- [ ] |todo\n");
    doc.check(Open::backspace, "|todo\n");
    assert_eq!(doc.kinds(), [paragraph()]);

    let mut doc = open("- [x] |done\n");
    doc.check(Open::backspace, "|done\n");
}

#[test]
fn backspace_in_an_empty_item_takes_off_the_marker() {
    let _serial = serial();
    let mut doc = open("- a\n- |\n");
    doc.check(Open::backspace, "- a\n\n|\n");
    assert_eq!(doc.kinds(), [bullet()]);
}

#[test]
fn backspace_at_the_start_of_a_nested_item_takes_it_out_of_the_list() {
    let _serial = serial();
    let mut doc = open("- a\n  - |n\n");
    doc.check(Open::backspace, "- a\n\n|n\n");
    assert_eq!(doc.kinds(), [bullet(), paragraph()]);
}

#[test]
fn backspace_at_the_start_of_a_list_item_in_a_quote_keeps_the_quote() {
    let _serial = serial();
    let mut doc = open("> - |a\n");
    doc.check(Open::backspace, "> |a\n");
    let document = doc.markdown().document();
    assert_eq!(document.block(0).unwrap().quote_depth, 1);
    assert_eq!(document.block(0).unwrap().list_depth, 0);
    doc.check(Open::backspace, "|a\n");
}

#[test]
fn backspace_at_the_start_of_a_quote_takes_one_quote_mark_off() {
    let _serial = serial();
    let mut doc = open("> |quoted\n");
    doc.check(Open::backspace, "|quoted\n");
    assert_eq!(doc.markdown().document().block(0).unwrap().quote_depth, 0);

    let mut doc = open("> > |deep\n");
    doc.check(Open::backspace, "> |deep\n");
    assert_eq!(doc.markdown().document().block(0).unwrap().quote_depth, 1);
}

#[test]
fn backspace_at_the_start_of_a_quote_of_several_lines_takes_the_mark_off_every_line() {
    let _serial = serial();
    let mut doc = open("> |one\n> two\n");
    doc.check(Open::backspace, "|one\ntwo\n");
    assert_eq!(doc.texts(), ["one two"]);
    assert_eq!(doc.markdown().document().block(0).unwrap().quote_depth, 0);
}

#[test]
fn backspace_at_the_start_of_the_second_paragraph_of_a_quote_leaves_the_quote() {
    let _serial = serial();
    let mut doc = open("> a\n>\n> |b\n");
    doc.backspace();
    let document = doc.markdown().document();
    assert_eq!(doc.texts(), ["a", "b"]);
    assert_eq!(
        (
            document.block(0).unwrap().quote_depth,
            document.block(1).unwrap().quote_depth
        ),
        (1, 0)
    );
    doc.assert_is_a_full_parse();
}

#[test]
fn backspace_after_a_rule_takes_the_rule_away() {
    let _serial = serial();
    let mut doc = open("text\n\n---\n\n|para\n");
    doc.check(Open::backspace, "text\n\n|para\n");
    assert_eq!(doc.kinds(), [paragraph(), paragraph()]);
}

#[test]
fn backspace_after_a_code_block_or_a_table_leaves_them_alone() {
    let _serial = serial();
    for source in [
        "```\ncode\n```\n\nafter\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n\nafter\n",
    ] {
        let mut doc = Open::with_caret(source, source.find("after").unwrap());
        let before = doc.marked();
        doc.check(Open::backspace, &before);
        assert_eq!(doc.editor().history().undo_len(), 0);
    }
}

#[test]
fn backspace_in_a_code_block_does_not_touch_the_fence() {
    let _serial = serial();
    let mut doc = open("para\n\n```\n|code\n```\n");
    let before = doc.source();
    doc.backspace();
    assert_eq!(doc.source(), before);
}

// ---------------------------------------------------------------------------
// Delete at the end of a block
// ---------------------------------------------------------------------------

#[test]
fn delete_at_the_end_of_a_paragraph_joins_the_next_one() {
    let _serial = serial();
    let mut doc = open("one|\n\ntwo\n");
    doc.check(Open::delete, "one|two\n");
    assert_eq!(doc.texts(), ["onetwo"]);
}

#[test]
fn delete_in_the_middle_of_a_paragraph_still_deletes_one_character() {
    let _serial = serial();
    let mut doc = open("o|ne\n\ntwo\n");
    doc.check(Open::delete, "o|e\n\ntwo\n");
}

#[test]
fn delete_at_the_end_of_the_last_block_does_nothing() {
    let _serial = serial();
    let mut doc = open("one\n\ntwo|\n");
    doc.check(Open::delete, "one\n\ntwo|\n");
    assert_eq!(doc.editor().history().undo_len(), 0);
}

#[test]
fn delete_drops_the_marker_of_the_item_it_joins() {
    let _serial = serial();
    let mut doc = open("- a|\n- b\n- c\n");
    doc.check(Open::delete, "- a|b\n- c\n");
    assert_eq!(doc.texts(), ["ab", "c"]);

    let mut doc = open("1. a|\n2. b\n");
    doc.check(Open::delete, "1. a|b\n");

    let mut doc = open("- a|\n- [x] b\n");
    doc.check(Open::delete, "- a|b\n");
}

#[test]
fn delete_drops_the_marks_of_a_heading_or_a_quote_it_joins() {
    let _serial = serial();
    let mut doc = open("a|\n\n## T\n");
    doc.check(Open::delete, "a|T\n");

    let mut doc = open("a|\n\n> q\n");
    doc.check(Open::delete, "a|q\n");
    assert_eq!(doc.kinds(), [paragraph()]);
}

#[test]
fn delete_joins_a_quote_of_several_lines_without_leaving_marks_in_the_text() {
    let _serial = serial();
    let mut doc = open("a|\n\n> b1\n> b2\n");
    doc.check(Open::delete, "a|b1\nb2\n");
    assert_eq!(doc.texts(), ["ab1 b2"]);

    let mut doc = open("a|\n\n- b1\n  b2\n- c\n");
    doc.check(Open::delete, "a|b1\nb2\n- c\n");
    assert_eq!(doc.texts(), ["ab1 b2", "c"]);
}

#[test]
fn delete_at_the_end_of_bold_text_joins_after_the_closing_delimiters() {
    let _serial = serial();
    let mut doc = open("**bold|**\n\nnext\n");
    doc.check(Open::delete, "**bold**|next\n");
    assert_eq!(doc.texts(), ["boldnext"]);
}

#[test]
fn delete_before_a_rule_takes_the_rule_away() {
    let _serial = serial();
    let mut doc = open("a|\n\n---\n\nb\n");
    doc.check(Open::delete, "a|\n\nb\n");
    assert_eq!(doc.kinds(), [paragraph(), paragraph()]);

    let mut doc = open("a|\n\n---\n");
    doc.check(Open::delete, "a|\n\n");
}

#[test]
fn delete_before_a_code_block_or_a_table_leaves_them_alone() {
    let _serial = serial();
    for source in [
        "a\n\n```\ncode\n```\n",
        "a\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
    ] {
        let mut doc = Open::with_caret(source, 1);
        let before = doc.marked();
        doc.check(Open::delete, &before);
        assert_eq!(doc.editor().history().undo_len(), 0);
    }
}

#[test]
fn delete_on_the_blank_line_between_blocks_pulls_the_next_one_up() {
    let _serial = serial();
    let mut doc = open("one|\n\ntwo\n");
    doc.enter();
    assert_eq!(doc.source(), "one\n\n\n\ntwo\n");
    doc.check(Open::delete, "one\n\n|two\n");
}

// ---------------------------------------------------------------------------
// Words
// ---------------------------------------------------------------------------

#[test]
fn control_right_and_left_move_over_words() {
    let _serial = serial();
    let mut doc = open("|hello big world\n");
    doc.word_right();
    assert_eq!(doc.marked(), "hello| big world\n");
    doc.word_right();
    assert_eq!(doc.marked(), "hello big| world\n");
    doc.word_right();
    assert_eq!(doc.marked(), "hello big world|\n");
    doc.word_right();
    assert_eq!(doc.marked(), "hello big world|\n", "stays at the end");

    doc.word_left();
    assert_eq!(doc.marked(), "hello big |world\n");
    doc.word_left();
    assert_eq!(doc.marked(), "hello |big world\n");
    doc.word_left();
    assert_eq!(doc.marked(), "|hello big world\n");
    doc.word_left();
    assert_eq!(doc.marked(), "|hello big world\n", "stays at the start");
}

#[test]
fn a_word_motion_leaves_no_step_in_the_history_and_no_selection() {
    let _serial = serial();
    let mut doc = open("|one two\n");
    doc.word_right();
    assert_eq!(doc.editor().history().undo_len(), 0);
    assert_eq!(doc.editor().selection(), None);
}

#[test]
fn command_does_the_same_as_control() {
    let _serial = serial();
    let mut doc = open("|one two\n");
    doc.key_event(KeyCode::SuperLeft, Key::Super, ButtonState::Pressed);
    doc.press_events(KeyCode::ArrowRight, Key::ArrowRight);
    doc.key_event(KeyCode::SuperLeft, Key::Super, ButtonState::Released);
    doc.settle();
    assert_eq!(doc.marked(), "one| two\n");
}

#[test]
fn control_right_walks_over_a_word_split_in_styled_runs() {
    let _serial = serial();
    let mut doc = open("|say **bold words** now\n");
    let mut stops = vec![];
    for _ in 0..4 {
        doc.word_right();
        stops.push(doc.editor().caret());
    }
    let document = doc.markdown().document();
    let text = &document.block(0).unwrap().content.text;
    assert_eq!(text, "say bold words now");
    // The caret is on a word boundary of the text after each motion.
    let block = document.block(0).unwrap();
    let ends: Vec<&str> = stops
        .iter()
        .map(|caret| &text[..block.text_offset(*caret).unwrap()])
        .collect();
    assert_eq!(
        ends,
        ["say", "say bold", "say bold words", "say bold words now"]
    );
}

#[test]
fn a_word_motion_does_not_leave_the_block() {
    let _serial = serial();
    let mut doc = open("one\n\n|two\n");
    doc.word_left();
    assert_eq!(doc.marked(), "one\n\n|two\n");
    doc.word_right();
    doc.word_right();
    assert_eq!(doc.marked(), "one\n\ntwo|\n");
}

#[test]
fn control_backspace_deletes_the_word_before_the_caret() {
    let _serial = serial();
    let mut doc = open("hello big world| end\n");
    doc.check(Open::word_backspace, "hello big | end\n");
    doc.check(Open::word_backspace, "hello | end\n");
}

#[test]
fn control_backspace_in_the_middle_of_a_word_deletes_up_to_its_start() {
    let _serial = serial();
    let mut doc = open("hello wor|ld\n");
    doc.check(Open::word_backspace, "hello |ld\n");
}

#[test]
fn control_delete_deletes_the_word_after_the_caret() {
    let _serial = serial();
    let mut doc = open("hello |big world end\n");
    doc.check(Open::word_delete, "hello | world end\n");
    doc.check(Open::word_delete, "hello | end\n");
}

#[test]
fn deleting_the_first_word_leaves_the_caret_in_the_block_not_before_its_blanks() {
    let _serial = serial();
    let mut doc = open("|hello big\n");
    // The parser drops the space that is left in front of "big", and a caret
    // before it would be between blocks, with no cursor and no motion.
    doc.check(Open::word_delete, " |big\n");
    doc.assert_caret_is_in_a_block();
    doc.word_right();
    assert_eq!(doc.marked(), " big|\n");
}

#[test]
fn enter_before_a_space_puts_the_caret_in_the_new_block() {
    let _serial = serial();
    let mut doc = open("one| two\n");
    doc.check(Open::enter, "one\n\n |two\n");
    doc.assert_caret_is_in_a_block();
    assert_eq!(doc.texts(), ["one", "two"]);

    let mut doc = open("- one| two\n");
    doc.check(Open::enter, "- one\n-  |two\n");
    assert_eq!(doc.assert_caret_is_in_a_block(), 1);
}

#[test]
fn deleting_words_is_one_step_each_and_never_merges_with_typing() {
    let _serial = serial();
    let mut doc = open("one two three| end\n");
    doc.type_text("x");
    doc.word_backspace();
    doc.word_backspace();
    assert_eq!(doc.marked(), "one | end\n");
    assert_eq!(doc.editor().history().undo_len(), 3);
    doc.undo();
    assert_eq!(doc.marked(), "one two | end\n");
    doc.undo();
    assert_eq!(doc.marked(), "one two threex| end\n");
    doc.undo();
    assert_eq!(doc.marked(), "one two three| end\n");
}

#[test]
fn deleting_a_word_inside_bold_text_edits_only_its_text() {
    let _serial = serial();
    let mut doc = open("**one tw|o** end\n");
    doc.check(Open::word_backspace, "**one |o** end\n");
    assert_eq!(doc.texts(), ["one o end"]);
    let runs = &doc.markdown().document().block(0).unwrap().content.runs;
    assert_eq!(runs[0].style, Style::BOLD);
}

#[test]
fn deleting_a_whole_styled_word_takes_its_delimiters() {
    let _serial = serial();
    let mut doc = open("a **bold**| b\n");
    doc.word_backspace();
    assert_eq!(doc.source(), "a  b\n");
    doc.assert_is_a_full_parse();
}

#[test]
fn control_backspace_at_the_start_of_a_block_joins_it_like_backspace() {
    let _serial = serial();
    let mut doc = open("one\n\n|two\n");
    doc.check(Open::word_backspace, "one|two\n");

    let mut doc = open("- a\n- |b\n");
    doc.check(Open::word_backspace, "- a\n\n|b\n");
}

#[test]
fn control_delete_at_the_end_of_a_block_joins_it_like_delete() {
    let _serial = serial();
    let mut doc = open("one|\n\ntwo\n");
    doc.check(Open::word_delete, "one|two\n");
}

#[test]
fn control_backspace_and_delete_do_nothing_at_the_ends_of_the_document() {
    let _serial = serial();
    let mut doc = open("|one\n");
    doc.check(Open::word_backspace, "|one\n");
    let mut doc = open("one|\n");
    doc.check(Open::word_delete, "one|\n");
    assert_eq!(doc.editor().history().undo_len(), 0);
}

#[test]
fn control_backspace_on_the_blank_line_between_blocks_takes_it_back() {
    let _serial = serial();
    let mut doc = open("hello|\n");
    doc.enter();
    doc.check(Open::word_backspace, "hello|\n");
}

#[test]
fn words_are_found_in_a_block_that_has_no_layout() {
    let _serial = serial();
    let mut doc = open("|x\n");
    let big = synthetic::source(2_000);
    doc.set_source(&big);
    doc.settle();
    // A paragraph near the top that starts with a plain word and a space.
    let (target, start, word) = doc
        .markdown()
        .document()
        .blocks()
        .enumerate()
        .find_map(|(ix, b)| {
            let start = b.span().start;
            let source = doc.markdown().document().source();
            let word = source[start..].find(' ')?;
            (matches!(b.kind, BlockKind::Paragraph)
                && source[start..start + word]
                    .chars()
                    .all(|c| c.is_ascii_alphabetic())
                && word > 1)
                .then_some((ix, start, word))
        })
        .expect("a paragraph that starts with a word");
    set_scroll(&mut doc.app, doc.area, 20_000.0);
    doc.settle();
    assert!(
        doc.markdown().block_entity(target).is_none(),
        "block {target} is out of the window"
    );
    doc.caret_at(start + word);
    let original = doc.source();

    doc.word_backspace();
    let mut expected = original.clone();
    expected.replace_range(start..start + word, "");
    assert_eq!(doc.source(), expected, "the word was deleted");
    // The space that is left is dropped by the parser: the caret is after it.
    assert_eq!(doc.editor().caret(), start + 1);

    doc.word_right();
    assert!(doc.editor().caret() > start, "the caret moved over a word");
    doc.assert_is_a_full_parse();
}

// ---------------------------------------------------------------------------
// The rules of the editor
// ---------------------------------------------------------------------------

#[test]
fn enter_and_joins_wait_while_a_large_change_parses_and_apply_in_order_after() {
    let _serial = serial();
    let mut doc = open("one|\n");
    let big = format!("{}\n\nabcdef\n", "word ".repeat(2_000).trim_end());
    assert!(big.len() > Markdown::SYNC_LIMIT);
    doc.set_source(&big);
    assert!(doc.markdown().is_parsing());
    let at = big.len() - 4;
    doc.caret_at(at);

    // Enter, then a letter, then Enter again, then Backspace: all while the
    // document is behind the source.
    doc.press_events(KeyCode::Enter, Key::Enter);
    doc.type_events("X");
    doc.press_events(KeyCode::Enter, Key::Enter);
    doc.press_events(KeyCode::Backspace, Key::Backspace);
    doc.app.update();
    assert!(doc.editor().pending_actions() >= 1, "kept, not dropped");
    assert_eq!(doc.source(), big, "nothing was applied to the source");

    doc.settle();
    assert_eq!(doc.editor().pending_actions(), 0);
    // abc|def -> abc, |def; X typed at the start of def; Enter after the X
    // gives X, |def; Backspace at the start of def joins it to the X again.
    let mut expected = big.clone();
    expected.insert_str(at, "\n\nX");
    assert_eq!(doc.source(), expected);
    assert_eq!(doc.editor().caret(), at + 3);
    assert_eq!(doc.texts().last().map(String::as_str), Some("Xdef"));
    assert_eq!(
        doc.markdown().document().source(),
        doc.source(),
        "the document caught up"
    );
    doc.assert_is_a_full_parse();
}

#[test]
fn enter_does_not_wait_for_the_layout_but_backspace_after_it_does() {
    let _serial = serial();
    let mut doc = open("ab|cd\n");
    doc.press_events(KeyCode::Enter, Key::Enter);
    doc.press_events(KeyCode::Backspace, Key::Backspace);
    doc.app.update();
    assert_eq!(
        doc.source(),
        "ab\n\ncd\n",
        "Enter went through in the frame"
    );
    assert_eq!(
        doc.editor().pending_actions(),
        1,
        "Backspace waits for the layout of the new block"
    );
    doc.settle();
    assert_eq!(doc.source(), "abcd\n");
    assert_eq!(doc.editor().caret(), 2);
    doc.assert_is_a_full_parse();
}

#[test]
fn the_caret_and_the_focus_follow_the_edit() {
    let _serial = serial();
    let mut doc = open("- a|b\n- c\n");
    doc.enter();
    let block = doc.assert_caret_is_in_a_block();
    assert_eq!(block, 1);
    let entity = doc
        .markdown()
        .block_entity(block)
        .expect("the block is shown");
    let world = doc.app.world_mut();
    let pinned: Vec<Entity> = world
        .query_filtered::<Entity, With<bevy_md::Pinned>>()
        .iter(world)
        .collect();
    assert_eq!(pinned, vec![entity], "the focused block is the caret's");
}

#[test]
fn many_blocks_edited_in_a_row_stay_a_full_parse() {
    let _serial = serial();
    let mut doc = open("# Title|\n\n- one\n- two\n\n> quote\n");
    for _ in 0..3 {
        doc.enter();
        doc.type_text("n");
    }
    doc.assert_is_a_full_parse();
    for _ in 0..12 {
        doc.backspace();
        doc.assert_is_a_full_parse();
    }
    for _ in 0..40 {
        doc.undo();
        doc.assert_is_a_full_parse();
    }
    assert_eq!(doc.marked(), "# Title|\n\n- one\n- two\n\n> quote\n");
}
