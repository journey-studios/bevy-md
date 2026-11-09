//! Only the block with the caret carries the editor: its components, its
//! pin. Having them or not must not change how the block breaks its lines,
//! and a document must cost the same with or without an editor on it.

#[path = "../../bevy_md/tests/common/mod.rs"]
mod common;

use bevy::prelude::*;
use bevy::text::{ComputedTextBlock, TextCursorStyle, TextReadWriteMode};
use bevy::ui::ComputedNode;
use bevy_md::{BlockText, Markdown, Pinned};
use bevy_md_core::synthetic;
use bevy_md_editor::{MarkdownEditor, MarkdownEditorPlugin};
use common::*;

struct Open {
    app: App,
    area: Entity,
    markdown: Entity,
}

impl Open {
    fn new(source: &str, editor: bool) -> Self {
        let mut app = headless_app();
        app.update();
        app.add_message::<bevy::window::Ime>();
        if editor {
            app.add_plugins(MarkdownEditorPlugin);
        }
        let (area, markdown) = spawn_document(&mut app, source);
        if editor {
            app.world_mut()
                .entity_mut(markdown)
                .insert(MarkdownEditor::new());
        }
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

    /// Put the caret `inside` bytes into block `block`, and let it settle.
    fn caret_in(&mut self, block: usize, inside: usize) {
        self.move_caret(block, inside);
        settle(&mut self.app);
    }

    /// Put the caret there and run one frame; returns how many text blocks
    /// Bevy laid out in it.
    fn caret_in_one_frame(&mut self, block: usize, inside: usize) -> usize {
        self.move_caret(block, inside);
        self.app.update();
        let laid_out = self.app.world().resource::<FrameStats>().laid_out;
        settle(&mut self.app);
        laid_out
    }

    fn move_caret(&mut self, block: usize, inside: usize) {
        let start = self
            .markdown()
            .document()
            .block(block)
            .unwrap()
            .span()
            .start;
        let world = self.app.world_mut();
        let (mut editor, markdown) = world
            .query::<(&mut MarkdownEditor, &Markdown)>()
            .single_mut(world)
            .unwrap();
        editor.set_caret(markdown, start + inside);
    }

    fn text_of(&self, block: usize) -> Entity {
        self.try_text_of(block)
            .unwrap_or_else(|| panic!("block {block} has no text"))
    }

    /// The text of a block, if it has one (a rule has none).
    fn try_text_of(&self, block: usize) -> Option<Entity> {
        let root = self.markdown().block_entity(block)?;
        let world = self.app.world();
        let mut stack = vec![root];
        while let Some(entity) = stack.pop() {
            if world.get::<BlockText>(entity).is_some() {
                return Some(entity);
            }
            if let Some(children) = world.get::<Children>(entity) {
                stack.extend(children.iter());
            }
        }
        None
    }

    /// The first block from `from` on, wrapping around, that has text.
    fn next_with_text(&self, from: usize) -> usize {
        let len = self.markdown().document().len();
        (0..len)
            .map(|k| (from + k) % len)
            .find(|&b| self.try_text_of(b).is_some())
            .expect("a block with text")
    }

    fn with<C: Component>(&mut self) -> Vec<Entity> {
        let world = self.app.world_mut();
        world
            .query_filtered::<Entity, With<C>>()
            .iter(world)
            .collect()
    }

    /// Where the lines of the block break, and how big the block came out.
    fn lines(&self, block: usize) -> (Vec<std::ops::Range<usize>>, Vec2, f32) {
        let text = self.text_of(block);
        let world = self.app.world();
        let layout = world.get::<ComputedTextBlock>(text).unwrap().buffer();
        let lines = layout.lines().map(|line| line.text_range()).collect();
        let size = world.get::<ComputedNode>(text).unwrap().size;
        (lines, size, layout.height())
    }
}

/// Paragraphs long enough to wrap, with styled runs in them, and a code block.
fn wrapping_source() -> String {
    let words = "lorem ipsum dolor sit amet consectetur adipiscing elit sed do eiusmod ".repeat(14);
    format!(
        "# A heading that is long enough to be worth a look at its lines\n\n\
         {words}**bold words in the middle** and then {words}\n\n\
         - a list item with {words}\n- and another one\n\n\
         > a quote with {words}\n\n\
         {words}`code` and *italics* and {words}\n\n\
         ```\nfn main() {{}}\n```\n\n{words}\n"
    )
}

#[test]
fn only_the_block_with_the_caret_has_the_editor_components_and_the_pin() {
    let _serial = serial();
    let mut doc = Open::new(&synthetic::source(2_000), true);
    for block in [3, 8, 3, 20] {
        let block = doc.next_with_text(block);
        doc.caret_in(block, 1);
        let text = doc.text_of(block);
        let root = doc.markdown().block_entity(block).unwrap();
        assert_eq!(
            doc.with::<TextCursorStyle>(),
            vec![text],
            "caret in {block}"
        );
        assert_eq!(
            doc.with::<TextReadWriteMode>(),
            vec![text],
            "caret in {block}"
        );
        assert_eq!(doc.with::<Pinned>(), vec![root], "caret in {block}");
    }
}

#[test]
fn the_block_of_the_caret_stays_alive_when_scrolled_away() {
    let _serial = serial();
    let mut doc = Open::new(&synthetic::source(2_000), true);
    let block = doc.next_with_text(3);
    doc.caret_in(block, 1);
    let root = doc.markdown().block_entity(block).unwrap();
    set_scroll(&mut doc.app, doc.area, 60_000.0);
    settle(&mut doc.app);
    assert_eq!(doc.markdown().block_entity(block), Some(root));
    assert!(doc.markdown().live_blocks().count() < 100);
    // Back where it was: the very same entity, with the editor on it.
    set_scroll(&mut doc.app, doc.area, 0.0);
    settle(&mut doc.app);
    assert_eq!(doc.markdown().block_entity(block), Some(root));
    assert_eq!(doc.with::<Pinned>(), vec![root]);
}

#[test]
fn gaining_and_losing_the_editor_does_not_change_the_line_breaks() {
    let _serial = serial();
    // The same document with no editor in the app at all.
    let baseline = Open::new(&wrapping_source(), false);
    let mut doc = Open::new(&wrapping_source(), true);
    let blocks = doc.markdown().document().len();
    let (mut wrapped, mut laid_out) = (0, 0);
    for target in 0..blocks {
        if doc.try_text_of(target).is_none() {
            continue;
        }
        // The caret elsewhere: `target` is plain text.
        let elsewhere = doc.next_with_text(target + 1);
        doc.caret_in(elsewhere, 0);
        let text = doc.text_of(target);
        assert!(!doc.with::<TextCursorStyle>().contains(&text));
        let plain = doc.lines(target);
        assert_eq!(plain, baseline.lines(target), "block {target} plain");

        // The caret in it: it has the editor's components.
        laid_out = laid_out.max(doc.caret_in_one_frame(target, 0));
        assert!(doc.with::<TextCursorStyle>().contains(&text));
        assert_eq!(
            plain,
            doc.lines(target),
            "block {target} gaining the editor"
        );

        // The caret leaves again.
        doc.caret_in(elsewhere, 0);
        assert!(!doc.with::<TextCursorStyle>().contains(&text));
        assert_eq!(plain, doc.lines(target), "block {target} losing the editor");
        wrapped += usize::from(plain.0.len() > 1);
    }
    assert!(
        wrapped >= 4,
        "only {wrapped} blocks wrap: the test says little"
    );
    report("g6_blocks_compared_that_wrap", wrapped);
    // Bevy's reaction to the components appearing, measured, not assumed.
    report("g6_laid_out_when_a_block_gains_the_editor", laid_out);
    check("focus_laid_out_when_a_block_gains_the_editor", laid_out);
}

/// What opening a document of `blocks` blocks left allocated, per block.
fn retained_per_block(blocks: usize, editor: bool) -> f64 {
    let source = synthetic::source(blocks);
    let mut app = headless_app();
    app.update();
    app.add_message::<bevy::window::Ime>();
    if editor {
        app.add_plugins(MarkdownEditorPlugin);
    }
    app.update();
    let ((), alloc) = count_allocations(|| {
        let (_, markdown) = spawn_document(&mut app, &source);
        if editor {
            app.world_mut()
                .entity_mut(markdown)
                .insert(MarkdownEditor::new());
        }
        settle(&mut app);
    });
    alloc.retained.measure() / blocks.measure()
}

#[test]
fn opening_a_document_costs_the_same_with_or_without_the_editor() {
    let _serial = serial();
    let blocks = 20_000;
    let without = retained_per_block(blocks, false);
    let with = retained_per_block(blocks, true);
    report("editor_bytes_per_block_without", without);
    report("editor_bytes_per_block_with", with);
    check(
        "editor_bytes_per_block_overhead_20000",
        (with - without).abs(),
    );
}
