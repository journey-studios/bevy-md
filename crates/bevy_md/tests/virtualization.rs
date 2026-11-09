//! Virtualization: only the blocks near the screen have entities, scrolling
//! spawns exactly the blocks that enter, and the block at the top of the
//! screen stays put when heights above it change.

mod common;

use std::collections::BTreeSet;

use bevy::prelude::*;
use bevy::ui::{ComputedNode, ScrollPosition, UiGlobalTransform};
use bevy_md::core::estimate::{Metrics, estimate_height};
use bevy_md::core::synthetic;
use bevy_md::{Markdown, Pinned};
use common::*;

struct Open {
    app: App,
    area: Entity,
    markdown: Entity,
}

impl Open {
    fn new(blocks: usize) -> Self {
        let mut app = headless_app();
        app.update();
        let (area, markdown) = spawn_document(&mut app, &synthetic::source(blocks));
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

    fn live(&self) -> BTreeSet<usize> {
        self.markdown().live_blocks().map(|(ix, _)| ix).collect()
    }

    fn stats(&self) -> FrameStats {
        *self.app.world().resource::<FrameStats>()
    }

    fn scroll_to(&mut self, y: f32) {
        set_scroll(&mut self.app, self.area, y);
    }

    fn scroll_y(&self) -> f32 {
        self.app
            .world()
            .get::<ScrollPosition>(self.area)
            .expect("scroll position")
            .y
    }

    /// UI nodes in the subtree of `entity`, itself included (text spans are
    /// not nodes).
    fn subtree(&self, entity: Entity) -> usize {
        let world = self.app.world();
        usize::from(world.get::<Node>(entity).is_some())
            + world.get::<Children>(entity).map_or(0, |children| {
                children.iter().map(|child| self.subtree(child)).sum()
            })
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
}

#[test]
fn live_blocks_do_not_depend_on_the_size_of_the_document() {
    let _serial = serial();
    let small = Open::new(2_000);
    let big = Open::new(100_000);
    assert!(!small.live().is_empty());
    assert_eq!(small.live(), big.live(), "the same blocks at the top");
    // Not just blocks: the nodes of the whole UI are the same too.
    assert_eq!(small.stats().nodes, big.stats().nodes);
    assert!(
        small.live().len() < 100,
        "{} live blocks",
        small.live().len()
    );
    report("live_blocks_100000", big.live().len());
}

#[test]
fn the_end_of_a_huge_document_is_reachable() {
    let _serial = serial();
    let mut doc = Open::new(100_000);
    let total = doc.markdown().content_height() as f32;
    doc.scroll_to(total);
    settle(&mut doc.app);
    let live = doc.live();
    assert!(live.contains(&99_999), "the last block is live: {live:?}");
    assert!(live.len() < 100);
    // The last block ends at the bottom of the scroll area, give or take the
    // padding below it.
    let bottom = doc.screen_top(99_999).expect("on screen")
        + doc
            .app
            .world()
            .get::<ComputedNode>(doc.markdown().block_entity(99_999).unwrap())
            .unwrap()
            .size
            .y;
    assert!(
        (bottom - VIEWPORT.y).abs() < 40.0,
        "bottom of the last block at {bottom}"
    );
}

#[test]
fn a_scroll_frame_spawns_at_most_the_blocks_that_enter() {
    let _serial = serial();
    for blocks in [2_000, 100_000] {
        let mut doc = Open::new(blocks);
        let mut y = 0.0;
        for step in [
            150.0, 150.0, 300.0, 1_500.0, -400.0, 9_000.0, -9_000.0, 150.0,
        ] {
            y = f32::max(y + step, 0.0);
            // What each live block weighs, before the frame.
            let before: Vec<(usize, usize)> = doc
                .markdown()
                .live_blocks()
                .map(|(ix, entity)| (ix, doc.subtree(entity)))
                .collect();
            doc.scroll_to(y);
            doc.app.update();
            let after = doc.live();
            let entering: usize = after
                .iter()
                .filter(|ix| !before.iter().any(|(b, _)| b == *ix))
                .map(|&ix| doc.subtree(doc.markdown().block_entity(ix).unwrap()))
                .sum();
            let leaving: usize = before
                .iter()
                .filter(|(ix, _)| !after.contains(ix))
                .map(|(_, size)| size)
                .sum();
            // Subtrees come from the pool when it has one of the shape, and
            // go back to it when it has room: only the first run of a
            // document spawns, and only a full pool despawns.
            assert!(
                doc.stats().spawned <= entering,
                "{blocks} blocks, scroll to {y}: spawned {} nodes for {entering} entering",
                doc.stats().spawned
            );
            assert!(
                doc.stats().despawned <= leaving,
                "{blocks} blocks, scroll to {y}: despawned {} nodes for {leaving} leaving",
                doc.stats().despawned
            );
            settle(&mut doc.app);
        }
    }
}

#[test]
fn the_block_at_the_top_stays_put_when_heights_above_change() {
    let _serial = serial();
    let mut doc = Open::new(600);
    // Far enough down that the blocks above are not live.
    doc.scroll_to(20_000.0);
    settle(&mut doc.app);
    let anchor = doc.markdown().scroll_anchor().expect("an anchor");
    assert!(anchor.block > 100, "{anchor:?}");
    let before = doc
        .screen_top(anchor.block)
        .expect("the anchor is on screen");
    let scroll_before = doc.scroll_y();

    // A long paragraph appears far above: every height after it moves.
    let at = doc.markdown().document().block(30).unwrap().span().start;
    let long = format!("{}\n\n", "a long paragraph of words ".repeat(120));
    doc.app
        .world_mut()
        .entity_mut(doc.markdown)
        .get_mut::<Markdown>()
        .unwrap()
        .edit(at..at, &long);
    settle(&mut doc.app);

    let moved = doc.markdown().scroll_anchor().expect("an anchor");
    assert_eq!(
        moved.block,
        anchor.block + 1,
        "the anchor follows its block"
    );
    let after = doc.screen_top(moved.block).expect("still on screen");
    assert!(
        (after - before).abs() < 1.5,
        "the block moved on screen from {before} to {after}"
    );
    assert!(
        doc.scroll_y() > scroll_before + 100.0,
        "the scroll position followed: {scroll_before} -> {}",
        doc.scroll_y()
    );

    // Heights changing the other way: the paragraph goes away again.
    let len = long.len();
    doc.app
        .world_mut()
        .entity_mut(doc.markdown)
        .get_mut::<Markdown>()
        .unwrap()
        .edit(at..at + len, "");
    settle(&mut doc.app);
    let back = doc.screen_top(anchor.block).expect("on screen");
    assert!((back - before).abs() < 1.5, "{back} vs {before}");
}

#[test]
fn measured_heights_replace_estimates_without_moving_the_screen() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    // Scroll in steps: each step brings never-measured blocks in above and
    // below the screen, and the block at the top must not drift.
    doc.scroll_to(30_000.0);
    settle(&mut doc.app);
    let anchor = doc.markdown().scroll_anchor().unwrap().block;
    let reference = doc.screen_top(anchor).unwrap();
    for _ in 0..6 {
        let y = doc.scroll_y() - 120.0;
        doc.scroll_to(y);
        settle(&mut doc.app);
        // The screen moved by 120 px and nothing else.
        let top = doc.markdown().scroll_anchor().unwrap();
        let on_screen = doc.screen_top(top.block).unwrap();
        assert!(
            (on_screen + top.offset).abs() < 1.5,
            "the anchor block starts {on_screen} px from the top with offset {}",
            top.offset
        );
    }
    // Scrolling back brings the very same block to the very same place.
    let back = doc.scroll_y() + 6.0 * 120.0;
    doc.scroll_to(back);
    settle(&mut doc.app);
    let again = doc.screen_top(anchor).unwrap();
    assert!((again - reference).abs() < 1.5, "{again} vs {reference}");
}

#[test]
fn estimates_are_in_the_range_of_what_layout_measures() {
    let _serial = serial();
    let doc = Open::new(2_000);
    let markdown = doc.markdown();
    let theme = test_theme();
    let metrics = Metrics {
        width: VIEWPORT.x - 80.0,
        body_size: theme.body_size,
        code_size: theme.code_size,
        heading_sizes: theme.heading_sizes,
        indent: theme.indent,
        block_gap: theme.block_gap,
        list_gap: theme.list_gap,
        ..Metrics::default()
    };
    let (mut estimated, mut measured) = (0.0f64, 0.0f64);
    let mut worst = 1.0f64;
    for (index, _) in markdown.live_blocks() {
        let block = markdown.document().block(index).unwrap();
        let guess = f64::from(estimate_height(block.block(), &metrics));
        let real = markdown.block_top(index + 1) - markdown.block_top(index);
        estimated += guess;
        measured += real;
        worst = worst.max((guess / real).max(real / guess));
    }
    let total_ratio = estimated / measured;
    report("estimate_total_over_measured", total_ratio);
    report("estimate_worst_block_ratio", worst);
    assert!(
        (0.8..1.25).contains(&total_ratio),
        "estimates add up to {total_ratio} of the measured height"
    );
    assert!(worst < 2.5, "a block is estimated {worst} times off");
}

#[test]
fn a_pinned_block_outlives_the_window_and_unpinning_frees_it() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    let pinned = doc.markdown().block_entity(3).expect("block 3 is live");
    doc.app.world_mut().entity_mut(pinned).insert(Pinned);
    doc.scroll_to(60_000.0);
    settle(&mut doc.app);
    assert_eq!(doc.markdown().block_entity(3), Some(pinned));
    assert!(doc.app.world().get_entity(pinned).is_ok());
    let live = doc.live();
    assert!(
        live.contains(&3) && live.iter().any(|&ix| ix > 1_000),
        "{live:?}"
    );
    // The pinned block is one more block, not a range back to it.
    assert!(live.len() < 100, "{} live blocks", live.len());

    doc.app.world_mut().entity_mut(pinned).remove::<Pinned>();
    doc.app.update();
    assert_eq!(doc.markdown().block_entity(3), None);
    // Unpinned, it is a block like any other: it waits in the pool, in the
    // container that is hidden.
    assert!(doc.in_pool(pinned), "the unpinned block waits in the pool");
}

#[test]
fn editing_a_pinned_block_pins_its_replacement() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    let pinned = doc.markdown().block_entity(3).unwrap();
    doc.app.world_mut().entity_mut(pinned).insert(Pinned);
    doc.scroll_to(60_000.0);
    settle(&mut doc.app);

    let at = doc.markdown().document().block(3).unwrap().span().start + 2;
    doc.app
        .world_mut()
        .entity_mut(doc.markdown)
        .get_mut::<Markdown>()
        .unwrap()
        .edit(at..at, "x");
    settle(&mut doc.app);
    let replacement = doc.markdown().block_entity(3).expect("still live");
    assert!(doc.app.world().get::<Pinned>(replacement).is_some());
    assert!(doc.live().iter().any(|&ix| ix > 1_000));
}

#[test]
fn edits_above_the_window_shift_the_live_blocks() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    doc.scroll_to(30_000.0);
    settle(&mut doc.app);
    let live = doc.live();
    let probe = *live.iter().nth(live.len() / 2).unwrap();
    let entity = doc.markdown().block_entity(probe).unwrap();
    // A new paragraph above the screen, before the first paragraph, shifts
    // every index after it by one.
    let at = doc.markdown().document().block(1).unwrap().span().start;
    doc.app
        .world_mut()
        .entity_mut(doc.markdown)
        .get_mut::<Markdown>()
        .unwrap()
        .edit(at..at, "A new paragraph.\n\n");
    settle(&mut doc.app);
    assert_eq!(doc.markdown().block_entity(probe + 1), Some(entity));
    assert_eq!(doc.stats().spawned + doc.stats().despawned, 0);
}

// ---------------------------------------------------------------------------
// Untouched blocks are not written (G7, G8) and subtrees are reused (G13)
// ---------------------------------------------------------------------------

impl Open {
    /// Entities with a `Text` in the subtree of `entity`.
    fn text_roots(&self, entity: Entity) -> usize {
        let world = self.app.world();
        usize::from(world.get::<Text>(entity).is_some())
            + world.get::<Children>(entity).map_or(0, |children| {
                children.iter().map(|child| self.text_roots(child)).sum()
            })
    }

    /// Whether `entity` is `root` or somewhere under it.
    fn is_under(&self, mut entity: Entity, root: Entity) -> bool {
        loop {
            if entity == root {
                return true;
            }
            match self.app.world().get::<ChildOf>(entity) {
                Some(parent) => entity = parent.parent(),
                None => return false,
            }
        }
    }

    fn writes(&self) -> Writes {
        self.app.world().resource::<Writes>().clone()
    }

    /// The container of the subtrees waiting in the pool: the one child of
    /// the document that is hidden.
    fn pool_container(&self) -> Option<Entity> {
        let world = self.app.world();
        world
            .get::<Children>(self.markdown)?
            .iter()
            .find(|&c| world.get::<Visibility>(c) == Some(&Visibility::Hidden))
    }

    /// The subtrees waiting in the pool.
    fn pool_children(&self) -> Vec<Entity> {
        self.pool_container()
            .and_then(|c| self.app.world().get::<Children>(c))
            .map(|children| children.iter().collect())
            .unwrap_or_default()
    }

    /// Subtrees waiting in the pool.
    fn waiting(&self) -> usize {
        self.pool_container()
            .and_then(|c| self.app.world().get::<Children>(c))
            .map_or(0, Children::len)
    }

    /// Whether `entity` is a subtree waiting in the pool.
    fn in_pool(&self, entity: Entity) -> bool {
        self.pool_container().is_some_and(|c| {
            self.app.world().get::<ChildOf>(entity).map(ChildOf::parent) == Some(c)
        })
    }
}

#[test]
fn an_idle_frame_writes_nothing() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    for _ in 0..5 {
        doc.app.update();
        let writes = doc.writes();
        assert!(writes.all.is_empty(), "written: {:?}", writes.all);
        assert_eq!(doc.stats().laid_out, 0);
    }
}

#[test]
fn scrolling_writes_only_what_enters_and_leaves() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    // Down a few screens, then back up over the blocks just seen.
    let down: Vec<f32> = (1..=16u16).map(|k| f32::from(k) * 150.0).collect();
    let up: Vec<f32> = (0..16u16).rev().map(|k| f32::from(k) * 150.0).collect();
    for (phase, steps) in [("down", down), ("up", up)] {
        for y in steps {
            let before: Vec<(usize, Entity)> = doc.markdown().live_blocks().collect();
            doc.scroll_to(y);
            doc.app.update();
            let after: Vec<(usize, Entity)> = doc.markdown().live_blocks().collect();
            let entering: Vec<Entity> = after
                .iter()
                .filter(|(ix, _)| !before.iter().any(|(b, _)| b == ix))
                .map(|(_, e)| *e)
                .collect();
            let leaving: Vec<Entity> = before
                .iter()
                .filter(|(ix, _)| !after.iter().any(|(a, _)| a == ix))
                .map(|(_, e)| *e)
                .collect();
            let staying: Vec<Entity> = after
                .iter()
                .filter(|(ix, _)| before.iter().any(|(b, _)| b == ix))
                .map(|(_, e)| *e)
                .collect();
            let writes = doc.writes();
            for &written in &writes.all {
                for &block in &staying {
                    assert!(
                        !doc.is_under(written, block),
                        "{phase}, y = {y}: {written} under a block that did not change"
                    );
                }
            }
            // Layouts: at most the text of the blocks that came in. A block
            // that goes out is not laid out again: a subtree waiting in the
            // pool keeps its size, so Bevy's text system has no reason to
            // shape it once more.
            let allowed: usize = entering.iter().map(|&e| doc.text_roots(e)).sum();
            let laid = doc.app.world().resource::<LaidOut>().0.clone();
            // Where the laid out text sits now, to say what is wrong.
            let place = |t: Entity| {
                let mut top = t;
                while let Some(parent) = doc.app.world().get::<ChildOf>(top).map(ChildOf::parent) {
                    if parent == doc.markdown || Some(parent) == doc.pool_container() {
                        return if Some(parent) == doc.pool_container() {
                            if before.iter().any(|(_, e)| *e == top) {
                                "pool, was live"
                            } else {
                                "pool, was waiting"
                            }
                        } else if entering.contains(&top) {
                            "entering"
                        } else {
                            "live, staying"
                        };
                    }
                    top = parent;
                }
                "elsewhere"
            };
            let chain = |mut e: Entity| {
                let mut out = vec![e];
                while let Some(p) = doc.app.world().get::<ChildOf>(e).map(ChildOf::parent) {
                    out.push(p);
                    e = p;
                }
                out
            };
            let odd: Vec<_> = laid
                .iter()
                .filter(|&&t| place(t) == "elsewhere")
                .take(2)
                .map(|&t| chain(t))
                .collect();
            let mut places = std::collections::BTreeMap::<&str, usize>::new();
            for &t in &laid {
                *places.entry(place(t)).or_default() += 1;
            }
            assert!(
                doc.stats().laid_out <= allowed,
                "{phase}, y = {y}: {} text blocks laid out for {allowed} of blocks entering \
                 ({} leaving): {places:?}; markdown {:?}, pool container {:?}, chains {odd:?}",
                doc.stats().laid_out,
                leaving.len(),
                doc.markdown,
                doc.pool_container(),
            );
            if phase == "up" {
                // Back over blocks that were seen: their subtrees are in the
                // pool, so nothing is spawned, and the only text written is
                // under the blocks that came in.
                assert_eq!(doc.stats().spawned, 0, "{phase}, y = {y}");
                for &written in &writes.texts {
                    assert!(
                        entering.iter().any(|&e| doc.is_under(written, e)),
                        "{phase}, y = {y}: text written outside the entering blocks"
                    );
                }
            }
            settle(&mut doc.app);
        }
    }
}

#[test]
fn an_edit_writes_and_lays_out_one_block() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    for block in [3, 10, 25] {
        let target = doc.markdown().block_entity(block).expect("live");
        let at = doc.markdown().document().block(block).unwrap().span().start + 2;
        doc.app
            .world_mut()
            .entity_mut(doc.markdown)
            .get_mut::<Markdown>()
            .unwrap()
            .edit(at..at, "x");
        doc.app.update();
        let writes = doc.writes();
        assert!(!writes.all.is_empty(), "block {block}: the edit shows");
        // The block is where it was and what it was: `BlockSurroundings`
        // stays as it is, and Bevy sees no change in it.
        assert!(
            writes.surroundings.is_empty(),
            "block {block}: surroundings written {:?}",
            writes.surroundings
        );
        for &written in &writes.all {
            assert!(
                doc.is_under(written, target),
                "block {block}: {written} is outside the edited block"
            );
        }
        assert_eq!(doc.stats().spawned + doc.stats().despawned, 0);
        assert!(
            doc.stats().laid_out <= doc.text_roots(target),
            "block {block}: {} laid out",
            doc.stats().laid_out
        );
        assert!(doc.stats().laid_out >= 1, "block {block}");
        settle(&mut doc.app);
    }
}

#[test]
fn a_block_released_to_the_pool_waits_hidden_and_comes_back_shown() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    let shown = |doc: &Open, entity: Entity| {
        doc.app.world().get::<ChildOf>(entity).map(ChildOf::parent) == Some(doc.markdown)
            && !doc.in_pool(entity)
    };
    doc.scroll_to(30_000.0);
    settle(&mut doc.app);
    // Blocks that left and were not taken by an entering one wait, hidden,
    // and are not despawned.
    let waiting = doc.waiting();
    assert!(waiting > 0, "some subtrees wait in the pool");
    for (_, entity) in doc.markdown().live_blocks() {
        assert!(shown(&doc, entity), "a block on screen is shown");
    }
    doc.scroll_to(0.0);
    settle(&mut doc.app);
    for (_, entity) in doc.markdown().live_blocks() {
        assert!(shown(&doc, entity), "a block back on screen is shown again");
    }
    assert!(doc.markdown().block_entity(0).is_some());
}

#[test]
fn a_block_that_leaves_and_returns_mostly_takes_its_own_subtree_back() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    doc.scroll_to(3_000.0);
    settle(&mut doc.app);
    // Back and forth by a little: what leaves at the top comes back and
    // finds its own subtree waiting, unless a block entering at the bottom
    // needed one of its shape and the pool had no other. Text is written
    // only for those.
    let (mut written, mut entering_texts, mut rebound) = (0, 0, 0);
    for round in 0..6 {
        let before: Vec<usize> = doc.live().into_iter().collect();
        doc.scroll_to(3_400.0);
        settle(&mut doc.app);
        doc.scroll_to(3_000.0);
        doc.app.update();
        let writes = doc.writes();
        let live: Vec<(usize, Entity)> = doc.markdown().live_blocks().collect();
        let back: Vec<Entity> = live
            .iter()
            .filter(|(ix, _)| !before.contains(ix) || *ix < before[0] + 8)
            .map(|(_, e)| *e)
            .collect();
        if round > 0 {
            written += writes.texts.len();
            entering_texts += back.iter().map(|&e| doc.text_roots(e)).sum::<usize>();
            rebound += doc.stats().spawned;
        }
        settle(&mut doc.app);
    }
    check("pool_return_text_writes", written);
    report("pool_return_texts_of_blocks_back", entering_texts);
    assert_eq!(rebound, 0, "nothing is spawned for blocks that were seen");
    assert!(
        written * 2 <= entering_texts,
        "{written} texts written for {entering_texts} texts of blocks coming back"
    );
}

// ---------------------------------------------------------------------------
// Review of M2/M3: no one-frame jump, no extra layouts, fresh surroundings
// ---------------------------------------------------------------------------

#[test]
fn scrolling_up_over_unmeasured_blocks_never_moves_the_screen_not_even_for_a_frame() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    doc.scroll_to(30_000.0);
    settle(&mut doc.app);
    // Up in small steps, each one bringing never-measured blocks in above the
    // screen. The frame is looked at as it would be drawn: right after the
    // update, with no frame to settle. The block at the top of the screen
    // has to be exactly where the scroll says it is.
    let (mut worst, mut corrected) = (0.0f32, 0);
    for step in 0..60u16 {
        let before = doc.scroll_y();
        let y = (before - 70.0 - f32::from(step % 5) * 13.0).max(0.0);
        doc.scroll_to(y);
        doc.app.update();
        let anchor = doc.markdown().scroll_anchor().expect("an anchor");
        // Where the block's slot starts on screen: the node's own top is
        // below its top margin, which the height index counts in the slot.
        let entity = doc
            .markdown()
            .block_entity(anchor.block)
            .expect("a live anchor");
        let margin = match doc.app.world().get::<Node>(entity).unwrap().margin.top {
            Val::Px(px) => px,
            _ => 0.0,
        };
        let top = doc
            .screen_top(anchor.block)
            .expect("the anchor is on screen")
            - margin;
        worst = worst.max((top + anchor.offset).abs());
        corrected += usize::from((doc.scroll_y() - y).abs() > 0.5);
    }
    report("scroll_jump_worst_px", worst);
    report("scroll_jump_frames_corrected", corrected);
    check("scroll_jump_worst_px", worst);
    assert!(
        corrected > 5,
        "only {corrected} frames needed a correction: the test says little"
    );
}

#[test]
fn blocks_that_move_say_so_in_their_surroundings() {
    let _serial = serial();
    let mut doc = Open::new(2_000);
    doc.scroll_to(30_000.0);
    settle(&mut doc.app);
    let at = doc.markdown().document().block(1).unwrap().span().start;
    doc.app
        .world_mut()
        .entity_mut(doc.markdown)
        .get_mut::<Markdown>()
        .unwrap()
        .edit(at..at, "Two new paragraphs.\n\nThe second.\n\n");
    doc.app.update();
    let live: Vec<(usize, Entity)> = doc.markdown().live_blocks().collect();
    assert!(live.len() > 20);
    for (index, entity) in live {
        let surroundings = doc
            .app
            .world()
            .get::<bevy_md::BlockSurroundings>(entity)
            .expect("every live block has surroundings");
        assert_eq!(surroundings.index, index, "block {index}");
    }
    // Every live block was told, and nothing else but the few that went out
    // of the window in the same frame.
    let writes = doc.writes();
    let live: Vec<Entity> = doc.markdown().live_blocks().map(|(_, e)| e).collect();
    assert!(live.iter().all(|e| writes.surroundings.contains(e)));
    assert!(
        writes.surroundings.len() <= live.len() + 8,
        "{} written for {} live blocks",
        writes.surroundings.len(),
        live.len()
    );
}

/// The pool moves subtrees between the document and its hidden container one
/// by one. Whatever the order of scrolls, edits and pins, every entity of
/// the tree ends up with exactly one parent that lists it, and no live block
/// is in the pool.
#[test]
fn scrolls_edits_and_pins_in_any_order_keep_the_tree_whole() {
    let _serial = serial();
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move |n: u64| {
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        state.wrapping_mul(0x2545_F491_4F6C_DD1D) % n
    };
    let mut doc = Open::new(2_000);
    let mut pinned: Vec<Entity> = Vec::new();
    for step in 0..400 {
        match next(7) {
            0..=2 => {
                let y = f32::from(u16::try_from(next(55_000)).unwrap());
                doc.scroll_to(y);
            }
            3 => {
                let y = (doc.scroll_y() + f32::from(u16::try_from(next(600)).unwrap()) - 300.0)
                    .max(0.0);
                doc.scroll_to(y);
            }
            4 => {
                let block = next(1_900) as usize;
                let at = doc.markdown().document().block(block).unwrap().span().start;
                doc.app
                    .world_mut()
                    .entity_mut(doc.markdown)
                    .get_mut::<Markdown>()
                    .unwrap()
                    .edit(at..at, "Inserted.\n\n");
            }
            5 => {
                let live: Vec<Entity> = doc.markdown().live_blocks().map(|(_, e)| e).collect();
                let entity = live[next(live.len() as u64) as usize];
                doc.app.world_mut().entity_mut(entity).insert(Pinned);
                pinned.push(entity);
            }
            _ => {
                if let Some(entity) = pinned.pop()
                    && let Ok(mut entity) = doc.app.world_mut().get_entity_mut(entity)
                {
                    entity.remove::<Pinned>();
                }
            }
        }
        doc.app.update();

        // Every child lists its parent and is listed by it; no duplicates.
        let world = doc.app.world();
        let mut seen = BTreeSet::new();
        let mut stack = vec![doc.markdown];
        while let Some(parent) = stack.pop() {
            let Some(children) = world.get::<Children>(parent) else {
                continue;
            };
            for child in children.iter() {
                assert!(seen.insert(child), "step {step}: {child} is listed twice");
                assert_eq!(
                    world.get::<ChildOf>(child).map(ChildOf::parent),
                    Some(parent),
                    "step {step}: {child} does not point at the parent that lists it"
                );
                stack.push(child);
            }
        }
        // Live blocks hang from the document, never from the pool.
        for (index, entity) in doc.markdown().live_blocks() {
            assert_eq!(
                world.get::<ChildOf>(entity).map(ChildOf::parent),
                Some(doc.markdown),
                "step {step}: live block {index} is not a child of the document"
            );
        }
        for &entity in &doc.pool_children() {
            assert!(
                !doc.markdown().live_blocks().any(|(_, e)| e == entity),
                "step {step}: a live block is in the pool"
            );
        }
    }
    settle(&mut doc.app);
}

/// A definition far below the screen changes how a reference on screen is
/// drawn: the plugin sees it as the edit it is (the index finds the user, the
/// splice covers both), without parsing the document again.
#[test]
fn a_definition_far_below_reaches_a_reference_on_screen() {
    let source = format!(
        "Use [thing] and [other][o] here.\n\n{}\n",
        synthetic::source(2_000)
    );
    let mut app = headless_app();
    app.update();
    let (_, entity) = spawn_document(&mut app, &source);
    settle(&mut app);
    let link_color = test_theme().link;
    let linked = |app: &App| -> usize {
        let world = app.world();
        let markdown = world.get::<Markdown>(entity).expect("markdown");
        let root = markdown
            .block_entity(0)
            .expect("the first block is on screen");
        let mut count = 0;
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if world.get::<TextColor>(e).is_some_and(|c| c.0 == link_color) {
                count += 1;
            }
            if let Some(children) = world.get::<Children>(e) {
                stack.extend(children.iter());
            }
        }
        count
    };
    assert_eq!(linked(&app), 0, "nothing is defined yet: plain text");

    let (full_parses, end) = {
        let markdown = app.world().get::<Markdown>(entity).unwrap();
        (
            markdown.document().stats().full_parses,
            markdown.source().len(),
        )
    };
    app.world_mut()
        .get_mut::<Markdown>(entity)
        .unwrap()
        .edit(end..end, "\n[thing]: /thing\n\n[o]: /other\n");
    settle(&mut app);
    {
        let markdown = app.world().get::<Markdown>(entity).unwrap();
        assert_eq!(markdown.document().stats().full_parses, full_parses);
        assert_eq!(
            markdown.document().block(0).unwrap().content.links.len(),
            2,
            "both references resolve"
        );
    }
    assert!(
        linked(&app) >= 1,
        "the reference on screen is drawn as a link"
    );

    // Taking one away turns its reference back into plain text.
    let at = app
        .world()
        .get::<Markdown>(entity)
        .unwrap()
        .source()
        .find("[thing]: /thing\n")
        .unwrap();
    app.world_mut()
        .get_mut::<Markdown>(entity)
        .unwrap()
        .edit(at..at + "[thing]: /thing\n".len(), "");
    settle(&mut app);
    let markdown = app.world().get::<Markdown>(entity).unwrap();
    assert_eq!(markdown.document().block(0).unwrap().content.links.len(), 1);
    assert_eq!(markdown.document().stats().full_parses, full_parses);
}
