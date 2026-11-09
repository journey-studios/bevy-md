//! The caret on a blank line between blocks.
//!
//! Markdown has no empty paragraph: after Enter at the end of a block the
//! caret is on the blank line that separates it from the next block, which
//! belongs to no block, and Bevy draws a text cursor only inside a text. This
//! draws one more thing, a single node that stands where the new block will
//! be: halfway between the block above and the block below (or just under the
//! last one), at the text's left edge, as tall as a line of the block above.
//! It blinks with the caret and is hidden whenever the caret is inside a
//! block, where Bevy draws it. Typing there starts the next block.

use bevy::prelude::*;
use bevy::time::Real;
use bevy::ui::{ComputedNode, GlobalZIndex, UiGlobalTransform, UiSystems};
use bevy_md::Markdown;
use parley::{Affinity, Cursor};

use crate::{BlockTexts, CaretStyle, MarkdownEditor, edit};

/// Adds the gap caret; part of [`crate::MarkdownEditorPlugin`].
pub(crate) struct GapCaretPlugin;

impl Plugin for GapCaretPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, show_gap_caret.after(UiSystems::PostLayout));
    }
}

/// The node that draws the caret between blocks. A root node of the UI, so
/// the document's own children are left alone.
#[derive(Component, Debug)]
pub struct GapCaret;

/// Where the gap caret goes, in logical window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    left: f32,
    top: f32,
    height: f32,
}

/// Where the caret between blocks is drawn, or `None` when the caret is in a
/// block (or the block above it is not on screen).
fn place(
    editor: &MarkdownEditor,
    markdown: &Markdown,
    blocks: &BlockTexts,
    nodes: &Query<(&ComputedNode, &UiGlobalTransform), Without<GapCaret>>,
) -> Option<Place> {
    if markdown.is_parsing() {
        return None;
    }
    let document = markdown.document();
    let caret = editor.caret;
    if edit::caret_text(document, caret).is_some() {
        return None;
    }
    // The last block that starts at or before the caret: the one above it.
    let (mut low, mut high) = (0, document.len());
    while low < high {
        let middle = usize::midpoint(low, high);
        if document
            .block(middle)
            .is_some_and(|block| block.span().start <= caret)
        {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    let above = low.checked_sub(1)?;
    let block = document.block(above)?;
    // A caret inside a block with no source map is not between blocks.
    if block.span().end > caret {
        return None;
    }

    let (root, text) = (
        markdown.block_entity(above)?,
        blocks.entity(markdown, above)?,
    );
    let (root_node, root_transform) = nodes.get(root).ok()?;
    let (text_node, text_transform) = nodes.get(text).ok()?;
    let layout = blocks.layout(markdown, above)?;
    let end = block.content.text.len();
    let line = Cursor::from_byte_index(layout, end, Affinity::Upstream).geometry(layout, 1.0);
    let height = (line.y1 - line.y0) as f32;

    let bottom = root_transform.translation.y + root_node.size().y / 2.0;
    let top = markdown
        .block_entity(above + 1)
        .and_then(|below| nodes.get(below).ok())
        .map(|(node, transform)| transform.translation.y - node.size().y / 2.0);
    let y = match top {
        // Halfway between the two blocks.
        Some(top) => f32::midpoint(bottom, top) - height / 2.0,
        // Under the last one, close to it.
        None => bottom + height * 0.4,
    };
    let x = text_transform
        .transform_point2(text_node.content_box().min)
        .x;
    let scale = text_node.inverse_scale_factor();
    Some(Place {
        left: x * scale,
        top: y * scale,
        height: height * scale,
    })
}

// A Bevy system: each parameter is a distinct piece of the world it reads.
#[allow(clippy::too_many_arguments)]
fn show_gap_caret(
    mut commands: Commands,
    time: Res<Time<Real>>,
    style: Res<CaretStyle>,
    editor: Single<(&MarkdownEditor, &Markdown)>,
    blocks: BlockTexts,
    nodes: Query<(&ComputedNode, &UiGlobalTransform), Without<GapCaret>>,
    mut carets: Query<(&mut Node, &mut Visibility, &mut BackgroundColor), With<GapCaret>>,
    // The one node, once it has been needed.
    mut spawned: Local<Option<Entity>>,
) {
    let (editor, markdown) = *editor;
    let place = place(editor, markdown, &blocks, &nodes);

    // The blink of the caret Bevy draws, from the same clock.
    let period = style.blink_period.as_secs_f32().max(f32::EPSILON);
    let phase = time
        .elapsed()
        .saturating_sub(editor.active_since)
        .as_secs_f32()
        % period;
    let visible = place.is_some() && phase < period / 2.0;

    let Some(entity) = *spawned else {
        if let Some(place) = place {
            *spawned = Some(
                commands
                    .spawn((
                        GapCaret,
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(place.left),
                            top: px(place.top),
                            width: px(style.width),
                            height: px(place.height),
                            ..default()
                        },
                        BackgroundColor(style.color),
                        GlobalZIndex(1_000),
                        Visibility::Hidden,
                    ))
                    .id(),
            );
        }
        return;
    };
    let Ok((mut node, mut visibility, mut color)) = carets.get_mut(entity) else {
        return;
    };
    if let Some(place) = place {
        let (left, top) = (px(place.left), px(place.top));
        let (width, height) = (px(style.width), px(place.height));
        if node.left != left || node.top != top || node.width != width || node.height != height {
            node.left = left;
            node.top = top;
            node.width = width;
            node.height = height;
        }
        if color.0 != style.color {
            color.0 = style.color;
        }
    }
    let wanted = if visible {
        Visibility::Visible
    } else {
        Visibility::Hidden
    };
    if *visibility != wanted {
        *visibility = wanted;
    }
}
