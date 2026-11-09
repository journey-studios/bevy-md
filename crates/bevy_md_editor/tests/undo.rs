//! Undo and redo through the real input path: Cmd/Ctrl+Z and
//! Cmd/Ctrl+Shift+Z as keyboard events, in a document of the size that
//! matters.

#[path = "../../bevy_md/tests/common/mod.rs"]
mod common;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::window::Ime;
use bevy_md::{Markdown, Pinned};
use bevy_md_core::{BlockKind, synthetic};
use bevy_md_editor::{MarkdownEditor, MarkdownEditorPlugin};
use common::*;

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

    /// Scroll so that `block` is at the top of the screen.
    fn scroll_to_block(&mut self, block: usize) {
        let y = self.markdown().block_top(block) as f32 + 20.0;
        set_scroll(&mut self.app, self.area, y);
        settle(&mut self.app);
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
        self.markdown().document().source().to_owned()
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

    /// Type `text` as key presses, all in the next frame.
    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            let s = c.to_string();
            self.key(
                KeyCode::KeyA,
                Key::Character(s.as_str().into()),
                ButtonState::Pressed,
                Some(&s),
            );
            self.key(
                KeyCode::KeyA,
                Key::Character(s.as_str().into()),
                ButtonState::Released,
                None,
            );
        }
        self.app.update();
    }

    /// Press the command key (and shift), then Z, and run the frame.
    fn chord(&mut self, shift: bool) {
        self.key(KeyCode::SuperLeft, Key::Super, ButtonState::Pressed, None);
        if shift {
            self.key(KeyCode::ShiftLeft, Key::Shift, ButtonState::Pressed, None);
        }
        self.key(
            KeyCode::KeyZ,
            Key::Character("z".into()),
            ButtonState::Pressed,
            None,
        );
        self.app.update();
        self.key(
            KeyCode::KeyZ,
            Key::Character("z".into()),
            ButtonState::Released,
            None,
        );
        if shift {
            self.key(KeyCode::ShiftLeft, Key::Shift, ButtonState::Released, None);
        }
        self.key(KeyCode::SuperLeft, Key::Super, ButtonState::Released, None);
        self.app.update();
    }

    fn undo(&mut self) {
        self.chord(false);
    }

    fn redo(&mut self) {
        self.chord(true);
    }
}

#[test]
fn command_z_takes_back_what_was_typed_and_command_shift_z_does_it_again() {
    let _serial = serial();
    let original = "hello world\n\nsecond block\n";
    let mut doc = Open::new(original);
    doc.caret_at(11);
    doc.type_text(" abc");
    assert_eq!(doc.source(), "hello world abc\n\nsecond block\n");
    assert_eq!(doc.editor().caret(), 15);
    // " " and "abc" are two steps: typing groups by word.
    assert_eq!(doc.editor().history().undo_len(), 2);

    doc.undo();
    assert_eq!(doc.source(), "hello world \n\nsecond block\n");
    assert_eq!(doc.editor().caret(), 12);
    doc.undo();
    assert_eq!(doc.source(), original);
    assert_eq!(doc.editor().caret(), 11);
    // Nothing more to undo: nothing happens.
    doc.undo();
    assert_eq!(doc.source(), original);

    doc.redo();
    assert_eq!(doc.source(), "hello world \n\nsecond block\n");
    assert_eq!(doc.editor().caret(), 12);
    doc.redo();
    assert_eq!(doc.source(), "hello world abc\n\nsecond block\n");
    assert_eq!(doc.editor().caret(), 15);
    doc.redo();
    assert_eq!(doc.source(), "hello world abc\n\nsecond block\n");
    doc.markdown().document().assert_consistent();
}

#[test]
fn control_z_works_like_command_z() {
    let _serial = serial();
    let mut doc = Open::new("one\n");
    doc.caret_at(3);
    doc.type_text("two");
    assert_eq!(doc.source(), "onetwo\n");
    doc.key(
        KeyCode::ControlLeft,
        Key::Control,
        ButtonState::Pressed,
        None,
    );
    doc.key(
        KeyCode::KeyZ,
        Key::Character("Z".into()),
        ButtonState::Pressed,
        None,
    );
    doc.app.update();
    assert_eq!(doc.source(), "one\n");
}

#[test]
fn z_without_the_command_key_is_just_a_letter() {
    let _serial = serial();
    let mut doc = Open::new("one\n");
    doc.caret_at(3);
    doc.type_text("z");
    assert_eq!(doc.source(), "onez\n");
    assert_eq!(doc.editor().history().undo_len(), 1);
}

#[test]
fn a_new_edit_after_undo_cannot_be_redone_over() {
    let _serial = serial();
    let mut doc = Open::new("a\n");
    doc.caret_at(1);
    doc.type_text("bc");
    doc.undo();
    assert_eq!(doc.source(), "a\n");
    doc.type_text("X");
    assert_eq!(doc.source(), "aX\n");
    doc.redo();
    assert_eq!(doc.source(), "aX\n", "the undone typing is gone for good");
    assert_eq!(doc.editor().history().redo_len(), 0);
}

#[test]
fn undo_puts_the_caret_back_and_the_editor_follows_it_to_another_block() {
    let _serial = serial();
    let mut doc = Open::new(&synthetic::source(2_000));
    let original = doc.source();
    let first = doc.markdown().document().block(2).unwrap().span().start + 1;
    doc.caret_at(first);
    doc.type_text("Q");
    // Move the caret far away, to another block, and undo.
    let far = doc.markdown().document().block(40).unwrap().span().start + 1;
    doc.caret_at(far);
    settle(&mut doc.app);
    doc.undo();
    settle(&mut doc.app);
    assert_eq!(doc.source(), original);
    assert_eq!(doc.editor().caret(), first);
    let block = doc.markdown().document().block_at(first).unwrap();
    let root = doc
        .markdown()
        .block_entity(block)
        .expect("the block is shown");
    let world = doc.app.world_mut();
    let pinned: Vec<Entity> = world
        .query_filtered::<Entity, With<Pinned>>()
        .iter(world)
        .collect();
    assert_eq!(pinned, vec![root], "the caret's block is the pinned one");
}

#[test]
fn undo_renders_again_only_the_blocks_it_changes() {
    let _serial = serial();
    let mut doc = Open::new(&synthetic::source(2_000));
    // A paragraph: typing in it does not change what kind of block it is.
    let target = doc
        .markdown()
        .document()
        .blocks()
        .position(|b| matches!(b.kind, BlockKind::Paragraph))
        .expect("a paragraph");
    let at = doc
        .markdown()
        .document()
        .block(target)
        .unwrap()
        .span()
        .start
        + 1;
    doc.caret_at(at);
    settle(&mut doc.app);
    let before: Vec<_> = doc.markdown().live_blocks().collect();
    let original = doc.source();

    // The same keystroke, as a point of comparison: typing, then undoing it.
    doc.type_text("x");
    let typed = *doc.app.world().resource::<FrameStats>();
    settle(&mut doc.app);

    doc.key(KeyCode::SuperLeft, Key::Super, ButtonState::Pressed, None);
    doc.key(
        KeyCode::KeyZ,
        Key::Character("z".into()),
        ButtonState::Pressed,
        None,
    );
    doc.app.update();
    let stats = *doc.app.world().resource::<FrameStats>();
    let writes = doc.app.world().resource::<Writes>().clone();
    let laid = doc.app.world().resource::<LaidOut>().0.clone();
    println!(
        "typing: {} laid out; undo: {} laid out, {} texts written, {} entities written",
        typed.laid_out,
        stats.laid_out,
        writes.texts.len(),
        writes.all.len()
    );
    assert_eq!(doc.source(), original, "the undo was applied in that frame");
    assert!(typed.laid_out >= 1, "typing lays out the block typed in");
    assert!(stats.laid_out >= 1, "undoing lays out the block it changed");
    report("undo_laid_out", stats.laid_out);
    report("undo_text_writes", writes.texts.len());
    assert_eq!(
        stats.spawned + stats.despawned,
        0,
        "no node was made or lost"
    );
    check("undo_laid_out", stats.laid_out);
    check("undo_text_writes", writes.texts.len());
    // Only the block undone was rendered again: everything laid out or
    // written is under it, and every other block keeps its entity.
    let edited = doc
        .markdown()
        .block_entity(target)
        .expect("the block is shown");
    let under = |mut e: Entity| loop {
        if e == edited {
            return true;
        }
        match doc.app.world().get::<ChildOf>(e) {
            Some(parent) => e = parent.parent(),
            None => return false,
        }
    };
    assert!(laid.iter().all(|&t| under(t)), "laid out outside the block");
    assert!(
        writes.all.iter().all(|&w| under(w)),
        "written outside the block"
    );
    let after: Vec<_> = doc.markdown().live_blocks().collect();
    let others = |blocks: &[(usize, Entity)]| {
        blocks
            .iter()
            .copied()
            .filter(|(ix, _)| *ix != target)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        others(&before),
        others(&after),
        "every other block keeps its entity"
    );
}

#[test]
fn history_memory_follows_the_text_changed_not_the_document() {
    let _serial = serial();
    // One byte per step: a letter and a space, over and over.
    let mut per_step = Vec::new();
    for blocks in [500, 20_000] {
        let mut doc = Open::new(&synthetic::source(blocks));
        let at = doc.markdown().document().block(3).unwrap().span().start + 1;
        doc.caret_at(at);
        for _ in 0..50 {
            doc.type_text("a b ");
        }
        let history = doc.editor().history();
        let steps = history.undo_len();
        assert_eq!(
            steps, 200,
            "one step per word and per space, up to the limit"
        );
        per_step.push(history.bytes() / steps);
    }
    println!("history bytes per one-byte step: {per_step:?}");
    assert_eq!(per_step[0], per_step[1], "the document size is not in it");
    report("undo_bytes_per_step", per_step[0]);
    check("undo_bytes_per_step", per_step[0]);

    // A paste of 4000 bytes costs about those bytes, not a document.
    let mut doc = Open::new(&synthetic::source(20_000));
    let at = doc.markdown().document().block(3).unwrap().span().start + 1;
    doc.caret_at(at);
    let pasted = "x".repeat(4_000);
    doc.app.world_mut().write_message(Ime::Commit {
        window: Entity::PLACEHOLDER,
        value: pasted.clone(),
    });
    doc.app.update();
    assert_eq!(doc.editor().history().undo_len(), 1);
    let bytes = doc.editor().history().bytes();
    #[allow(clippy::cast_precision_loss)]
    let ratio = bytes as f64 / pasted.len() as f64;
    println!("history bytes of a 4000-byte paste: {bytes} ({ratio:.2} per byte)");
    report("undo_bytes_per_pasted_byte", ratio);
    check("undo_bytes_per_pasted_byte", ratio);
}

// ---------------------------------------------------------------------------
// A definition typed at the end of a note whose mentions are far above it
// ---------------------------------------------------------------------------

/// The links in the first inline text of block `block`.
fn links_in_block(doc: &Open, block: usize) -> usize {
    doc.markdown()
        .document()
        .block(block)
        .unwrap()
        .content
        .links
        .len()
}

const FILLER: usize = 40_000;
/// Where `spread_references_source(FILLER)` mentions the label.
const MENTIONS: [usize; 3] = [0, FILLER + 1, 2 * FILLER + 2];

/// The document shown is what a full parse of its source gives (the blocks
/// at the mentions, at the end and every 997th are compared).
#[track_caller]
fn assert_full_parse(doc: &Open, context: &str) {
    let shown = doc.markdown().document();
    let full = bevy_md_core::Document::parse(shown.source());
    shown.assert_consistent();
    assert_eq!(shown.len(), full.len(), "{context}: block count");
    for ix in (0..shown.len())
        .step_by(997)
        .chain(MENTIONS)
        .chain([shown.len() - 1])
    {
        assert_eq!(shown.block(ix), full.block(ix), "{context}: block {ix}");
    }
}

/// The entities with the pin.
fn pinned_nodes(doc: &mut Open) -> Vec<Entity> {
    let world = doc.app.world_mut();
    world
        .query_filtered::<Entity, With<Pinned>>()
        .iter(world)
        .collect()
}

/// Typing the definition of a label at the end of a 120 000-block note,
/// undoing it and doing it again: the three mentions, ~40 000 blocks apart
/// and far from the caret, change with it (as links, then as text, then as
/// links), the document stays what a full parse of its source gives, the
/// blocks around the caret keep their nodes, and the caret and the pin stay
/// with the block of the caret.
#[test]
fn a_definition_typed_at_the_end_reaches_far_mentions_and_undoes_with_them() {
    let _serial = serial();
    let mentions = MENTIONS;
    let mut doc = Open::new(&synthetic::spread_references_source(FILLER));
    assert_eq!(doc.markdown().document().len(), 3 * FILLER + 3);
    let original = doc.source();
    let end = original.len();
    doc.caret_at(end);
    doc.scroll_to_block(3 * FILLER + 3 - 12);
    let blocks = doc.markdown().document().len();
    let before: Vec<_> = doc.markdown().live_blocks().collect();
    assert!(
        before.iter().all(|(ix, _)| *ix > blocks - 100),
        "the caret scrolled the end into view: {:?}",
        before.first()
    );
    for mention in mentions {
        assert_eq!(links_in_block(&doc, mention), 0);
    }

    // Typed: the definition is complete only with its destination.
    doc.type_text("[shared]: /dest");
    let typed = *doc.app.world().resource::<FrameStats>();
    assert!(
        doc.source().ends_with("[shared]: /dest"),
        "typed at the end"
    );
    assert_full_parse(&doc, "typed");
    for mention in mentions {
        assert_eq!(links_in_block(&doc, mention), 1, "mention {mention} typed");
    }
    assert_eq!(typed.despawned, 0, "typing the definition despawned nodes");
    assert!(
        typed.laid_out <= 4,
        "{} laid out by the keystrokes",
        typed.laid_out
    );
    settle(&mut doc.app);
    let live: Vec<_> = doc.markdown().live_blocks().collect();
    assert!(live.len() <= before.len() + 4, "{} live", live.len());
    // The blocks that were on screen and are still blocks are the same nodes.
    let blocks_now = doc.markdown().document().len();
    for (ix, entity) in &before {
        if *ix + 1 < blocks_now.min(blocks) {
            assert_eq!(
                doc.markdown().block_entity(*ix),
                Some(*entity),
                "block {ix} lost its node"
            );
        }
    }
    assert!(pinned_nodes(&mut doc).len() <= 1);

    // Undone: the mentions are text again, the text is the original.
    doc.undo();
    settle(&mut doc.app);
    assert!(doc.source().starts_with(&original));
    assert_full_parse(&doc, "undone");
    let undone = doc.source();
    for mention in mentions {
        let links = links_in_block(&doc, mention);
        assert_eq!(
            links,
            usize::from(undone.contains("[shared]:")),
            "mention {mention} after undo"
        );
    }
    // Undo until the original (typing groups by word, so a few steps).
    let remaining = doc.editor().history().undo_len();
    let steps = remaining + 1;
    assert!((1..=8).contains(&steps), "{steps} steps to undo");
    for _ in 0..remaining {
        doc.undo();
        settle(&mut doc.app);
    }
    assert_eq!(doc.editor().history().undo_len(), 0);
    assert!(doc.source() == original, "the source is not the original");
    assert_full_parse(&doc, "undone to the original");
    for mention in mentions {
        assert_eq!(links_in_block(&doc, mention), 0, "mention {mention} undone");
    }
    assert_eq!(doc.editor().caret(), end);
    assert!(pinned_nodes(&mut doc).len() <= 1);

    // Redone: the same definition, the same links.
    for _ in 0..steps {
        doc.redo();
        settle(&mut doc.app);
    }
    assert_eq!(doc.editor().history().redo_len(), 0);
    assert!(doc.source().ends_with("[shared]: /dest"));
    assert_full_parse(&doc, "redone");
    for mention in mentions {
        assert_eq!(links_in_block(&doc, mention), 1, "mention {mention} redone");
    }
    assert!(pinned_nodes(&mut doc).len() <= 1);
}
