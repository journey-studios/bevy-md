//! Performance budgets with exact counts (see `docs/plans/fase-2-performance.md`).
//!
//! Everything runs in a headless app with real UI and text layout. The
//! counts (entities, spawns, layouts, allocations) are exact and repeatable;
//! time is only ever compared as a ratio between two documents, measured
//! alternately. The limits live in `perf-budgets.json`.

mod common;

use bevy::prelude::*;
use bevy_md::Markdown;
use bevy_md_core::synthetic;
use common::*;

struct Open {
    app: App,
    area: Entity,
    markdown: Entity,
}

impl Open {
    fn live_blocks(&self) -> usize {
        self.app
            .world()
            .get::<Markdown>(self.markdown)
            .expect("markdown")
            .live_blocks()
            .len()
    }

    fn stats(&self) -> FrameStats {
        *self.app.world().resource::<FrameStats>()
    }

    /// Scroll to `y` and run one frame.
    fn scroll_frame(&mut self, y: f32) -> (FrameStats, AllocStats) {
        set_scroll(&mut self.app, self.area, y);
        let ((), alloc) = count_allocations(|| self.app.update());
        (self.stats(), alloc)
    }

    /// Insert `x` inside block `block` and run one frame, the edit included.
    fn edit_frame(&mut self, block: usize) -> (FrameStats, AllocStats) {
        let markdown = self.markdown;
        let ((), alloc) = count_allocations(|| {
            let mut entity = self.app.world_mut().entity_mut(markdown);
            let mut markdown = entity.get_mut::<Markdown>().expect("markdown");
            let at = markdown.document().block(block).unwrap().span().start + 2;
            markdown.edit(at..at, "x");
            self.app.update();
        });
        (self.stats(), alloc)
    }
}

/// Open a document of `blocks` blocks and settle it. Returns what opening
/// it left allocated, too.
fn open(blocks: usize) -> (Open, AllocStats) {
    let source = synthetic::source(blocks);
    let mut app = headless_app();
    // One frame first, so the plugin's own one-time state is not charged
    // to the document.
    app.update();
    let ((area, markdown), alloc) = count_allocations(|| {
        let pair = spawn_document(&mut app, &source);
        settle(&mut app);
        pair
    });
    (
        Open {
            app,
            area,
            markdown,
        },
        alloc,
    )
}

/// Scroll down through the document, `step` pixels a frame, and report the
/// worst frame of each count.
fn scroll_through(doc: &mut Open, frames: u16, step: f32) -> (usize, usize, usize, u64) {
    let (mut spawned, mut despawned, mut laid_out, mut allocations) = (0, 0, 0, 0);
    for frame in 1..=frames {
        let (stats, alloc) = doc.scroll_frame(f32::from(frame) * step);
        spawned = spawned.max(stats.spawned);
        despawned = despawned.max(stats.despawned);
        laid_out = laid_out.max(stats.laid_out);
        allocations = allocations.max(alloc.allocations);
    }
    (spawned, despawned, laid_out, allocations)
}

fn counts_for(blocks: usize) {
    let _serial = serial();
    let (mut doc, opened) = open(blocks);
    let live = doc.live_blocks();
    report(&format!("live_blocks_{blocks}"), live);
    check(&format!("live_blocks_{blocks}"), live);
    check(
        &format!("bytes_per_block_{blocks}"),
        opened.retained.measure() / blocks.measure(),
    );
    report(
        &format!("open_allocations_per_block_{blocks}"),
        opened.allocations.measure() / blocks.measure(),
    );

    // An idle frame: nothing changed, so nothing should be spawned, laid
    // out or allocated beyond the app's own bookkeeping.
    // Spawns and layouts must be zero in every one of thirty frames. For
    // allocations, the worst of the best 27: Bevy itself has a frame now and
    // then (about 240 allocations, at any document size, in some runs only)
    // that is not the document's doing. Ten frames first, to let Bevy's own
    // housekeeping reach its steady state.
    frames(&mut doc.app, 10);
    let (mut spawns, mut laid_out) = (0, 0);
    let (mut allocations, mut bytes) = (Vec::new(), Vec::new());
    for _ in 0..30 {
        let ((), idle) = count_allocations(|| doc.app.update());
        let stats = doc.stats();
        spawns = spawns.max(stats.spawned + stats.despawned);
        laid_out = laid_out.max(stats.laid_out);
        allocations.push(idle.allocations);
        bytes.push(idle.bytes);
    }
    allocations.sort_unstable();
    bytes.sort_unstable();
    let (allocations, bytes) = (allocations[26], bytes[26]);
    check(&format!("idle_spawns_{blocks}"), spawns);
    check(&format!("idle_laid_out_{blocks}"), laid_out);
    check(&format!("idle_allocations_{blocks}"), allocations);
    check(&format!("idle_bytes_{blocks}"), bytes);

    // Scrolling: the worst frame of a pass down the first screens.
    let (spawned, despawned, laid_out, allocations) = scroll_through(&mut doc, 12, 150.0);
    check(&format!("scroll_spawns_per_frame_{blocks}"), spawned);
    check(&format!("scroll_despawns_per_frame_{blocks}"), despawned);
    check(&format!("scroll_laid_out_per_frame_{blocks}"), laid_out);
    check(
        &format!("scroll_allocations_per_frame_{blocks}"),
        allocations,
    );

    // An edit inside a visible block.
    set_scroll(&mut doc.app, doc.area, 0.0);
    settle(&mut doc.app);
    // Once before, so the source's buffer has grown: the first edit of a
    // freshly parsed document copies it, whatever the document.
    doc.edit_frame(3);
    settle(&mut doc.app);
    let (stats, alloc) = doc.edit_frame(3);
    check(&format!("edit_spawns_{blocks}"), stats.spawned);
    check(&format!("edit_laid_out_{blocks}"), stats.laid_out);
    check(&format!("edit_allocations_{blocks}"), alloc.allocations);
    check(&format!("edit_bytes_{blocks}"), alloc.bytes);
}

#[test]
fn counts_with_200_blocks() {
    counts_for(200);
}

#[test]
fn counts_with_2000_blocks() {
    counts_for(2_000);
}

#[test]
fn counts_with_20000_blocks() {
    counts_for(20_000);
}

#[test]
fn counts_with_100000_blocks() {
    counts_for(100_000);
}

/// The median frame of `small` and `big`, run alternately after three
/// warm-up passes, as a ratio: it measures how a frame's cost grows with the
/// document without depending on the machine.
fn frame_ratio(
    small: &mut Open,
    big: &mut Open,
    frame: impl Fn(&mut Open, usize) -> std::time::Duration,
) -> f64 {
    for pass in 0..3 {
        for step in 0..8 {
            frame(small, pass * 8 + step);
            frame(big, pass * 8 + step);
        }
    }
    let (mut small_times, mut big_times) = (Vec::new(), Vec::new());
    for step in 0..60 {
        small_times.push(frame(small, step).as_secs_f64());
        big_times.push(frame(big, step).as_secs_f64());
    }
    median(big_times) / median(small_times)
}

fn idle(doc: &mut Open, _step: usize) -> std::time::Duration {
    timed_frame(&mut doc.app)
}

/// A frame that scrolls a little, back and forth.
fn scroll(doc: &mut Open, step: usize) -> std::time::Duration {
    let y = if step.is_multiple_of(2) { 100.0 } else { 140.0 };
    set_scroll(&mut doc.app, doc.area, y);
    timed_frame(&mut doc.app)
}

/// A frame with a keystroke in a visible block.
fn edit(doc: &mut Open, step: usize) -> std::time::Duration {
    let markdown = doc.markdown;
    {
        let mut entity = doc.app.world_mut().entity_mut(markdown);
        let mut markdown = entity.get_mut::<Markdown>().expect("markdown");
        let at = markdown.document().block(3).unwrap().span().start + 2;
        // Alternate insert and delete so the document does not grow.
        if step.is_multiple_of(2) {
            markdown.edit(at..at, "x");
        } else {
            markdown.edit(at..at + 1, "");
        }
    }
    timed_frame(&mut doc.app)
}

fn ratio_between(small: usize, big: usize) {
    let _serial = serial();
    let (mut small_doc, _) = open(small);
    let (mut big_doc, _) = open(big);
    let idle_ratio = frame_ratio(&mut small_doc, &mut big_doc, idle);
    let scroll_ratio = frame_ratio(&mut small_doc, &mut big_doc, scroll);
    let edit_ratio = frame_ratio(&mut small_doc, &mut big_doc, edit);
    check(&format!("idle_frame_ratio_{big}_over_{small}"), idle_ratio);
    check(
        &format!("scroll_frame_ratio_{big}_over_{small}"),
        scroll_ratio,
    );
    check(&format!("edit_frame_ratio_{big}_over_{small}"), edit_ratio);
}

#[test]
fn frame_time_ratio_2000_over_200() {
    ratio_between(200, 2_000);
}

#[test]
fn frame_time_ratio_100000_over_200() {
    ratio_between(200, 100_000);
}
