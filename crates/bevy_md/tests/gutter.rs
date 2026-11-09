//! The block controls are one entity for the whole document: none when the
//! pointer is away, exactly one beside the block under it, moved (never
//! duplicated) as the pointer travels.

mod common;

use bevy::prelude::*;
use bevy::ui::UiGlobalTransform;
use bevy_md::core::synthetic;
use bevy_md::{GUTTER_WIDTH, GutterControls, GutterPart, GutterPointer, Markdown, MarkdownGutter};
use common::*;

struct Open {
    app: App,
    area: Entity,
    markdown: Entity,
}

impl Open {
    fn new(blocks: usize, gutter: bool) -> Self {
        let mut app = headless_app();
        app.update();
        let (area, markdown) = spawn_document(&mut app, &synthetic::source(blocks));
        if gutter {
            app.world_mut().entity_mut(markdown).insert(MarkdownGutter);
        }
        settle(&mut app);
        Self {
            app,
            area,
            markdown,
        }
    }

    fn markdown(&self) -> &Markdown {
        self.app
            .world()
            .get::<Markdown>(self.markdown)
            .expect("markdown")
    }

    fn count<C: Component>(&mut self) -> usize {
        let world = self.app.world_mut();
        world.query_filtered::<(), With<C>>().iter(world).count()
    }

    fn controls(&mut self) -> Vec<(Entity, GutterControls)> {
        let world = self.app.world_mut();
        world
            .query::<(Entity, &GutterControls)>()
            .iter(world)
            .map(|(entity, controls)| (entity, *controls))
            .collect()
    }

    /// The pointer at `position`, in window pixels, and one frame.
    fn point(&mut self, position: Option<Vec2>) {
        self.app
            .world_mut()
            .insert_resource(GutterPointer(position));
        self.app.update();
    }

    /// The middle of block `block` on screen, at x = 500.
    fn over_block(&self, block: usize) -> Vec2 {
        let entity = self.markdown().block_entity(block).expect("a live block");
        let at = self.app.world().get::<UiGlobalTransform>(entity).unwrap();
        Vec2::new(500.0, at.translation.y)
    }
}

#[test]
fn there_are_no_controls_until_the_pointer_is_over_a_block() {
    let _serial = serial();
    let mut doc = Open::new(2_000, true);
    assert_eq!(doc.count::<GutterControls>(), 0);
    doc.point(None);
    assert_eq!(doc.count::<GutterControls>(), 0);
    // Over the document, but in the padding above its first block.
    doc.point(Some(Vec2::new(500.0, 5.0)));
    assert_eq!(doc.count::<GutterControls>(), 0);

    let target = doc.over_block(4);
    doc.point(Some(target));
    let controls = doc.controls();
    assert_eq!(controls.len(), 1);
    assert_eq!(controls[0].1.block, 4);
}

#[test]
fn the_controls_move_with_the_pointer_and_are_never_duplicated() {
    let _serial = serial();
    let mut doc = Open::new(2_000, true);
    let first = doc.over_block(2);
    doc.point(Some(first));
    let (entity, _) = doc.controls()[0];
    let parts = doc.count::<GutterPart>();
    assert!(parts > 1, "the controls have a button and a handle");

    let mut most = 1;
    let mut seen = std::collections::BTreeSet::new();
    // Blocks on screen: the window also holds blocks above and below it,
    // which the scroll area clips and the pointer cannot be over.
    let live: Vec<usize> = doc
        .markdown()
        .live_blocks()
        .map(|(ix, _)| ix)
        .filter(|&ix| (1.0..VIEWPORT.y - 1.0).contains(&doc.over_block(ix).y))
        .collect();
    assert!(live.len() >= 10, "{} blocks on screen", live.len());
    for &block in live.iter().take(30) {
        let target = doc.over_block(block);
        doc.point(Some(target));
        let controls = doc.controls();
        most = most.max(controls.len());
        assert_eq!(controls.len(), 1, "block {block}");
        assert_eq!(controls[0].0, entity, "the same entity follows the pointer");
        assert_eq!(controls[0].1.block, block);
        assert_eq!(doc.count::<GutterPart>(), parts, "block {block}");
        seen.insert(block);
        // And it sits beside the block: top matches the height index.
        let top = doc
            .app
            .world()
            .get::<Node>(entity)
            .map(|node| node.top)
            .unwrap();
        let padding = 20.0;
        assert_eq!(
            top,
            Val::Px(padding + doc.markdown().block_top(block) as f32),
            "block {block}"
        );
    }
    assert!(
        seen.len() >= 10,
        "the pointer visited {} blocks",
        seen.len()
    );
    report("gutter_entities_max", most);
    check("gutter_entities_max", most);
    report("gutter_parts", parts);
    check("gutter_parts", parts);
}

#[test]
fn the_controls_go_when_the_pointer_leaves_the_document() {
    let _serial = serial();
    let mut doc = Open::new(2_000, true);
    let target = doc.over_block(3);
    doc.point(Some(target));
    assert_eq!(doc.count::<GutterControls>(), 1);
    let parts = doc.count::<GutterPart>();
    assert!(parts > 1);

    // Outside the window.
    doc.point(None);
    assert_eq!(doc.count::<GutterControls>(), 0);
    assert_eq!(
        doc.count::<GutterPart>(),
        0,
        "all of the controls, not just the root"
    );
    assert!(
        !doc.app
            .world()
            .get::<Children>(doc.markdown)
            .unwrap()
            .iter()
            .any(|child| doc.app.world().get::<GutterControls>(child).is_some())
    );

    // Back, then off the right edge of the document (the node is 960 wide).
    doc.point(Some(target));
    assert_eq!(doc.count::<GutterControls>(), 1);
    doc.point(Some(Vec2::new(955.0, target.y)));
    assert_eq!(
        doc.count::<GutterControls>(),
        1,
        "still inside the document"
    );
    doc.point(Some(Vec2::new(990.0, target.y)));
    assert_eq!(doc.count::<GutterControls>(), 0);

    // The margin the controls hang in counts as the document.
    doc.point(Some(Vec2::new(5.0, target.y)));
    assert_eq!(doc.count::<GutterControls>(), 1);
    doc.point(Some(Vec2::new(-GUTTER_WIDTH - 10.0, target.y)));
    assert_eq!(doc.count::<GutterControls>(), 0);

    // Below the viewport: out of the scroll area that clips the document.
    doc.point(Some(target));
    assert_eq!(doc.count::<GutterControls>(), 1);
    doc.point(Some(Vec2::new(500.0, 900.0)));
    assert_eq!(doc.count::<GutterControls>(), 0);
}

#[test]
fn scrolling_under_a_still_pointer_moves_the_controls_to_the_new_block() {
    let _serial = serial();
    let mut doc = Open::new(2_000, true);
    let target = Vec2::new(500.0, 300.0);
    doc.point(Some(target));
    let (entity, before) = doc.controls()[0];

    for y in [900.0, 5_000.0, 40_000.0, 100.0] {
        set_scroll(&mut doc.app, doc.area, y);
        doc.app.update();
        settle(&mut doc.app);
        doc.point(Some(target));
        let controls = doc.controls();
        assert_eq!(controls.len(), 1, "scroll to {y}");
        assert_eq!(controls[0].0, entity);
        assert!(
            doc.markdown().block_entity(controls[0].1.block).is_some(),
            "the controls are beside a block that has a node"
        );
    }
    let (_, after) = doc.controls()[0];
    assert_ne!(before.block, after.block);
}

#[test]
fn a_document_without_the_marker_never_gets_controls() {
    let _serial = serial();
    let mut doc = Open::new(2_000, false);
    let target = doc.over_block(4);
    doc.point(Some(target));
    assert_eq!(doc.count::<GutterControls>(), 0);
}

#[test]
fn taking_the_marker_away_removes_the_controls() {
    let _serial = serial();
    let mut doc = Open::new(2_000, true);
    let target = doc.over_block(4);
    doc.point(Some(target));
    assert_eq!(doc.count::<GutterControls>(), 1);
    doc.app
        .world_mut()
        .entity_mut(doc.markdown)
        .remove::<MarkdownGutter>();
    doc.app.update();
    assert_eq!(doc.count::<GutterControls>(), 0);
    assert_eq!(doc.count::<GutterPart>(), 0);
}

#[test]
fn controls_on_screen_cost_nothing_while_the_pointer_rests() {
    let _serial = serial();
    let mut doc = Open::new(2_000, true);
    let target = doc.over_block(4);
    doc.point(Some(target));
    settle(&mut doc.app);
    for _ in 0..10 {
        doc.app.update();
        let stats = *doc.app.world().resource::<FrameStats>();
        assert_eq!(stats.spawned + stats.despawned, 0);
        assert_eq!(stats.laid_out, 0);
        assert_eq!(doc.count::<GutterControls>(), 1);
    }
}
