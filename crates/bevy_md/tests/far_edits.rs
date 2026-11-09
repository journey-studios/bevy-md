//! Changes far apart in the block list (a link definition at the end of a
//! note, the paragraph that uses it at the top) reach the nodes as the
//! disjoint splices they are: the blocks between keep their entities, their
//! measured heights, the scroll anchor and the pins, and nothing is laid out,
//! written or estimated for them, however many blocks there are.
//!
//! The note is `Top [shared]`, a run of one-line paragraphs, `Bottom
//! [shared]`: adding `[shared]: /dest` at the end changes the two mentions
//! and no block between. Sizes are 200, 2000 and 120 000 blocks.

mod common;

use std::collections::BTreeMap;
use std::fmt::Write;

use bevy::prelude::*;
use bevy::ui::{ComputedNode, ScrollPosition, UiGlobalTransform};
use bevy_md::core::Document;
use bevy_md::{BlockSurroundings, Markdown, Pinned};
use common::*;

const DEFINITION: &str = "\n[shared]: /dest\n";

/// The sizes the budgets are checked at, as paragraphs between the mentions
/// (the note has two more blocks).
const SIZES: [usize; 3] = [198, 1_998, 119_998];

fn source(paragraphs: usize) -> String {
    let mut out = String::from("Top [shared] mention of the label.\n\n");
    for n in 0..paragraphs {
        let _ = write!(out, "Filler paragraph {n} of the document.\n\n");
    }
    out.push_str("Bottom [shared] mention of the label.\n\n");
    out
}

struct Far {
    app: App,
    area: Entity,
    markdown: Entity,
}

/// What one frame did, the changes made before it included.
struct Frame {
    alloc: AllocStats,
    stats: FrameStats,
    writes: Writes,
}

impl Far {
    /// The note, opened and settled, with block `at` (when given) at the
    /// top of the screen.
    fn open(paragraphs: usize, at: Option<usize>) -> Self {
        Self::open_source(&source(paragraphs), at)
    }

    fn open_source(source: &str, at: Option<usize>) -> Self {
        let mut app = headless_app();
        app.update();
        let (area, markdown) = spawn_document(&mut app, source);
        settle(&mut app);
        let mut far = Self {
            app,
            area,
            markdown,
        };
        if let Some(block) = at {
            far.scroll_to_block(block);
        }
        far
    }

    fn markdown(&self) -> &Markdown {
        self.app
            .world()
            .get::<Markdown>(self.markdown)
            .expect("markdown")
    }

    fn scroll_to_block(&mut self, block: usize) {
        let y = self.markdown().block_top(block) as f32 + 20.0;
        set_scroll(&mut self.app, self.area, y);
        settle(&mut self.app);
    }

    fn scroll_y(&self) -> f32 {
        self.app
            .world()
            .get::<ScrollPosition>(self.area)
            .expect("scroll position")
            .y
    }

    fn live(&self) -> Vec<(usize, Entity)> {
        self.markdown().live_blocks().collect()
    }

    /// Height of block `block`, estimated or measured.
    fn height(&self, block: usize) -> f64 {
        self.markdown().block_top(block + 1) - self.markdown().block_top(block)
    }

    fn change(&mut self, change: impl FnOnce(&mut Markdown)) {
        let mut entity = self.app.world_mut().entity_mut(self.markdown);
        change(&mut entity.get_mut::<Markdown>().expect("markdown"));
    }

    /// Make `change` and run one frame, counting what it allocated.
    fn frame(&mut self, change: impl FnOnce(&mut Markdown)) -> Frame {
        let markdown = self.markdown;
        let ((), alloc) = count_allocations(|| {
            let mut entity = self.app.world_mut().entity_mut(markdown);
            change(&mut entity.get_mut::<Markdown>().expect("markdown"));
            self.app.update();
        });
        Frame {
            alloc,
            stats: *self.app.world().resource::<FrameStats>(),
            writes: self.app.world().resource::<Writes>().clone(),
        }
    }

    fn append_definition(&mut self) -> Frame {
        self.frame(|m| {
            let end = m.source().len();
            m.edit(end..end, DEFINITION);
        })
    }

    /// The text a block's subtree shows: its `Text` and spans, in order.
    fn shown_text(&self, entity: Entity) -> String {
        fn collect(world: &World, entity: Entity, out: &mut String) {
            if let Some(text) = world.get::<Text>(entity) {
                out.push_str(&text.0);
            }
            if let Some(span) = world.get::<TextSpan>(entity) {
                out.push_str(&span.0);
            }
            if let Some(children) = world.get::<Children>(entity) {
                for child in children.iter() {
                    collect(world, child, out);
                }
            }
        }
        let mut out = String::new();
        collect(self.app.world(), entity, &mut out);
        out
    }

    /// Every live block shows its block of the document, and says so.
    #[track_caller]
    fn assert_shows_the_document(&self, context: &str) {
        let markdown = self.markdown();
        for (index, entity) in markdown.live_blocks() {
            let block = markdown
                .document()
                .block(index)
                .unwrap_or_else(|| panic!("{context}: live block {index} is not in the document"));
            assert_eq!(
                self.shown_text(entity),
                block.content.text,
                "{context}: block {index} shows other text than the document's"
            );
            let surroundings = self
                .app
                .world()
                .get::<BlockSurroundings>(entity)
                .unwrap_or_else(|| panic!("{context}: block {index} has no surroundings"));
            assert_eq!(surroundings.index, index, "{context}: surroundings");
        }
        assert!(
            (markdown.block_top(markdown.document().len()) - markdown.content_height()).abs()
                < 1e-6,
            "{context}: the heights cover another number of blocks than the document has"
        );
    }

    #[track_caller]
    fn assert_equals_a_full_parse(&self, context: &str) {
        let doc = self.markdown().document();
        let full = Document::parse(doc.source());
        assert!(
            doc.blocks().eq(full.blocks()),
            "{context}: the document differs from a full parse"
        );
    }

    /// Top of block `block` on screen, relative to the top of the scroll
    /// area, in logical pixels.
    fn screen_top(&self, block: usize) -> Option<f32> {
        let world = self.app.world();
        let entity = self.markdown().block_entity(block)?;
        let node = world.get::<ComputedNode>(entity)?;
        let at = world.get::<UiGlobalTransform>(entity)?;
        let area_node = world.get::<ComputedNode>(self.area)?;
        let area_at = world.get::<UiGlobalTransform>(self.area)?;
        Some(
            (at.translation.y - node.size.y / 2.0)
                - (area_at.translation.y - area_node.size.y / 2.0),
        )
    }

    /// Text colored as a link in the subtree of `entity`.
    fn links_in(&self, entity: Entity) -> usize {
        let world = self.app.world();
        let link = test_theme().link;
        let mut count = 0;
        let mut stack = vec![entity];
        while let Some(e) = stack.pop() {
            if world.get::<TextColor>(e).is_some_and(|c| c.0 == link) {
                count += 1;
            }
            if let Some(children) = world.get::<Children>(e) {
                stack.extend(children.iter());
            }
        }
        count
    }
}

/// The blocks that are live, by index, with their entity and height.
fn live_with_heights(far: &Far) -> BTreeMap<usize, (Entity, f64)> {
    far.live()
        .into_iter()
        .map(|(ix, entity)| (ix, (entity, far.height(ix))))
        .collect()
}

/// The blocks on screen before are the same blocks `shift` places later,
/// with the same entities and heights. The window around the screen is
/// computed before layout, with the scroll position of the frame before, so
/// when blocks were added above the screen it can sit a block or two off for
/// that one frame (the scroll follows its anchor right after, see `measure`):
/// `slack` blocks may enter or leave at its edges, and no more.
#[track_caller]
fn assert_same_live(
    far: &Far,
    before: &BTreeMap<usize, (Entity, f64)>,
    shift: usize,
    slack: usize,
    context: &str,
) {
    let after = live_with_heights(far);
    let (mut left, mut entered) = (0, 0);
    for (index, (entity, height)) in before {
        match after.get(&(index + shift)) {
            Some((now, now_height)) => {
                assert_eq!(now, entity, "{context}: block {index} lost its entity");
                assert!(
                    (now_height - height).abs() < 1e-6,
                    "{context}: block {index} changed height from {height} to {now_height}"
                );
            }
            None => left += 1,
        }
    }
    for index in after.keys() {
        if index
            .checked_sub(shift)
            .is_none_or(|b| !before.contains_key(&b))
        {
            entered += 1;
        }
    }
    assert!(
        left <= slack && entered <= slack,
        "{context}: before {:?}..{:?}, after {:?}..{:?}",
        before.keys().next(),
        before.keys().last(),
        after.keys().next(),
        after.keys().last()
    );
    assert!(
        left <= slack && entered <= slack,
        "{context}: {left} blocks left the screen's window and {entered} entered (at most {slack})"
    );
}

// ---------------------------------------------------------------------------
// The definition at the end, the mentions elsewhere
// ---------------------------------------------------------------------------

/// Add the definition with the screen in the middle of the note and report
/// the frame. Nothing on screen changed, so nothing on screen is touched.
fn definition_with_the_screen_in_the_middle(paragraphs: usize) -> Frame {
    let _serial = serial();
    let blocks = paragraphs + 2;
    let mut far = Far::open(paragraphs, Some(blocks / 2));
    // A first edit grows the source's buffer once, whatever the document:
    // the frames measured are the ones after it.
    far.change(|m| {
        let at = m
            .source()
            .find("Filler paragraph 7 of")
            .expect("a paragraph");
        m.edit(at..at, "y");
    });
    settle(&mut far.app);
    let live = live_with_heights(&far);
    assert!(live.len() > 20, "{} live blocks", live.len());
    assert!(
        !live.contains_key(&0) && !live.contains_key(&(blocks - 1)),
        "the mentions are away from the screen at {blocks} blocks"
    );
    let anchor = far.markdown().scroll_anchor().expect("an anchor");
    let scroll = far.scroll_y();
    let total = far.markdown().content_height();
    let on_screen = far
        .screen_top(anchor.block)
        .expect("the anchor is on screen");

    let frame = far.append_definition();

    // Both mentions were reparsed, and nothing else.
    let doc = far.markdown().document();
    assert_eq!(doc.len(), blocks);
    assert_eq!(doc.block(0).unwrap().content.links.len(), 1);
    assert_eq!(doc.block(blocks - 1).unwrap().content.links.len(), 1);
    assert_eq!(
        doc.stats().full_parses,
        1,
        "only the opening was a full parse"
    );

    // The blocks on screen are the same entities with the same measured
    // heights, at the same indices, and none was written or laid out again.
    assert_same_live(&far, &live, 0, 0, &format!("{blocks} blocks"));
    assert_eq!(
        frame.stats.spawned + frame.stats.despawned,
        0,
        "{blocks} blocks"
    );
    assert_eq!(frame.stats.laid_out, 0, "{blocks} blocks");
    assert!(
        frame.writes.all.is_empty(),
        "{blocks} blocks: written {:?}",
        frame.writes.all
    );
    // The anchor and the scroll position did not move, and the heights add
    // up as before: the mentions kept the height they had.
    assert_eq!(far.markdown().scroll_anchor(), Some(anchor));
    assert!((far.scroll_y() - scroll).abs() < 0.01);
    assert!((far.markdown().content_height() - total).abs() < 1e-6);
    let after = far.screen_top(anchor.block).expect("still on screen");
    assert!((after - on_screen).abs() < 0.01, "{on_screen} -> {after}");
    far.assert_shows_the_document("after the definition");
    far.assert_equals_a_full_parse("after the definition");

    // Scrolling to the top finds the mention drawn as a link.
    far.scroll_to_block(0);
    let top = far.markdown().block_entity(0).expect("on screen");
    assert!(far.links_in(top) >= 1, "the top mention is a link");
    far.assert_shows_the_document("at the top");
    frame
}

#[test]
fn a_definition_at_the_end_leaves_the_screen_alone_at_any_size() {
    let frames: Vec<(usize, Frame)> = SIZES
        .iter()
        .map(|&n| (n + 2, definition_with_the_screen_in_the_middle(n)))
        .collect();
    let (small, big) = (&frames[0].1, &frames[2].1);
    check("far_definition_frame_allocations", big.alloc.allocations);
    check("far_definition_frame_bytes", big.alloc.bytes);
    // Independent of the absolute limits: 600 times the blocks cost a few
    // allocations at most (the depth of the trees), and the same bytes.
    for (blocks, frame) in &frames {
        assert!(
            frame.alloc.allocations <= small.alloc.allocations + 16,
            "{blocks} blocks: {} allocations against {} at 200",
            frame.alloc.allocations,
            small.alloc.allocations
        );
        assert!(
            frame.alloc.bytes <= small.alloc.bytes + small.alloc.bytes / 10 + 4096,
            "{blocks} blocks: {} bytes against {} at 200",
            frame.alloc.bytes,
            small.alloc.bytes
        );
    }
}

/// With the mention on screen, only it changes: the blocks around it keep
/// their entities and are not written.
#[test]
fn a_mention_on_screen_is_rebuilt_and_its_neighbours_are_not() {
    let _serial = serial();
    for paragraphs in [198, 119_998] {
        let mut far = Far::open(paragraphs, None);
        let before: BTreeMap<usize, Entity> = far.live().into_iter().collect();
        assert!(before.contains_key(&0) && before.len() > 20);
        let frame = far.append_definition();
        let after: BTreeMap<usize, Entity> = far.live().into_iter().collect();
        assert_eq!(
            after.keys().collect::<Vec<_>>(),
            before.keys().collect::<Vec<_>>()
        );
        for (index, entity) in &before {
            if *index != 0 {
                assert_eq!(after[index], *entity, "block {index} lost its entity");
                assert!(
                    !frame.writes.all.contains(entity),
                    "block {index} was written"
                );
            }
        }
        // The mention changed shape (it has a link run now) and laid out; the
        // only block that did.
        let mention = after[&0];
        assert!(far.links_in(mention) >= 1);
        assert!(
            frame.stats.laid_out <= 2,
            "{} laid out",
            frame.stats.laid_out
        );
        far.assert_shows_the_document("mention on screen");
        far.assert_equals_a_full_parse("mention on screen");
    }
}

// ---------------------------------------------------------------------------
// Several edits before one frame, changing the number of blocks
// ---------------------------------------------------------------------------

/// Four changes made one after another before the plugin runs: two
/// paragraphs inserted near the top, the definition added at the end, a
/// paragraph deleted far below the screen, and a keystroke in a block on
/// screen. The blocks on screen move by the two that came before them, keep
/// their entities and heights, and only the block typed in is written.
fn accumulated_edits(paragraphs: usize) {
    let _serial = serial();
    let blocks = paragraphs + 2;
    let mut far = Far::open(paragraphs, Some(blocks / 2));
    let live = live_with_heights(&far);
    let typed = *live.keys().nth(live.len() / 2).expect("a live block");
    let anchor = far.markdown().scroll_anchor().expect("an anchor");
    let on_screen = far.screen_top(anchor.block).expect("on screen");

    let frame = far.frame(|m| {
        // 1. The definition, at the end of the source as it is now.
        let end = m.source().len();
        m.edit(end..end, DEFINITION);
        // 2. A paragraph far below the screen goes away (-1 block); its
        //    position is read from the source, not the document, since the
        //    document already moved on.
        let marker = format!("Filler paragraph {} of", paragraphs - 3);
        let at = m.source().find(&marker).expect("the paragraph");
        let end = at + m.source()[at..].find("\n\n").unwrap() + 2;
        m.edit(at..end, "");
        // 3. Two paragraphs after the first block (+2 blocks), after the two
        //    edits that are below them in the document: the splices already
        //    in the list move down by the two.
        let at = m.document().block(1).unwrap().span().start;
        m.edit(at..at, "Added one.\n\nAdded two.\n\n");
        // 4. A keystroke in a block on screen (index now shifted by 2).
        let at = m.document().block(typed + 2).unwrap().span().start + 2;
        m.edit(at..at, "x");
    });

    let context = format!("{blocks} blocks");
    assert_eq!(far.markdown().document().len(), blocks + 2 - 1);
    far.assert_equals_a_full_parse(&context);
    far.assert_shows_the_document(&context);
    assert!(far.markdown().is_rendered());

    // Blocks that stayed: the same entities two places later, same heights.
    assert_same_live(&far, &live, 2, 4, &context);
    // Nothing spawned or despawned (the blocks that entered at the edge of
    // the window took subtrees from the pool), and the only text written is
    // the typed block's and the entering blocks'.
    assert_eq!(frame.stats.spawned + frame.stats.despawned, 0, "{context}");
    let typed_entity = live[&typed].0;
    let entering: Vec<Entity> = far
        .live()
        .into_iter()
        .filter(|(ix, _)| !live.contains_key(&(ix - 2)))
        .map(|(_, e)| e)
        .collect();
    assert!(
        frame.stats.laid_out <= 2 + 2 * entering.len(),
        "{context}: {} laid out, {} blocks entered",
        frame.stats.laid_out,
        entering.len()
    );
    for written in &frame.writes.texts {
        let mut e = *written;
        let mut under = e == typed_entity || entering.contains(&e);
        while !under {
            let Some(parent) = far.app.world().get::<ChildOf>(e) else {
                break;
            };
            e = parent.parent();
            under = e == typed_entity || entering.contains(&e);
        }
        assert!(
            under,
            "{context}: {written} outside the typed and entering blocks was written"
        );
    }
    assert!(
        frame.writes.surroundings.len() <= live.len() + 8,
        "{context}: {} surroundings written for {} live blocks",
        frame.writes.surroundings.len(),
        live.len()
    );
    // The anchor followed its block and the screen did not jump.
    settle(&mut far.app);
    let moved = far.markdown().scroll_anchor().expect("an anchor");
    assert_eq!(moved.block, anchor.block + 2, "{context}");
    assert!((moved.offset - anchor.offset).abs() < 0.01);
    let after = far.screen_top(moved.block).expect("on screen");
    assert!(
        (after - on_screen).abs() < 1.5,
        "{context}: the anchor moved on screen from {on_screen} to {after}"
    );
    // The new heights are for the right blocks: scrolling back to the top
    // shows the mention as a link and the inserted paragraphs as text.
    far.scroll_to_block(0);
    far.assert_shows_the_document("top after accumulated edits");
    let top = far.markdown().block_entity(0).unwrap();
    assert!(far.links_in(top) >= 1);
}

#[test]
fn edits_accumulated_before_a_frame_compose_when_the_block_count_changes() {
    for paragraphs in SIZES {
        accumulated_edits(paragraphs);
    }
}

/// A paragraph split in two above the screen, the definition added at the
/// end and two paragraphs joined next to it: unions and separations of
/// blocks, in one frame. The screen's blocks move by the one the split
/// added; the join below changes nothing for them.
#[test]
fn splitting_and_joining_blocks_between_far_changes() {
    let _serial = serial();
    for paragraphs in [198, 19_998] {
        let blocks = paragraphs + 2;
        let mut far = Far::open(paragraphs, Some(blocks / 2));
        let live = live_with_heights(&far);
        let frame = far.frame(|m| {
            // Split the first filler paragraph in two (+1 block).
            let at = m.source().find("Filler paragraph 0 of").unwrap() + 7;
            m.edit(at..at, "\n\n");
            // The definition, at the end.
            let end = m.source().len();
            m.edit(end..end, DEFINITION);
            // Join the last filler paragraph with the mention below it (-1).
            let at = m.source().find("Bottom [shared]").unwrap() - 2;
            m.edit(at..at + 2, " ");
        });
        let context = format!("{blocks} blocks");
        assert_eq!(far.markdown().document().len(), blocks, "{context}");
        far.assert_equals_a_full_parse(&context);
        far.assert_shows_the_document(&context);
        assert_same_live(&far, &live, 1, 4, &context);
        assert_eq!(frame.stats.spawned + frame.stats.despawned, 0, "{context}");
        assert!(
            frame.stats.laid_out <= 8,
            "{context}: {}",
            frame.stats.laid_out
        );
    }
}

// ---------------------------------------------------------------------------
// Pins
// ---------------------------------------------------------------------------

/// A pinned block far from the screen is one more block: a far definition
/// neither moves it nor frees it, and when the edit replaces it, its
/// replacement is pinned.
#[test]
fn pins_survive_far_changes_and_follow_replacements() {
    let _serial = serial();
    for paragraphs in [198, 119_998] {
        let blocks = paragraphs + 2;
        let mut far = Far::open(paragraphs, None);
        // Pin the top mention (it will be replaced) and a filler block on
        // screen (it will only move), then go to the middle.
        let mention = far.markdown().block_entity(0).expect("live");
        let filler = far.markdown().block_entity(5).expect("live");
        far.app.world_mut().entity_mut(mention).insert(Pinned);
        far.app.world_mut().entity_mut(filler).insert(Pinned);
        far.scroll_to_block(blocks / 2);
        let filler_height = far.height(5);
        assert_eq!(far.markdown().block_entity(5), Some(filler));
        assert!(far.markdown().block_entity(0).is_some(), "pinned and far");

        // The definition lands, and two paragraphs above the pinned filler.
        let frame = far.frame(|m| {
            let end = m.source().len();
            m.edit(end..end, DEFINITION);
            let at = m.document().block(3).unwrap().span().start;
            m.edit(at..at, "Added one.\n\nAdded two.\n\n");
        });
        let context = format!("{blocks} blocks");
        // The filler was kept: same entity two places later, still pinned,
        // still measured.
        assert_eq!(far.markdown().block_entity(7), Some(filler), "{context}");
        assert!(far.app.world().get::<Pinned>(filler).is_some(), "{context}");
        assert!((far.height(7) - filler_height).abs() < 1e-9, "{context}");
        // The mention was replaced and its replacement is pinned, as a link.
        let replacement = far.markdown().block_entity(0).expect("still live");
        assert!(
            far.app.world().get::<Pinned>(replacement).is_some(),
            "{context}"
        );
        assert!(far.links_in(replacement) >= 1, "{context}");
        assert!(
            frame.stats.laid_out <= 2 + 1 + 1,
            "{context}: {} laid out",
            frame.stats.laid_out
        );
        far.assert_shows_the_document(&context);
        far.assert_equals_a_full_parse(&context);

        // Unpinned, they are blocks like the rest.
        far.app
            .world_mut()
            .entity_mut(replacement)
            .remove::<Pinned>();
        far.app.world_mut().entity_mut(filler).remove::<Pinned>();
        far.app.update();
        assert_eq!(far.markdown().block_entity(0), None, "{context}");
        assert_eq!(far.markdown().block_entity(7), None, "{context}");
    }
}

// ---------------------------------------------------------------------------
// Streaming
// ---------------------------------------------------------------------------

/// Streaming into the end of a note whose first paragraph mentions a label
/// the last chunk defines: every frame costs the same at 200 and at 120 000
/// blocks, and the frame that defines it reaches the mention at the top
/// without touching the blocks in between.
fn streamed_definition(paragraphs: usize) -> (Frame, Frame) {
    let _serial = serial();
    let blocks = paragraphs + 1;
    let mut text = String::from("Top [shared] mention of the label.\n\n");
    for n in 0..paragraphs {
        let _ = write!(text, "Filler paragraph {n} of the document.\n\n");
    }
    let mut far = Far::open_source(&text, Some(blocks - 20));
    let live = far.live();
    assert!(
        live.iter().all(|(ix, _)| *ix > 10),
        "away from the top mention"
    );
    let chunks = [
        "Streamed paragraph one, ",
        "continuing with more words.\n\n",
        "A second one",
        " ends here.\n\n",
    ];
    let mut typical = None;
    for (n, chunk) in chunks.iter().enumerate() {
        let frame = far.frame(|m| m.push_str(chunk));
        far.assert_shows_the_document(&format!("chunk {n}"));
        assert_eq!(frame.stats.despawned, 0, "chunk {n}");
        typical = Some(frame);
    }
    let last = far.frame(|m| m.push_str(DEFINITION));
    let context = format!("{blocks} blocks");
    far.assert_shows_the_document(&format!("{context}: definition"));
    far.assert_equals_a_full_parse(&format!("{context}: definition"));
    assert_eq!(
        far.markdown()
            .document()
            .block(0)
            .unwrap()
            .content
            .links
            .len(),
        1
    );
    // The blocks on screen were not touched by the frame that defined the
    // label far above them.
    assert_eq!(last.stats.spawned + last.stats.despawned, 0, "{context}");
    assert_eq!(last.stats.laid_out, 0, "{context}");
    assert!(
        last.writes.all.is_empty(),
        "{context}: {:?}",
        last.writes.all
    );
    for (index, entity) in live {
        assert_eq!(
            far.markdown().block_entity(index),
            Some(entity),
            "{context}"
        );
    }
    (typical.unwrap(), last)
}

#[test]
fn streaming_a_definition_reaches_the_top_without_touching_the_rest() {
    let sizes: Vec<_> = SIZES.iter().map(|&n| streamed_definition(n)).collect();
    let (small, big) = (&sizes[0], &sizes[2]);
    check(
        "far_definition_stream_frame_allocations",
        big.1.alloc.allocations,
    );
    check("far_definition_stream_frame_bytes", big.1.alloc.bytes);
    for (chunk, (typical, last)) in sizes.iter().enumerate() {
        assert!(
            last.alloc.allocations <= small.1.alloc.allocations + 16,
            "size {chunk}: {} allocations in the defining frame against {} at 200",
            last.alloc.allocations,
            small.1.alloc.allocations
        );
        assert!(
            typical.alloc.allocations <= small.0.alloc.allocations + 16,
            "size {chunk}: a streamed chunk costs {} allocations against {} at 200",
            typical.alloc.allocations,
            small.0.alloc.allocations
        );
    }
}

// ---------------------------------------------------------------------------
// Random: any order of edits, scrolls and pins
// ---------------------------------------------------------------------------

/// Edits (definitions added and removed, paragraphs inserted and deleted,
/// several per frame), scrolls and pins in any order: after every frame the
/// nodes show the document, and the document is a full parse of the source.
#[test]
fn random_far_edits_scrolls_and_pins_keep_the_nodes_on_the_document() {
    let _serial = serial();
    let mut state = 0xD1B5_4A32_D192_ED03u64;
    let mut next = move |n: u64| {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D) % n
    };
    let mut far = Far::open(1_500, None);
    let mut pinned: Vec<Entity> = Vec::new();
    for step in 0..300 {
        let edits = next(3);
        for _ in 0..edits {
            let choice = next(6);
            let n = next(1_500);
            far.change(|m| {
                let source = m.source();
                match choice {
                    0 if !source.contains("[shared]: /dest") => {
                        let end = source.len();
                        m.edit(end..end, DEFINITION);
                    }
                    1 => {
                        if let Some(at) = source.find("[shared]: /dest\n") {
                            m.edit(at..at + "[shared]: /dest\n".len(), "");
                        }
                    }
                    2 => {
                        let marker = format!("Filler paragraph {n} of");
                        if let Some(at) = source.find(&marker) {
                            m.edit(at..at, "Inserted [shared] and more.\n\n");
                        }
                    }
                    3 => {
                        let marker = format!("Filler paragraph {n} of");
                        if let Some(at) = source.find(&marker)
                            && let Some(len) = source[at..].find("\n\n")
                        {
                            m.edit(at..at + len + 2, "");
                        }
                    }
                    4 => {
                        let marker = format!("Filler paragraph {n} of");
                        if let Some(at) = source.find(&marker) {
                            m.edit(at + 3..at + 3, "x");
                        }
                    }
                    _ => {
                        // Join or split around a paragraph.
                        let marker = format!("Filler paragraph {n} of");
                        if let Some(at) = source.find(&marker)
                            && at >= 2
                            && source.as_bytes()[at - 1] == b'\n'
                        {
                            m.edit(at - 2..at, " ");
                        }
                    }
                }
            });
        }
        match next(8) {
            0..=2 => {
                let block = next(far.markdown().document().len() as u64) as usize;
                let y = far.markdown().block_top(block) as f32 + 20.0;
                set_scroll(&mut far.app, far.area, y);
            }
            3 => {
                let live = far.live();
                if !live.is_empty() {
                    let (_, entity) = live[next(live.len() as u64) as usize];
                    far.app.world_mut().entity_mut(entity).insert(Pinned);
                    pinned.push(entity);
                }
            }
            4 => {
                if let Some(entity) = pinned.pop()
                    && let Ok(mut entity) = far.app.world_mut().get_entity_mut(entity)
                {
                    entity.remove::<Pinned>();
                }
            }
            _ => {}
        }
        far.app.update();
        let context = format!("step {step}");
        far.assert_shows_the_document(&context);
        if step % 10 == 0 {
            far.assert_equals_a_full_parse(&context);
        }
    }
    settle(&mut far.app);
    far.assert_shows_the_document("settled");
    far.assert_equals_a_full_parse("settled");
}

// ---------------------------------------------------------------------------
// Async parse: a paste of more than SYNC_LIMIT bytes is parsed on a worker,
// which reports the splices against the document being shown
// ---------------------------------------------------------------------------

/// A paste of paragraphs, well over `Markdown::SYNC_LIMIT`, holding the
/// definition in its middle.
fn paste() -> String {
    let mut text = String::new();
    for n in 0..400 {
        let _ = write!(text, "Pasted paragraph {n} with some words in it.\n\n");
        if n == 200 {
            text.push_str("[shared]: /dest\n\n");
        }
    }
    assert!(
        text.len() > Markdown::SYNC_LIMIT * 4,
        "{} bytes",
        text.len()
    );
    text
}

impl Far {
    /// Run frames until the parse in flight has been installed; the frame
    /// that installed it is returned, and `quiet` sees every earlier one.
    fn install(&mut self, mut quiet: impl FnMut(&Far, &Frame)) -> Frame {
        for _ in 0..5_000 {
            let frame = self.frame(|_| {});
            if !self.markdown().is_parsing() {
                return frame;
            }
            quiet(self, &frame);
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the parse never landed");
    }
}

/// A paste at the end, holding the definition the first block uses: while
/// the worker parses nothing changes, and the frame that installs the result
/// replaces the mention and the paste, and leaves the blocks on screen (and
/// the 120000 ones around them) as they were.
fn paste_below_the_screen(paragraphs: usize) -> Frame {
    let _serial = serial();
    let blocks = paragraphs + 2;
    let mut far = Far::open(paragraphs, Some(blocks / 2));
    far.change(|m| {
        let at = m
            .source()
            .find("Filler paragraph 7 of")
            .expect("a paragraph");
        m.edit(at..at, "y");
    });
    settle(&mut far.app);
    let live = live_with_heights(&far);
    let context = format!("{blocks} blocks");

    let pasted = paste();
    let frame = far.frame(|m| {
        let end = m.source().len();
        m.edit(end..end, &pasted);
    });
    assert!(far.markdown().is_parsing(), "{context}");
    assert_eq!(frame.stats.spawned + frame.stats.laid_out, 0, "{context}");
    let installed = far.install(|far, frame| {
        assert_eq!(frame.stats.spawned + frame.stats.laid_out, 0);
        assert!(frame.writes.all.is_empty(), "{:?}", frame.writes.all);
        assert_eq!(
            far.markdown().document().len(),
            blocks,
            "the shown document waits for the result"
        );
    });

    assert_eq!(far.markdown().document().len(), blocks + 400, "{context}");
    assert_same_live(&far, &live, 0, 0, &context);
    assert_eq!(
        installed.stats.spawned + installed.stats.despawned,
        0,
        "{context}"
    );
    assert_eq!(installed.stats.laid_out, 0, "{context}");
    // Only the two spacers are resized (the document is taller).
    assert!(installed.writes.texts.is_empty(), "{context}");
    assert!(installed.writes.surroundings.is_empty(), "{context}");
    assert!(
        installed.writes.all.len() <= 2,
        "{context}: {:?}",
        installed.writes.all
    );
    assert_eq!(far.markdown().discarded_parses(), 0, "{context}");
    far.assert_equals_a_full_parse(&context);
    far.assert_shows_the_document(&context);
    // The mention at the top is a link now, and so is the one at the bottom
    // of the old text (between the first and the paste).
    far.scroll_to_block(0);
    let top = far.markdown().block_entity(0).unwrap();
    assert!(far.links_in(top) >= 1, "{context}");
    far.assert_shows_the_document(&format!("{context}: top"));
    installed
}

#[test]
fn a_pasted_definition_installs_without_touching_the_blocks_in_view() {
    // Bevy has a frame now and then with a couple of hundred allocations
    // that are not the document's (see the idle budgets): the best of three
    // runs is the frame that is.
    let frames: Vec<Frame> = SIZES
        .iter()
        .map(|&n| {
            (0..3)
                .map(|_| paste_below_the_screen(n))
                .min_by_key(|frame| frame.alloc.allocations)
                .unwrap()
        })
        .collect();
    let (small, big) = (&frames[0], &frames[2]);
    check("far_paste_install_frame_allocations", big.alloc.allocations);
    check("far_paste_install_frame_bytes", big.alloc.bytes);
    // The install frame varies by a dozen allocations from run to run (the
    // task's wake-up, Bevy's own systems); a cost that grew with 600 times
    // the blocks would be thousands.
    for frame in &frames {
        assert!(
            frame.alloc.allocations <= small.alloc.allocations + 40,
            "{} allocations against {} at 200 blocks",
            frame.alloc.allocations,
            small.alloc.allocations
        );
        assert!(
            frame.alloc.bytes <= small.alloc.bytes + small.alloc.bytes / 4,
            "{} bytes against {} at 200 blocks",
            frame.alloc.bytes,
            small.alloc.bytes
        );
    }
}

/// A paste above the screen: the blocks on screen move down by the blocks
/// pasted, and once the frames have settled the screen shows what it showed.
#[test]
fn a_paste_above_the_screen_is_followed_by_the_anchor() {
    for paragraphs in SIZES {
        let _serial = serial();
        let blocks = paragraphs + 2;
        let mut far = Far::open(paragraphs, Some(blocks / 2));
        let live = live_with_heights(&far);
        let anchor = far.markdown().scroll_anchor().expect("an anchor");
        let on_screen = far.screen_top(anchor.block).expect("on screen");
        let context = format!("{blocks} blocks");

        let pasted = paste();
        far.change(|m| {
            let at = m.document().block(1).unwrap().span().start;
            m.edit(at..at, &pasted);
        });
        assert!(far.markdown().is_parsing(), "{context}");
        far.install(|far, frame| {
            // Nothing moves until the result is there.
            assert_eq!(frame.stats.spawned + frame.stats.laid_out, 0);
            assert_eq!(far.markdown().document().len(), blocks);
        });
        settle(&mut far.app);

        assert_eq!(far.markdown().document().len(), blocks + 400, "{context}");
        far.assert_equals_a_full_parse(&context);
        far.assert_shows_the_document(&context);
        let moved = far.markdown().scroll_anchor().expect("an anchor");
        assert_eq!(moved.block, anchor.block + 400, "{context}");
        let after = far.screen_top(moved.block).expect("on screen");
        assert!(
            (after - on_screen).abs() < 1.5,
            "{context}: the anchor moved on screen from {on_screen} to {after}"
        );
        // The blocks that were on screen are the ones on screen, with the
        // heights they had; the window around the screen has the same size.
        let now = live_with_heights(&far);
        assert!(now.len() <= live.len() + 8, "{context}: {}", now.len());
        for (index, (_, height)) in &live {
            let (_, now_height) = now
                .get(&(index + 400))
                .unwrap_or_else(|| panic!("{context}: block {index} is not live after the paste"));
            assert!(
                (now_height - height).abs() < 1e-6,
                "{context}: block {index}"
            );
        }
    }
}

/// Two large pastes before the first lands, a keystroke on screen while the
/// second parses: the first result is thrown away, the second is installed
/// and the keystroke is applied after it, to the right block.
#[test]
fn a_newer_paste_and_a_keystroke_queued_behind_it_land_in_order() {
    for paragraphs in [198, 1_998] {
        let _serial = serial();
        let blocks = paragraphs + 2;
        let mut far = Far::open(paragraphs, Some(blocks / 2));
        far.change(|m| {
            let at = m
                .source()
                .find("Filler paragraph 7 of")
                .expect("a paragraph");
            m.edit(at..at, "y");
        });
        settle(&mut far.app);
        let live = live_with_heights(&far);
        let typed = *live.keys().nth(live.len() / 2).unwrap();
        let context = format!("{blocks} blocks");

        let pasted = paste();
        far.frame(|m| {
            let end = m.source().len();
            m.edit(end..end, &pasted);
        });
        assert!(far.markdown().is_parsing());
        far.frame(|m| {
            // A second paste, in the middle of the first one.
            let at = m.source().len() - pasted.len() / 2;
            let at = at + m.source()[at..].find("\n\n").unwrap() + 2;
            m.edit(at..at, &pasted);
        });
        let mut expected_len = 0;
        far.frame(|m| {
            // A keystroke in a block on screen, while the parse runs.
            let at = m.document().block(typed).unwrap().span().start + 2;
            m.edit(at..at, "k");
            expected_len = m.source().len();
        });
        assert!(far.markdown().is_parsing(), "{context}");
        far.install(|_, _| {});
        settle(&mut far.app);
        assert!(far.markdown().discarded_parses() >= 1, "{context}");
        assert_eq!(far.markdown().source().len(), expected_len, "{context}");
        assert!(!far.markdown().is_parsing());
        far.assert_equals_a_full_parse(&context);
        far.assert_shows_the_document(&context);
        // The mention at the top is replaced, above the screen and never
        // measured, so its height is an estimate that may differ by a pixel
        // from the one it had: the window can sit a block off by that much.
        assert_same_live(&far, &live, 0, 1, &context);
        let typed_text = far.shown_text(far.markdown().block_entity(typed).unwrap());
        assert!(typed_text.contains('k'), "{context}: {typed_text:?}");
    }
}
