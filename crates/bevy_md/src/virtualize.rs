//! Virtualization: nodes only for the blocks near the screen.
//!
//! Each frame, before layout, [`sync_blocks`] applies the edits made to a
//! [`Markdown`], works out which blocks the screen needs (one screen above
//! and two below the visible ones, plus the [`Pinned`] ones) and gives nodes
//! to the ones that lack them. Every other block is a gap in the children,
//! filled by a spacer node as tall as the blocks it stands for.
//!
//! A block that leaves the window hands its subtree to a pool, keyed by what
//! the subtree is made of (its signature); a block that enters takes one of
//! the same signature and has only its strings rewritten, and only the ones
//! that differ. Nothing is spawned for a block that finds a subtree, and
//! nothing is written that did not change.
//!
//! After layout, [`measure_blocks`] reads the real height of the blocks that
//! have nodes into the height index, replacing the estimates, and keeps the
//! scroll position on the anchor block: the one at the top of the screen.
//! When anything above it changes height the scroll position moves by the
//! difference, so what is on screen does not jump.

use std::ops::Range;
use std::sync::Arc;

use bevy::ecs::system::SystemParam;
use bevy::math::Affine2;
use bevy::prelude::*;
use bevy::ui::{ComputedNode, ScrollPosition, UiGlobalTransform};
use bevy_md_core::Fate;
use bevy_md_core::estimate::{Metrics, estimate_height};
use bevy_md_core::{BlockKind, BlockRef, ListMarker};

use crate::background::Op;
use crate::heights::Height;
use crate::render;
use crate::theme::MarkdownTheme;
use crate::{BlockSurroundings, Live, Markdown, POOL_MAX, Pinned, ScrollAnchor, Sides};

/// Viewport height assumed before the scrolled node has been laid out.
const FALLBACK_VIEWPORT: f64 = 1080.0;
/// Screens kept above and below the visible ones.
const SCREENS_ABOVE: f64 = 1.0;
const SCREENS_BELOW: f64 = 2.0;
/// Heights replaced by an install that are worth freeing on the pool instead
/// of on the main thread.
const DROP_AWAY: usize = 256;
/// How many ancestors are searched for the scrolled node.
const MAX_DEPTH: usize = 32;

/// The nearest ancestor of `start` for which `is_area` holds.
pub(crate) fn scroll_area(
    start: Entity,
    parents: &Query<&ChildOf>,
    is_area: impl Fn(Entity) -> bool,
) -> Option<Entity> {
    let mut current = start;
    for _ in 0..MAX_DEPTH {
        current = parents.get(current).ok()?.parent();
        if is_area(current) {
            return Some(current);
        }
    }
    None
}

fn metrics(theme: &MarkdownTheme, width: f32) -> Metrics {
    Metrics {
        width,
        body_size: theme.body_size,
        code_size: theme.code_size,
        heading_sizes: theme.heading_sizes,
        indent: theme.indent,
        block_gap: theme.block_gap,
        list_gap: theme.list_gap,
        ..Metrics::default()
    }
}

/// A length in logical pixels, when it is one.
pub(crate) fn px(val: Val) -> f32 {
    match val {
        Val::Px(px) => px,
        _ => 0.0,
    }
}

fn surroundings(index: usize, block: &BlockRef) -> BlockSurroundings {
    BlockSurroundings {
        index,
        list_number: match block.kind {
            BlockKind::ListItem {
                marker: ListMarker::Ordered(n),
                ..
            } => Some(n),
            _ => None,
        },
        list_depth: block.list_depth,
        quote_depth: block.quote_depth,
    }
}

/// What `sync_blocks` writes to and reads pins from, as one system param so
/// the helpers can take it whole.
#[derive(SystemParam)]
pub(crate) struct Sinks<'w, 's> {
    commands: Commands<'w, 's>,
    pins: Query<'w, 's, (), With<Pinned>>,
    nodes: Query<'w, 's, &'static mut Node>,
    texts: Query<'w, 's, &'static mut Text>,
    spans: Query<'w, 's, &'static mut TextSpan>,
    surroundings: Query<'w, 's, &'static mut BlockSurroundings>,
    theme: Res<'w, MarkdownTheme>,
}

impl Markdown {
    /// Give a subtree back to the pool, or despawn it when the pool is full.
    fn release(&mut self, live: Live, hint: Option<usize>, sinks: &mut Sinks<'_, '_>) {
        let root = live.built.root;
        if sinks.pins.contains(root) {
            // Whoever takes the subtree next is not the pinned block.
            sinks.commands.entity(root).remove::<Pinned>();
        }
        if self.pool.len() >= POOL_MAX {
            sinks.commands.entity(root).despawn();
            return;
        }
        let mut built = live.built;
        built.last = hint;
        self.pool.put(built);
    }

    /// A subtree for block `index`: one from the pool rebound to the block
    /// when there is one of the same signature, a new one otherwise. The
    /// flag says whether it is new.
    fn acquire(
        &mut self,
        index: usize,
        pinned: bool,
        window: &Range<usize>,
        sinks: &mut Sinks<'_, '_>,
    ) -> (Live, bool) {
        let block = self.document.block(index).expect("a block of the document");
        let signature = render::signature(&block);
        let (built, fresh) = match self.pool.take(&signature, index, window) {
            Some(built) => {
                render::rebind(&block, &built, &mut sinks.texts, &mut sinks.spans);
                (built, false)
            }
            None => (
                render::spawn_block(&mut sinks.commands, &block, &sinks.theme),
                true,
            ),
        };
        let wanted = surroundings(index, &block);
        if fresh {
            sinks.commands.entity(built.root).insert(wanted);
        } else if let Ok(mut current) = sinks.surroundings.get_mut(built.root) {
            current.set_if_neq(wanted);
        }
        if pinned {
            sinks.commands.entity(built.root).insert(Pinned);
        }
        (Live { index, built }, fresh)
    }

    /// Apply the edits since the last sync to the height index, the live
    /// blocks and the anchor. Returns the (new) indices of replaced blocks
    /// that were pinned, when anything was applied.
    ///
    /// The edits are the splices of `dirty`: disjoint stretches of the block
    /// list, in the list the heights describe (`old`) and in the document's
    /// (`new`). Only they are touched. A block outside them keeps its entity,
    /// its measured height, its pool hint and its place as the scroll anchor;
    /// it only changes index, by what the splices before it added or removed
    /// (`Splices::locate_old`), and the entities that have one are told.
    fn apply_edits(&mut self, sinks: &mut Sinks<'_, '_>) -> Option<Vec<usize>> {
        if self.dirty.is_empty() {
            return None;
        }
        let splices = std::mem::take(&mut self.dirty);
        let new_len = self.document.len();
        debug_assert_eq!(
            splices.len_after(self.heights.len()),
            new_len,
            "the pending splices lead from the heights to the document"
        );

        // What the pooled subtrees showed is stale where a splice replaced
        // it, and has moved where it did not.
        for built in self.pool.iter_mut() {
            if let Some(at) = built.last {
                built.last = splices.map_old(at);
            }
        }

        // Nodes: the replaced blocks give their subtrees back (so a block
        // rebuilt as the same shape reuses its own), the ones outside the
        // splices move over.
        let mut repin = Vec::new();
        let live = std::mem::take(&mut self.live);
        let mut kept = Vec::with_capacity(live.len());
        for mut entry in live {
            match splices.locate_old(entry.index) {
                Fate::Kept(index) => {
                    if index != entry.index {
                        entry.index = index;
                        // The block moved: its surroundings say where, and
                        // only the blocks that moved are written.
                        if let Ok(mut at) = sinks.surroundings.get_mut(entry.built.root)
                            && at.index != index
                        {
                            at.index = index;
                        }
                    }
                    kept.push(entry);
                }
                Fate::Replaced { splice, offset } => {
                    let new = &splices[splice].new;
                    if sinks.pins.contains(entry.built.root) && !new.is_empty() {
                        repin.push(new.start + offset.min(new.len() - 1));
                    }
                    // The replacement of this block takes the subtree back.
                    let hint = (offset < new.len()).then_some(new.start + offset);
                    self.release(entry, hint, sinks);
                }
            }
        }
        self.live = kept;

        // Heights: a replaced block starts from the height it had, which is
        // the best guess for an edit of it, and new ones from an estimate.
        // A background parse brought its own estimates, already an index
        // per splice: they are hung in without a visit to any of their
        // blocks. The splices go from the last, so that the ones before keep
        // the coordinates they were given in.
        let heights_started = std::time::Instant::now();
        let mut built = self.estimates.take().filter(|estimates| {
            estimates.len() == splices.len()
                && estimates
                    .iter()
                    .zip(&splices)
                    .all(|(heights, splice)| heights.len() == splice.new.len())
        });
        let metrics = metrics(&sinks.theme, self.width);
        for splice in splices.iter().rev() {
            let (old, new) = (splice.old.clone(), splice.new.clone());
            if let Some(estimated) = built.as_mut().and_then(Vec::pop) {
                let removed = self.heights.splice_built(old, estimated);
                if removed.len() > DROP_AWAY {
                    crate::background::drop_in_background(removed);
                }
            } else {
                let heights: Vec<Height> = self
                    .document
                    .blocks_from(new.start)
                    .take(new.len())
                    .enumerate()
                    .map(|(k, block)| match self.heights.get(old.start + k) {
                        Some(before) if k < old.len() => Height::estimated(before.px),
                        _ => Height::estimated(estimate_height(block.block(), &metrics)),
                    })
                    .collect();
                self.heights.splice(old, heights);
            }
        }
        if self.timing_install {
            self.background.timings.install_heights = heights_started.elapsed();
        }

        // The anchor moves with the blocks; if its block was replaced it
        // keeps its place among the replacements.
        if let Some(anchor) = &mut self.anchor {
            match splices.locate_old(anchor.block) {
                Fate::Kept(block) => anchor.block = block,
                Fate::Replaced { splice, offset } => {
                    let new = &splices[splice].new;
                    if new.is_empty() {
                        anchor.block = new.start;
                        anchor.offset = 0.0;
                    } else {
                        anchor.block = new.start + offset.min(new.len() - 1);
                    }
                }
            }
            if new_len == 0 {
                self.anchor = None;
            } else if let Some(anchor) = &mut self.anchor {
                anchor.block = anchor.block.min(new_len - 1);
            }
        }
        self.layout_dirty = true;
        Some(repin)
    }

    /// Move a background parse along and apply what is ready: start it when
    /// one is wanted; install its result when it is done (the blocks that
    /// differ from what is shown are replaced, the rest stay); apply the
    /// changes queued meanwhile once it has landed. Then whatever small edit
    /// was made on the document. Returns what `apply_edits` does.
    fn advance(&mut self, sinks: &mut Sinks<'_, '_>) -> Option<Vec<usize>> {
        let mut repin = None;
        if self.background.is_waiting_to_start() {
            // The parse is told which blocks differ from the document shown,
            // so what is shown has to be what the document is: a small edit
            // made in this same frame is applied first.
            if !self.dirty.is_empty() {
                repin = self.apply_edits(sinks);
            }
            let metrics = metrics(&sinks.theme, self.width);
            self.background.start(&self.document, metrics);
        }
        let advance_started = std::time::Instant::now();
        if let Some(parsed) = self.background.poll() {
            self.timing_install = true;
            {
                let t = &mut self.background.timings;
                t.parse = parsed.parse;
                t.splice_from = parsed.splice_from;
                t.estimate = parsed.estimate;
                t.new_blocks = parsed.splices.inserted();
            }
            let old = std::mem::replace(&mut self.document, Arc::new(parsed.document));
            crate::background::drop_in_background(old);
            self.estimates = Some(parsed.heights);
            self.queue(parsed.splices);
            self.background.finish();
        } else if self.background.only_queue_left() {
            let ops = self.background.take_queue();
            if !ops.is_empty() {
                // The document holds the text the ops apply to: no second
                // owner, so the first of them edits it in place.
                self.background.release_source();
            }
            for op in ops {
                let splice = match op {
                    Op::Edit { range, text } => {
                        Arc::make_mut(&mut self.document).edit(range, &text)
                    }
                    Op::Append(text) => Arc::make_mut(&mut self.document).push_str(&text),
                };
                self.queue(splice);
            }
            self.background.finish();
        }
        let applied = self.apply_edits(sinks);
        if self.timing_install {
            self.background.timings.install_advance = advance_started.elapsed();
            self.timing_install = false;
        }
        // Estimates are for the install that has just been applied.
        self.estimates = None;
        match (repin, applied) {
            (Some(mut first), Some(second)) => {
                first.extend(second);
                Some(first)
            }
            (first, second) => first.or(second),
        }
    }

    /// The blocks the screen needs, given the scrolled node: one screen above
    /// the visible ones to two below. Everything when nothing scrolls.
    fn needed(&self, view: Option<(f64, f64)>) -> Range<usize> {
        let len = self.heights.len();
        let Some((top, height)) = view else {
            return 0..len;
        };
        if len == 0 {
            return 0..0;
        }
        let first = self.heights.at(top - SCREENS_ABOVE * height);
        let last = self.heights.at(top + (1.0 + SCREENS_BELOW) * height) + 1;
        first..last.min(len)
    }

    /// Give nodes to the blocks the window needs and take them from the ones
    /// it no longer does, then order the children and size the spacers.
    fn rebuild(&mut self, root: Entity, window: Range<usize>, repin: &[usize], sinks: &mut Sinks) {
        let old = std::mem::take(&mut self.live);
        // Blocks outside the window that stay: pinned ones, and the
        // replacements of pinned ones.
        let mut known: Vec<(usize, Option<Live>)> = Vec::new();
        for entry in old {
            if window.contains(&entry.index) || sinks.pins.contains(entry.built.root) {
                known.push((entry.index, Some(entry)));
            } else {
                let hint = Some(entry.index);
                self.release(entry, hint, sinks);
            }
        }
        for &index in repin {
            if !window.contains(&index) && !known.iter().any(|(ix, _)| *ix == index) {
                known.push((index, None));
            }
        }
        known.sort_unstable_by_key(|(index, _)| *index);

        // The window's blocks without a subtree.
        let mut live = Vec::with_capacity(known.len() + window.len());
        let mut kept = known.into_iter().peekable();
        let mut place =
            |this: &mut Self, index: usize, entry: Option<Live>, sinks: &mut Sinks<'_, '_>| {
                let entry = entry.unwrap_or_else(|| {
                    this.acquire(index, repin.contains(&index), &window, sinks)
                        .0
                });
                live.push(entry);
            };
        for index in window.clone() {
            // Kept blocks before this index (pinned ones outside the window).
            while let Some((ix, _)) = kept.peek() {
                if *ix >= index {
                    break;
                }
                let (ix, entry) = kept.next().expect("peeked");
                place(self, ix, entry, sinks);
            }
            match kept.peek() {
                Some((ix, _)) if *ix == index => {
                    let (ix, entry) = kept.next().expect("peeked");
                    place(self, ix, entry, sinks);
                }
                _ => place(self, index, None, sinks),
            }
        }
        for (ix, entry) in kept {
            place(self, ix, entry, sinks);
        }
        self.live = live;
        self.window = window;
        self.arrange(root, sinks);
    }

    /// Order the children as spacer, blocks, spacer, ..., pool, and size
    /// the spacers to what each stands for.
    fn arrange(&mut self, root: Entity, sinks: &mut Sinks<'_, '_>) {
        let mut order: Vec<Entity> = Vec::with_capacity(self.live.len() + 4);
        let mut spacers = 0;
        let mut next = 0;
        let mut spacer = |this: &mut Self, gap: Range<usize>, sinks: &mut Sinks<'_, '_>| {
            let height = this.heights.span(gap) as f32;
            let entity = if let Some(&entity) = this.spacers.get(spacers) {
                if let Ok(mut node) = sinks.nodes.get_mut(entity) {
                    let want = Val::Px(height);
                    if node.height != want {
                        node.height = want;
                    }
                }
                entity
            } else {
                let entity = sinks.commands.spawn(spacer_node(height)).id();
                this.spacers.push(entity);
                entity
            };
            spacers += 1;
            entity
        };
        let live = std::mem::take(&mut self.live);
        // The spacer before the first block and the one after the last are
        // always there, even at height 0, so the order is stable.
        order.push(spacer(self, 0..live.first().map_or(0, |l| l.index), sinks));
        for (at, entry) in live.iter().enumerate() {
            if entry.index > next && at > 0 {
                order.push(spacer(self, next..entry.index, sinks));
            }
            order.push(entry.built.root);
            next = entry.index + 1;
        }
        let len = self.heights.len();
        order.push(spacer(self, next.min(len)..len, sinks));
        self.live = live;
        for extra in self.spacers.drain(spacers..) {
            sinks.commands.entity(extra).despawn();
        }
        // Waiting subtrees live in one container that is hidden and out of
        // the flow, above the document so that it adds nothing to the
        // scrollable size. They are laid out at the width they had, with the
        // same inputs as in the column, so neither Taffy nor Bevy's text
        // system has a reason to measure their text again, which a change to
        // the subtree's own `Node` (making it absolute, or `Display::None`)
        // does.
        let waiting: Vec<Entity> = self.pool.iter_mut().map(|built| built.root).collect();
        let pool_root = *self.pool_root.get_or_insert_with(|| {
            sinks
                .commands
                .spawn((pool_node(self.padding), Visibility::Hidden))
                .id()
        });
        if self.pool_padding != self.padding {
            self.pool_padding = self.padding;
            let padding = self.padding;
            sinks
                .commands
                .entity(pool_root)
                .entry::<Node>()
                .and_modify(move |mut node| {
                    node.left = Val::Px(padding.left);
                    node.right = Val::Px(padding.right);
                });
        }
        if waiting != self.pool_children {
            // Move subtrees one by one: `replace_children` skips the hooks
            // that take an entity out of its old parent, so using it on two
            // parents that trade children would leave the entity in both
            // lists and orphan it. Moves made here are plain `ChildOf`
            // inserts; the document's own `replace_children` below then only
            // orders what is already its own.
            let was: std::collections::HashSet<Entity> =
                self.pool_children.iter().copied().collect();
            let is: std::collections::HashSet<Entity> = waiting.iter().copied().collect();
            for &entity in waiting.iter().filter(|e| !was.contains(e)) {
                sinks.commands.entity(entity).try_insert(ChildOf(pool_root));
            }
            for &entity in self.pool_children.iter().filter(|e| !is.contains(e)) {
                sinks.commands.entity(entity).try_insert(ChildOf(root));
            }
            self.pool_children = waiting;
        }
        order.push(pool_root);
        // The controls follow the pooled subtrees.
        order.extend(self.gutter);
        if order != self.children {
            sinks.commands.entity(root).replace_children(&order);
            self.children = order;
        }
    }
}

/// The container of the subtrees waiting in the pool: absolute, with the
/// document's padding as insets so it is as wide as its content, and with its
/// bottom edge at the document's top, so everything in it lies above.
fn pool_node(padding: Sides) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: Val::Px(padding.left),
        right: Val::Px(padding.right),
        bottom: Val::Percent(100.0),
        flex_direction: FlexDirection::Column,
        ..default()
    }
}

fn spacer_node(height: f32) -> Node {
    Node {
        height: Val::Px(height),
        flex_shrink: 0.0,
        ..default()
    }
}

/// Applies pending edits and keeps nodes on the blocks near the screen.
pub(crate) fn sync_blocks(
    mut documents: Query<(Entity, &mut Markdown)>,
    parents: Query<&ChildOf>,
    areas: Query<(&ComputedNode, &ScrollPosition)>,
    mut sinks: Sinks,
) {
    for (root, mut markdown) in &mut documents {
        // Change detection is for what callers do to the document: the
        // plugin's own bookkeeping is not a change.
        let markdown = markdown.bypass_change_detection();
        if let Ok(node) = sinks.nodes.get(root) {
            markdown.padding = Sides {
                left: px(node.padding.left),
                right: px(node.padding.right),
            };
        }
        let repin = markdown.advance(&mut sinks);

        let view = scroll_area(root, &parents, |e| areas.contains(e))
            .and_then(|area| areas.get(area).ok())
            .map(|(computed, scroll)| {
                let height = f64::from(computed.size.y * computed.inverse_scale_factor);
                let height = if height > 0.0 {
                    height
                } else {
                    FALLBACK_VIEWPORT
                };
                let top = f64::from(scroll.y)
                    - f64::from(markdown.origin)
                    - f64::from(markdown.padding_top);
                (top, height)
            });
        let window = markdown.needed(view);
        let stale = markdown
            .live
            .iter()
            .any(|l| !window.contains(&l.index) && !sinks.pins.contains(l.built.root));
        if repin.is_some() || window != markdown.window || stale || markdown.layout_dirty {
            markdown.layout_dirty = false;
            markdown.rebuild(
                root,
                window,
                repin.as_deref().unwrap_or_default(),
                &mut sinks,
            );
        }
    }
}

/// Reads heights back from the layout and holds the scroll position on the
/// anchor block.
///
/// When the position has to move (blocks that entered above the anchor came
/// out a different height than estimated), the layout of this frame was done
/// with the old position, and the frame would be drawn with the anchor off by
/// the difference. Layout writes positions straight from the scroll position,
/// so the same shift is applied to the positions of everything inside the
/// scroll area: the frame is drawn as the next layout will lay it out, with no
/// second layout pass.
pub(crate) fn measure_blocks(
    mut documents: Query<(Entity, &mut Markdown, &Node, &ComputedNode)>,
    blocks: Query<(&ComputedNode, &Node), Without<Markdown>>,
    parents: Query<&ChildOf>,
    children: Query<&Children>,
    mut areas: Query<(&ComputedNode, &mut ScrollPosition), Without<Markdown>>,
    mut transforms: Query<&mut UiGlobalTransform>,
) {
    for (root, mut markdown, node, computed) in &mut documents {
        let markdown = markdown.bypass_change_detection();
        if computed.size.x > 0.0 {
            markdown.width = computed.content_box().width() * computed.inverse_scale_factor;
        }
        markdown.padding_top = px(node.padding.top) + px(node.border.top);

        let mut changed = false;
        for live in &markdown.live {
            let Ok((block, node)) = blocks.get(live.built.root) else {
                continue;
            };
            if block.size == Vec2::ZERO {
                continue; // Not laid out yet.
            }
            let height = block.size.y * block.inverse_scale_factor
                + px(node.margin.top)
                + px(node.margin.bottom);
            changed |= markdown.heights.set(
                live.index,
                Height {
                    px: height,
                    measured: true,
                },
            );
        }
        if changed {
            markdown.layout_dirty = true;
        }

        let Some(area) = scroll_area(root, &parents, |e| areas.contains(e)) else {
            continue;
        };
        let Ok((area_node, mut scroll)) = areas.get_mut(area) else {
            continue;
        };
        let (Ok(transform), Ok(area_transform)) = (transforms.get(root), transforms.get(area))
        else {
            continue;
        };
        let scale = area_node.inverse_scale_factor;
        let node_top = transform.translation.y - computed.size.y / 2.0;
        let area_top = area_transform.translation.y - area_node.size.y / 2.0;
        markdown.origin = (node_top - area_top + area_node.scroll_position.y) * scale;

        let y = scroll.y;
        let first_block = f64::from(markdown.origin + markdown.padding_top);
        let moved = markdown.anchor.is_none() || (y - markdown.last_scroll).abs() > 0.5;
        if moved {
            // The caller scrolled: move the anchor by as much, through the
            // blocks between, or find it from scratch the first time.
            let from = match markdown.anchor {
                Some(anchor) => {
                    markdown.heights.top(anchor.block)
                        + f64::from(anchor.offset)
                        + f64::from(y - markdown.last_scroll)
                }
                None => f64::from(y) - first_block,
            };
            let block = markdown.heights.at(from);
            let offset = from - markdown.heights.top(block);
            markdown.anchor = Some(ScrollAnchor {
                block,
                offset: offset as f32,
            });
            markdown.last_scroll = y;
        }
        let Some(anchor) = markdown.anchor else {
            continue;
        };
        // Where the scroll position has to be for the anchor to be where it
        // was: different from `y` when something above it changed height.
        let wanted = first_block + markdown.heights.top(anchor.block) + f64::from(anchor.offset);
        if (wanted - f64::from(y)).abs() > 0.5 && wanted >= 0.0 {
            scroll.y = wanted as f32;
            markdown.last_scroll = wanted as f32;
            markdown.layout_dirty = true;

            // The positions this frame's layout wrote, as the new scroll
            // position would have put them (physical pixels).
            let shift = (wanted - f64::from(y)) as f32 / scale;
            for entity in children.iter_descendants(area) {
                if let Ok(mut transform) = transforms.get_mut(entity) {
                    let mut moved: Affine2 = **transform;
                    moved.translation.y -= shift;
                    *transform = moved.into();
                }
            }
        }
    }
}
