//! Markdown for Bevy UI.
//!
//! Put a [`Markdown`] component on a UI node and change it through its
//! methods. Every change reports which blocks it replaced, and the plugin
//! rebuilds only those: a streamed token touches the last block, a keystroke
//! the block it lands in.
//!
//! Only the blocks near the screen have entities. Inside a scrollable node
//! (anything with a [`ScrollPosition`](bevy::ui::ScrollPosition)) the blocks above and below the
//! visible ones are two spacers as tall as the blocks they stand for, so a
//! document of a hundred thousand blocks costs what one of a hundred does.
//! Outside a scrollable node every block is shown.
//!
//! ```no_run
//! use bevy::prelude::*;
//! use bevy_md::{Markdown, MarkdownPlugin};
//!
//! App::new()
//!     .add_plugins((DefaultPlugins, MarkdownPlugin))
//!     .add_systems(Startup, |mut commands: Commands| {
//!         commands.spawn(Camera2d);
//!         commands.spawn(Markdown::new("# Hello\n\nFrom **bevy_md**."));
//!     })
//!     .run();
//! ```

mod background;
mod gutter;
mod heights;
mod render;
mod theme;
mod virtualize;

use std::ops::Range;
use std::sync::Arc;

pub use background::ParseTimings;
use background::{Background, SYNC_LIMIT};
use bevy::prelude::*;
pub use bevy_md_core as core;
use bevy_md_core::{Document, Splices};
pub use gutter::{GUTTER_WIDTH, GutterControls, GutterPart, GutterPointer, MarkdownGutter};
use heights::Heights;
use render::{Built, Signature};
pub use theme::MarkdownTheme;

/// System sets of [`MarkdownPlugin`], in [`PostUpdate`].
#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MarkdownSystems {
    /// Applies edits, and spawns and despawns the nodes of the blocks that
    /// enter or leave the window around the screen.
    Sync,
    /// Reads the layout back: measured block heights replace estimates, and
    /// the scroll position follows the anchor block.
    Measure,
}

/// Renders [`Markdown`] nodes.
pub struct MarkdownPlugin;

impl Plugin for MarkdownPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MarkdownTheme>().add_systems(
            PostUpdate,
            (
                virtualize::sync_blocks
                    .in_set(MarkdownSystems::Sync)
                    .before(bevy::ui::UiSystems::Prepare),
                virtualize::measure_blocks
                    .in_set(MarkdownSystems::Measure)
                    .in_set(bevy::ui::UiSystems::PostLayout),
            ),
        );
        gutter::plugin(app);
    }
}

/// Marks the text entity of a block of prose (paragraph, heading, list item).
///
/// It is a child of the block's node, found through
/// [`Markdown::block_entity`], and the entity whose
/// [`ComputedTextBlock`](bevy::text::ComputedTextBlock) lays the block's
/// [`Inline::text`](bevy_md_core::Inline::text) out: byte `i` of the text is
/// byte `i` of the layout. An editor reads glyph geometry from it.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct BlockText;

/// Put on the node of a block to keep it alive while it is outside the
/// window around the screen, such as the block holding the caret. Without it
/// the node is despawned when the block scrolls far enough away. An edit
/// that replaces the block hands the marker to its replacement.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct Pinned;

/// Where a block sits in its document, kept on the node of every block that
/// has one. It is written only when it changes, so `Changed<BlockSurroundings>`
/// finds the blocks that moved, were renumbered or were re-indented, and
/// nothing else.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockSurroundings {
    /// Index of the block in the document.
    pub index: usize,
    /// The number of an ordered list item (`3.`).
    pub list_number: Option<u64>,
    /// How many lists the block is nested in.
    pub list_depth: u8,
    /// How many quotes the block is nested in.
    pub quote_depth: u8,
}

/// The block at the top of the screen and how far into it the screen starts,
/// in logical pixels. It is what stays put when blocks above it change
/// height, or are replaced.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScrollAnchor {
    /// Index of the block.
    pub block: usize,
    /// Pixels from the top of the block to the top of the screen. Negative
    /// above the first block.
    pub offset: f32,
}

/// A markdown document shown as a column of block nodes.
#[derive(Component)]
#[require(Node = column())]
pub struct Markdown {
    /// The document being shown. Shared with a parse running in the
    /// background (read only: nothing changes it meanwhile).
    document: Arc<Document>,
    /// A large change being parsed off the main thread.
    background: Background,
    /// Estimated heights of the blocks a background parse brought in, one
    /// index per splice built on the worker, for the next sync to hang into
    /// `heights` instead of estimating (and inserting) them on the main
    /// thread.
    estimates: Option<Vec<Heights>>,
    /// The frame being synced installs a background result (for the
    /// diagnostic timings only).
    timing_install: bool,
    /// What changed in the block list since the heights and nodes were last
    /// synced: the blocks that were replaced, as disjoint splices from the
    /// list the heights describe to the document's current one. Every change
    /// composes into it (see [`Splices`]), so edits made far apart, and the
    /// edits that change how many blocks there are, leave the blocks between
    /// them out.
    dirty: Splices,
    /// Height of every block, estimated or measured.
    heights: Heights,
    /// The blocks that have a node, by index.
    live: Vec<Live>,
    /// Subtrees of blocks that left the window, kept for the next ones.
    pool: Pool,
    /// Stand-ins for the blocks without nodes, as tall as their sum.
    spacers: Vec<Entity>,
    /// The children as last set, spacers included.
    children: Vec<Entity>,
    /// The hidden container the pool's subtrees are children of, and the
    /// children and padding it was last given.
    pool_root: Option<Entity>,
    pool_children: Vec<Entity>,
    pool_padding: Sides,
    /// The block controls, when the pointer is over a block (see
    /// [`MarkdownGutter`]); always the last child.
    gutter: Option<Entity>,
    /// The blocks the screen needs, as of the last sync.
    window: Range<usize>,
    /// Heights or live blocks changed since the spacers were sized.
    layout_dirty: bool,
    anchor: Option<ScrollAnchor>,
    /// The scroll position as of the last time the anchor was looked at.
    last_scroll: f32,
    /// Top of the node inside the scrolled content, in logical pixels.
    origin: f32,
    /// Space above the first block inside the node.
    padding_top: f32,
    /// Space left and right of the blocks inside the node, which the pool's
    /// container takes as insets so the subtrees in it keep their width.
    padding: Sides,
    /// Width available to blocks, for estimates.
    width: f32,
}

/// Space on the left and the right, in logical pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Sides {
    pub left: f32,
    pub right: f32,
}

/// A block with a node.
pub(crate) struct Live {
    pub index: usize,
    pub built: Built,
}

/// Subtrees waiting to be rebound to another block. They are children of one
/// hidden container that is out of the flow, and are laid out at the width
/// they had, so nothing about them is measured again until a block takes one.
#[derive(Default)]
pub(crate) struct Pool {
    built: Vec<Built>,
}

/// Subtrees kept at most: past this, a block leaving the window is despawned.
pub(crate) const POOL_MAX: usize = 192;

impl Pool {
    pub(crate) fn len(&self) -> usize {
        self.built.len()
    }

    pub(crate) fn put(&mut self, built: Built) {
        self.built.push(built);
    }

    /// A subtree of this shape for block `index`: the one that last showed
    /// that block if it is here (so that nothing needs writing), else the one
    /// whose last block is farthest from the window, since the nearest ones
    /// are the likeliest to be wanted back by scrolling.
    pub(crate) fn take(
        &mut self,
        signature: &Signature,
        index: usize,
        window: &Range<usize>,
    ) -> Option<Built> {
        let mut best: Option<(usize, usize)> = None;
        for (at, built) in self.built.iter().enumerate() {
            if built.signature != *signature {
                continue;
            }
            if built.last == Some(index) {
                return Some(self.built.remove(at));
            }
            let distance = built.last.map_or(usize::MAX, |last| {
                if last < window.start {
                    window.start - last
                } else {
                    last.saturating_sub(window.end) + 1
                }
            });
            if best.is_none_or(|(_, far)| distance > far) {
                best = Some((at, distance));
            }
        }
        Some(self.built.remove(best?.0))
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Built> {
        self.built.iter_mut()
    }
}

fn column() -> Node {
    Node {
        flex_direction: FlexDirection::Column,
        ..default()
    }
}

impl Markdown {
    /// A document showing `source`, parsed right here on the calling
    /// thread. To show a large text without stalling a frame, spawn an empty
    /// one and call [`Markdown::set_source`] from a system: it parses in the
    /// background above [`SYNC_LIMIT`](Markdown::SYNC_LIMIT) bytes.
    pub fn new(source: &str) -> Self {
        let mut markdown = Self {
            document: Arc::new(Document::new()),
            background: Background::default(),
            estimates: None,
            timing_install: false,
            dirty: Splices::new(),
            heights: Heights::default(),
            live: Vec::new(),
            pool: Pool::default(),
            spacers: Vec::new(),
            children: Vec::new(),
            pool_root: None,
            pool_children: Vec::new(),
            pool_padding: Sides::default(),
            gutter: None,
            window: 0..0,
            layout_dirty: false,
            anchor: None,
            last_scroll: 0.0,
            origin: 0.0,
            padding_top: 0.0,
            padding: Sides::default(),
            width: core::estimate::Metrics::default().width,
        };
        let splices = Arc::make_mut(&mut markdown.document).set_source(source);
        markdown.queue(splices);
        markdown
    }

    /// Changes adding more than this many bytes are parsed in the
    /// background; smaller ones are applied before the frame is drawn.
    pub const SYNC_LIMIT: usize = SYNC_LIMIT;

    /// The parsed document being shown. While a large change is parsing in
    /// the background ([`Markdown::is_parsing`]) it is the document as it was
    /// before that change.
    pub fn document(&self) -> &Document {
        &self.document
    }

    /// The source after every change made so far, including the ones a
    /// background parse has not brought to [`Markdown::document`] yet. Byte
    /// ranges passed to [`Markdown::edit`] refer to this text.
    pub fn source(&self) -> &str {
        self.background
            .logical()
            .unwrap_or_else(|| self.document.source())
    }

    /// Whether the document shown is behind [`Markdown::source`]: a large
    /// change is being parsed in the background, or changes made meanwhile
    /// are waiting for its result.
    pub fn is_parsing(&self) -> bool {
        self.background.busy()
    }

    /// Whether the nodes (and so the text layout) show [`Markdown::document`]
    /// as it is: `false` between a change made to the document and the
    /// plugin's next sync, which is when the nodes of the changed blocks are
    /// rebuilt. A caret that reads glyph geometry must not use the layout of
    /// a block while this is `false`: it is the text before the change.
    pub fn is_rendered(&self) -> bool {
        self.dirty.is_empty()
    }

    /// Block the calling thread until the background parse that is running
    /// has finished, so that the next frame installs its result. Does
    /// nothing when no parse is running.
    ///
    /// For tests and tools that need a deterministic hand-over (for example,
    /// to make a parse land in a chosen frame); an app should keep running
    /// frames instead. A parse still waiting for the sync that starts it is
    /// started here, with default height estimates (they are replaced by
    /// measured heights once the blocks are laid out).
    pub fn wait_for_parse(&mut self) {
        let metrics = core::estimate::Metrics {
            width: self.width,
            ..core::estimate::Metrics::default()
        };
        self.background.wait(&self.document, metrics);
    }

    /// How many large changes were made: the revision a background parse
    /// carries, which has to be the latest for its result to be used.
    pub fn parse_revision(&self) -> u64 {
        self.background.revision()
    }

    /// Where the time of the last large change went (diagnostics): see
    /// [`ParseTimings`].
    #[doc(hidden)]
    pub fn parse_timings(&self) -> ParseTimings {
        self.background.timings
    }

    /// Background parses whose results were thrown away because a newer
    /// change had come first.
    pub fn discarded_parses(&self) -> u64 {
        self.background.discarded()
    }

    /// The node of block `index`, when it has one: only blocks near the
    /// screen (and [`Pinned`] ones) do. After an edit it is stale until the
    /// plugin has run.
    pub fn block_entity(&self, index: usize) -> Option<Entity> {
        let at = self
            .live
            .binary_search_by_key(&index, |live| live.index)
            .ok()?;
        Some(self.live[at].built.root)
    }

    /// The blocks that have a node, with their indices, in document order.
    pub fn live_blocks(&self) -> impl ExactSizeIterator<Item = (usize, Entity)> + '_ {
        self.live.iter().map(|live| (live.index, live.built.root))
    }

    /// Offset in logical pixels of the top of block `index` from the top of
    /// the first block: measured heights for the blocks that have been laid
    /// out, estimates for the rest.
    pub fn block_top(&self, index: usize) -> f64 {
        self.heights.top(index.min(self.heights.len()))
    }

    /// Height of all the blocks together, in logical pixels, estimated where
    /// not measured.
    pub fn content_height(&self) -> f64 {
        self.heights.total()
    }

    /// The block the scroll position is anchored to, once known.
    pub fn scroll_anchor(&self) -> Option<ScrollAnchor> {
        self.anchor
    }

    /// Append streamed text. Chunks arriving while a large change is still
    /// parsing are merged and applied together once it lands.
    pub fn push_str(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        if self.background.busy() {
            self.background.append(text);
        } else if text.len() <= SYNC_LIMIT {
            let splices = Arc::make_mut(&mut self.document).push_str(text);
            self.queue(splices);
        } else {
            let started = std::time::Instant::now();
            let mut source = String::with_capacity(self.document.source().len() + text.len());
            source.push_str(self.document.source());
            source.push_str(text);
            self.background.begin(Arc::new(source));
            self.background.timings.source_copy = started.elapsed();
        }
    }

    /// Replace the source; text extending the current one is appended.
    /// A replacement larger than [`Markdown::SYNC_LIMIT`] is parsed in the
    /// background (and supersedes one already running).
    pub fn set_source(&mut self, source: &str) {
        if let Some(delta) = source.strip_prefix(self.source()) {
            let delta = delta.to_owned();
            self.push_str(&delta);
            return;
        }
        if source.len() <= SYNC_LIMIT {
            self.background.cancel();
            let replacement = Document::parse(source);
            self.replace_document(replacement);
        } else {
            let started = std::time::Instant::now();
            self.background.begin(Arc::new(source.to_owned()));
            self.background.timings.source_copy = started.elapsed();
        }
    }

    /// [`Markdown::set_source`] for a caller that has the text as an owned
    /// `String` and is done with it: a replacement larger than
    /// [`Markdown::SYNC_LIMIT`] is handed to the background parse as it is,
    /// without the copy of the whole text that `set_source` makes. This is
    /// the way to open a large text.
    pub fn set_source_owned(&mut self, source: String) {
        if let Some(delta) = source.strip_prefix(self.source())
            && delta.len() <= SYNC_LIMIT
        {
            let delta = delta.to_owned();
            self.push_str(&delta);
            return;
        }
        if source.len() <= SYNC_LIMIT {
            self.background.cancel();
            let replacement = Document::parse(source);
            self.replace_document(replacement);
        } else {
            let started = std::time::Instant::now();
            self.background.begin(Arc::new(source));
            self.background.timings.source_copy = started.elapsed();
        }
    }

    /// Replace the bytes `range` of the source with `text`. The range
    /// refers to [`Markdown::source`]. An edit inserting more than
    /// [`Markdown::SYNC_LIMIT`] bytes is parsed in the background.
    ///
    /// # Panics
    ///
    /// When `range` is out of bounds or not on character boundaries.
    pub fn edit(&mut self, range: Range<usize>, text: &str) {
        if self.background.busy() {
            let source = self.source();
            assert!(
                range.start <= range.end
                    && source.is_char_boundary(range.start)
                    && source.is_char_boundary(range.end),
                "edit range {range:?} is not a valid range of the source"
            );
            self.background.edit(range, text);
        } else if text.len() <= SYNC_LIMIT {
            let splices = Arc::make_mut(&mut self.document).edit(range, text);
            self.queue(splices);
        } else {
            let source = self.document.source();
            assert!(
                range.start <= range.end
                    && source.is_char_boundary(range.start)
                    && source.is_char_boundary(range.end),
                "edit range {range:?} is not a valid range of the source"
            );
            let started = std::time::Instant::now();
            let mut edited = String::with_capacity(source.len() - range.len() + text.len());
            edited.push_str(&source[..range.start]);
            edited.push_str(text);
            edited.push_str(&source[range.end..]);
            self.background.begin(Arc::new(edited));
            self.background.timings.source_copy = started.elapsed();
        }
    }

    /// Show `document` in place of the one being shown, which is dropped
    /// away from the main thread.
    fn replace_document(&mut self, document: Document) {
        let splices = Splices::everything(self.document.len(), document.len());
        let old = std::mem::replace(&mut self.document, Arc::new(document));
        if old.len() > 256 {
            background::drop_in_background(old);
        }
        self.queue(splices);
    }

    /// Note that the blocks `splices` names changed, after whatever was
    /// pending: the pending list now goes from the blocks as last synced to
    /// the current ones.
    fn queue(&mut self, splices: Splices) {
        if self.dirty.is_empty() {
            self.dirty = splices;
        } else {
            self.dirty.compose(&splices);
        }
    }
}
