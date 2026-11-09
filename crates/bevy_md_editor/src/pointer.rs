//! The mouse: clicks that count, drags and Shift+click.
//!
//! The left button is read into [`PointerInput`] messages (and a test or an
//! app can write them itself), which are applied to the editor's selection
//! with [`MarkdownEditor::set_selection`]:
//!
//! - **One click** puts the caret under the pointer.
//! - **Two clicks** (close in time and place, see [`ClickSettings`]) select
//!   the word under the pointer, by the word boundaries of the block's layout.
//! - **Three clicks** select the whole text of the block under the pointer,
//!   as Notion does. This is the block's *text*, not its source span: the
//!   `# ` of a heading or the `- ` of a list item stay out of it, so typing
//!   replaces the text and keeps the kind of block. More clicks keep it.
//! - **Dragging** with the button down extends the selection from where the
//!   press began to the point under the pointer, in any block: a point that
//!   is not over any text counts as the nearest block vertically. After two
//!   or three clicks the drag grows by words or by blocks.
//! - **Shift+click** extends the selection from the current anchor to the
//!   point clicked, by characters whatever the number of clicks.
//!
//! A press is hit-tested against the block under it only. Everything is
//! computed on the layout Bevy has already made, and mapped to the markdown
//! source with the source map of `bevy_md_core`, so a selected word is the
//! bytes of that word in the source: the `**` around a bold word are not
//! part of it.

use std::ops::Range;
use std::time::Duration;

use bevy::ecs::system::SystemParam;
use bevy::input::keyboard::Key;
use bevy::input::mouse::MouseButton;
use bevy::prelude::*;
use bevy::text::{ComputedTextBlock, TextBrush};
use bevy::time::Real;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use bevy_md::{BlockText, Markdown};
use parley::{Affinity, Cursor, Layout, Selection};

use crate::{BlockTexts, EditMetrics, MarkdownEditor, edit, handle_input, place_caret};

/// What the left mouse button does, in the order it happens. The mouse
/// writes these; a test or an app can write them too. Positions are in
/// physical window pixels from the top-left corner.
#[derive(Message, Clone, Copy, Debug, PartialEq)]
pub enum PointerInput {
    /// The button went down: a click (one, two or three of them in a row,
    /// see [`ClickSettings`]), or Shift+click when `shift` is set.
    Press {
        /// Where the pointer is.
        position: Vec2,
        /// Whether Shift is held: extend the selection instead of starting
        /// a new one.
        shift: bool,
    },
    /// The pointer moved with the button down. Ignored when no press
    /// started a drag.
    Drag {
        /// Where the pointer is.
        position: Vec2,
    },
    /// The button went up: the drag, if any, is over.
    Release,
}

/// When two clicks count as a double click, and so on.
#[derive(Resource, Clone, Copy, Debug)]
pub struct ClickSettings {
    /// The most real time between two presses for the second to continue
    /// the first. [`Duration::ZERO`] turns double and triple clicks off.
    pub multi_click_window: Duration,
    /// The most the pointer may be from the previous press, in physical
    /// pixels, for the press to continue it.
    pub multi_click_distance: f32,
}

impl Default for ClickSettings {
    fn default() -> Self {
        Self {
            multi_click_window: Duration::from_millis(450),
            multi_click_distance: 6.0,
        }
    }
}

/// Adds the mouse systems; part of [`crate::MarkdownEditorPlugin`].
pub(crate) struct PointerPlugin;

impl Plugin for PointerPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClickSettings>()
            .init_resource::<PointerState>()
            .add_message::<PointerInput>()
            .add_systems(
                Update,
                (mouse_to_pointer_input, apply_pointer_input)
                    .chain()
                    .after(place_caret)
                    .before(handle_input),
            );
    }
}

/// How far a press or a drag reaches around its starting point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Granularity {
    Char,
    Word,
    Block,
}

/// A drag in progress.
#[derive(Clone, Debug)]
struct Drag {
    granularity: Granularity,
    /// The source bytes the press selected (empty for a caret): the part of
    /// the selection that stays whichever way the drag goes.
    origin: Range<usize>,
}

/// What the mouse has been doing, for counting clicks and dragging.
#[derive(Resource, Default, Debug)]
struct PointerState {
    /// When and where the last press was.
    last_press: Option<(Duration, Vec2)>,
    /// How many presses in a row it was, 1 to 3.
    clicks: u8,
    drag: Option<Drag>,
}

impl PointerState {
    /// Record a press and say how many in a row it makes.
    fn press(&mut self, now: Duration, position: Vec2, settings: &ClickSettings) -> u8 {
        let continues = self.last_press.is_some_and(|(at, from)| {
            now.saturating_sub(at) < settings.multi_click_window
                && from.distance(position) <= settings.multi_click_distance
        });
        self.clicks = if continues {
            (self.clicks + 1).min(3)
        } else {
            1
        };
        self.last_press = Some((now, position));
        self.clicks
    }

    /// Forget the clicks and the drag.
    fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Turns the left button into [`PointerInput`].
fn mouse_to_pointer_input(
    buttons: Res<ButtonInput<MouseButton>>,
    keys: Res<ButtonInput<Key>>,
    window: Single<&Window, With<PrimaryWindow>>,
    mut last: Local<Option<Vec2>>,
    mut inputs: MessageWriter<PointerInput>,
) {
    let position = window.physical_cursor_position();
    if buttons.just_pressed(MouseButton::Left)
        && let Some(position) = position
    {
        *last = Some(position);
        inputs.write(PointerInput::Press {
            position,
            shift: keys.pressed(Key::Shift),
        });
    } else if buttons.pressed(MouseButton::Left)
        && let Some(position) = position
        && *last != Some(position)
    {
        *last = Some(position);
        inputs.write(PointerInput::Drag { position });
    }
    if buttons.just_released(MouseButton::Left) {
        *last = None;
        inputs.write(PointerInput::Release);
    }
}

/// A point over the text of a block.
pub(crate) struct Hit {
    pub(crate) block: usize,
    /// The block's text entity.
    entity: Entity,
    /// In the space of the block's layout.
    pub(crate) point: Vec2,
}

/// Finds the text under a point and reads it.
#[derive(SystemParam)]
pub(crate) struct Hits<'w, 's> {
    ui_scale: Res<'w, UiScale>,
    blocks: BlockTexts<'w, 's>,
    /// The node of the document: a click anywhere in it is a click on the
    /// text, not only a click on a glyph.
    areas: Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform), With<MarkdownEditor>>,
    texts: Query<
        'w,
        's,
        (
            &'static ComputedTextBlock,
            &'static ComputedNode,
            &'static UiGlobalTransform,
        ),
        With<BlockText>,
    >,
}

impl Hits<'_, '_> {
    /// Whether `position` (physical window pixels) is anywhere in the node
    /// of the document, its padding and the empty part of a line included.
    pub(crate) fn in_area(&self, position: Vec2) -> bool {
        self.areas.iter().any(|(node, transform)| {
            let Some(inverse) = transform.try_inverse() else {
                return false;
            };
            // The same mapping as `locate`: the node's own space, centered.
            let local = inverse.transform_point2(position / self.ui_scale.0);
            let half = node.size() / 2.0;
            local.x.abs() <= half.x && local.y.abs() <= half.y
        })
    }

    /// The block a click at `position` lands on: the text under it, or else,
    /// anywhere in the document's node, the nearest block (so a click to the
    /// right of a short line, or in the space between blocks, still puts the
    /// caret on that line instead of being lost).
    pub(crate) fn locate_click(&self, markdown: &Markdown, position: Vec2) -> Option<Hit> {
        self.locate(markdown, position, false).or_else(|| {
            self.in_area(position)
                .then(|| self.locate(markdown, position, true))
                .flatten()
        })
    }

    /// Whether the point of `hit` is over text that is a link (or an
    /// image), by the glyphs' own rectangles: the empty part of the line
    /// after a link is not.
    pub(crate) fn over_link(&self, markdown: &Markdown, hit: &Hit) -> bool {
        let Some(block) = markdown.document().block(hit.block) else {
            return false;
        };
        let Some(layout) = self.layout(hit) else {
            return false;
        };
        let Vec2 { x, y } = hit.point;
        block
            .content
            .runs
            .iter()
            .filter(|run| run.link.is_some() && run.start < run.end)
            .any(|run| {
                let selection = Selection::new(
                    Cursor::from_byte_index(layout, run.start as usize, Affinity::Downstream),
                    Cursor::from_byte_index(layout, run.end as usize, Affinity::Upstream),
                );
                selection.geometry(layout).iter().any(|(bounds, _)| {
                    f64::from(x) >= bounds.x0
                        && f64::from(x) <= bounds.x1
                        && f64::from(y) >= bounds.y0
                        && f64::from(y) <= bounds.y1
                })
            })
    }

    /// The block under `position` (physical window pixels). With `nearest`
    /// a point over no text gives the block nearest to it vertically (then
    /// horizontally): what a drag needs when the pointer leaves the text.
    pub(crate) fn locate(&self, markdown: &Markdown, position: Vec2, nearest: bool) -> Option<Hit> {
        let mut best: Option<((f32, f32), Hit)> = None;
        for (block, _) in markdown.live_blocks() {
            let Some(entity) = self.blocks.entity(markdown, block) else {
                continue;
            };
            let Ok((_, node, transform)) = self.texts.get(entity) else {
                continue;
            };
            let Some(inverse) = transform.try_inverse() else {
                continue;
            };
            // The same mapping `bevy_ui_widgets` uses for `TextInput`:
            // window pixels to the node's own space, centered on the node.
            let local = inverse.transform_point2(position / self.ui_scale.0);
            let content = node.content_box();
            if !nearest && !content.contains(local) {
                continue;
            }
            let away = (
                (content.min.y - local.y)
                    .max(local.y - content.max.y)
                    .max(0.0),
                (content.min.x - local.x)
                    .max(local.x - content.max.x)
                    .max(0.0),
            );
            if best.as_ref().is_none_or(|(known, _)| away < *known) {
                let hit = Hit {
                    block,
                    entity,
                    point: local - content.min,
                };
                best = Some((away, hit));
            }
            if !nearest {
                break;
            }
        }
        best.map(|(_, hit)| hit)
    }

    fn layout(&self, hit: &Hit) -> Option<&Layout<TextBrush>> {
        let (computed, ..) = self.texts.get(hit.entity).ok()?;
        Some(computed.buffer())
    }

    /// The source bytes of the unit of `granularity` under `hit`: a caret
    /// (an empty range) for a character, the word, or the whole text of the
    /// block. A word or a block with no text to select is a caret.
    fn unit_at(
        &self,
        markdown: &Markdown,
        hit: &Hit,
        granularity: Granularity,
    ) -> Option<Range<usize>> {
        let block = markdown.document().block(hit.block)?;
        let layout = self.layout(hit)?;
        let Vec2 { x, y } = hit.point;
        let text = match granularity {
            Granularity::Char => None,
            Granularity::Word => Some(Selection::word_from_point(layout, x, y).text_range()),
            Granularity::Block => Some(0..block.content.text.len()),
        };
        if let Some(text) = text
            && let Some(source) = block.source_range(text)
            && !source.is_empty()
        {
            return Some(source);
        }
        let at = edit::source_caret(&block, Cursor::from_point(layout, x, y).index())?;
        Some(at..at)
    }
}

/// The selection a drag has when the pointer is over `unit`: from the unit
/// the press selected to the one under the pointer, backwards when the
/// pointer is before it. Returns `(anchor, caret)`.
fn extend(origin: &Range<usize>, unit: &Range<usize>) -> (usize, usize) {
    if unit.start < origin.start {
        (origin.end, unit.start)
    } else {
        (origin.start, unit.end)
    }
}

fn apply_pointer_input(
    mut inputs: MessageReader<PointerInput>,
    settings: Res<ClickSettings>,
    time: Res<Time<Real>>,
    mut state: ResMut<PointerState>,
    mut metrics: ResMut<EditMetrics>,
    editor: Single<(&mut MarkdownEditor, &Markdown)>,
    hits: Hits,
) {
    let (mut editor, markdown) = editor.into_inner();
    for input in inputs.read() {
        match *input {
            PointerInput::Press { position, shift } => {
                if markdown.is_parsing() {
                    // The pixels belong to the document being replaced, and
                    // the selection is made of bytes of the source that is
                    // replacing it.
                    metrics.clicks_ignored += 1;
                    state.reset();
                    continue;
                }
                let Some(hit) = hits.locate_click(markdown, position) else {
                    state.reset();
                    continue;
                };
                let now = time.elapsed();
                let clicks = state.press(now, position, &settings);
                let granularity = match (shift, clicks) {
                    (true, _) | (_, 1) => Granularity::Char,
                    (false, 2) => Granularity::Word,
                    (false, _) => Granularity::Block,
                };
                let Some(unit) = hits.unit_at(markdown, &hit, granularity) else {
                    continue;
                };
                let (origin, selection) = if shift {
                    // Shift+click is not part of a run of clicks.
                    state.last_press = None;
                    let anchor = editor.anchor();
                    (anchor..anchor, (anchor, unit.end))
                } else {
                    (unit.clone(), (unit.start, unit.end))
                };
                editor.set_selection(markdown, selection.0, selection.1);
                editor.active_since = now;
                state.drag = Some(Drag {
                    granularity: if unit.is_empty() {
                        Granularity::Char
                    } else {
                        granularity
                    },
                    origin,
                });
            }
            PointerInput::Drag { position } => {
                let Some(drag) = &state.drag else {
                    continue;
                };
                if markdown.is_parsing() {
                    state.reset();
                    continue;
                }
                let Some(hit) = hits.locate(markdown, position, true) else {
                    continue;
                };
                let Some(unit) = hits.unit_at(markdown, &hit, drag.granularity) else {
                    continue;
                };
                let (anchor, caret) = extend(&drag.origin, &unit);
                let before = (editor.anchor, editor.caret);
                editor.set_selection(markdown, anchor, caret);
                if (editor.anchor, editor.caret) != before {
                    editor.active_since = time.elapsed();
                }
            }
            PointerInput::Release => state.drag = None,
        }
    }
}
