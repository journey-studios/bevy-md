//! Block controls in the margin, one entity for the whole document (G9).
//!
//! A page editor shows a `+` and a drag handle beside the block under the
//! pointer. Giving every block its own pair would cost two nodes (and their
//! layout) per block, hidden most of the time; here there is one set of
//! controls per document, moved to whichever block has the pointer, and
//! removed when the pointer leaves the document or the block it was on
//! scrolls out of the window. The document has either no gutter entity or
//! exactly one.
//!
//! This is only how the controls look and where they sit: dragging and the
//! menu come with the editor's next phase. Add [`MarkdownGutter`] to a
//! [`Markdown`] entity to turn the controls on.

use bevy::prelude::*;
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;

use crate::theme::MarkdownTheme;
use crate::virtualize::{px, scroll_area};
use crate::{Markdown, MarkdownSystems};

/// Width of the controls, margin included, in logical pixels. They sit in the
/// left padding of the [`Markdown`] node, ending where the blocks begin; a
/// node with less padding than this lets them hang out to the left.
pub const GUTTER_WIDTH: f32 = 48.0;
/// Height of the controls: about one line of body text.
const GUTTER_HEIGHT: f32 = 24.0;
/// Side of the `+` button and of the drag handle.
const BUTTON: f32 = 20.0;
/// Side of one of the six dots of the drag handle.
const DOT: f32 = 3.0;

/// Shows block controls in the left margin of the block under the pointer.
/// Put it on the entity of a [`Markdown`].
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct MarkdownGutter;

/// The controls of a document: at most one entity has this per
/// [`Markdown`]. Its children are the button and the handle.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GutterControls {
    /// The block the controls sit beside.
    pub block: usize,
}

/// A part of the controls (the button, the handle, a dot of the handle).
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct GutterPart;

/// Where the pointer is, in physical window pixels, or `None` when it is
/// outside the window. The plugin writes it from the primary window's
/// cursor; write it yourself to simulate a pointer, or when the window is not
/// the primary one.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq)]
pub struct GutterPointer(pub Option<Vec2>);

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<GutterPointer>().add_systems(
        PostUpdate,
        (track_pointer, update_gutter)
            .chain()
            .after(MarkdownSystems::Measure),
    );
}

/// Copies the primary window's cursor to [`GutterPointer`], touching the
/// resource only when it moved.
fn track_pointer(
    window: Option<Single<&Window, With<PrimaryWindow>>>,
    mut out: ResMut<GutterPointer>,
) {
    if let Some(window) = window {
        let position = window.physical_cursor_position();
        if out.0 != position {
            out.0 = position;
        }
    }
}

/// The block of `markdown` under `pointer` (physical window pixels), when
/// there is one with a node.
fn block_under(
    markdown: &Markdown,
    node: &Node,
    computed: &ComputedNode,
    transform: &UiGlobalTransform,
    pointer: Vec2,
) -> Option<usize> {
    let local = transform.try_inverse()?.transform_point2(pointer);
    let half = computed.size / 2.0;
    // The controls hang in the margin, which is part of the document for
    // this purpose: the pointer moving onto them keeps their block.
    let reach = GUTTER_WIDTH / computed.inverse_scale_factor;
    if local.x < -half.x - reach || local.x > half.x || local.y.abs() > half.y {
        return None;
    }
    let y = (local.y + half.y) * computed.inverse_scale_factor
        - px(node.padding.top)
        - px(node.border.top);
    if y < 0.0 || f64::from(y) >= markdown.heights.total() {
        return None;
    }
    let block = markdown.heights.at(f64::from(y));
    markdown.block_entity(block).map(|_| block)
}

/// Whether `pointer` is over the node `area`, which clips the document.
fn over(area: (&ComputedNode, &UiGlobalTransform), pointer: Vec2) -> bool {
    let (computed, transform) = area;
    let Some(inverse) = transform.try_inverse() else {
        return false;
    };
    let local = inverse.transform_point2(pointer);
    let half = computed.size / 2.0;
    local.x.abs() <= half.x && local.y.abs() <= half.y
}

/// Moves the controls to the block under the pointer, makes them if there are
/// none and takes them away when there is no such block.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_gutter(
    pointer: Res<GutterPointer>,
    ui_scale: Res<UiScale>,
    theme: Res<MarkdownTheme>,
    mut commands: Commands,
    mut documents: Query<
        (
            Entity,
            &mut Markdown,
            &Node,
            &ComputedNode,
            &UiGlobalTransform,
        ),
        With<MarkdownGutter>,
    >,
    parents: Query<&ChildOf>,
    areas: Query<
        (&ComputedNode, &UiGlobalTransform),
        (With<bevy::ui::ScrollPosition>, Without<Markdown>),
    >,
    mut controls: Query<(&mut Node, &mut GutterControls), Without<Markdown>>,
    mut without: Query<&mut Markdown, Without<MarkdownGutter>>,
) {
    // Controls left behind by a document that stopped asking for them.
    for mut markdown in &mut without {
        if markdown.gutter.is_some() {
            let markdown = markdown.bypass_change_detection();
            if let Some(entity) = markdown.gutter.take() {
                commands.entity(entity).despawn();
                markdown.children.retain(|&child| child != entity);
            }
        }
    }
    for (root, mut markdown, node, computed, transform) in &mut documents {
        let markdown = markdown.bypass_change_detection();
        let hovered = pointer.0.and_then(|pointer| {
            let pointer = pointer / ui_scale.0;
            let clipped = scroll_area(root, &parents, |e| areas.contains(e))
                .and_then(|area| areas.get(area).ok())
                .is_some_and(|area| !over(area, pointer));
            if clipped {
                return None;
            }
            block_under(markdown, node, computed, transform, pointer)
        });

        match (hovered, markdown.gutter) {
            (Some(block), Some(entity)) => {
                if let Ok((mut style, mut at)) = controls.get_mut(entity) {
                    let top = Val::Px(px(node.padding.top) + markdown.heights.top(block) as f32);
                    if style.top != top {
                        style.top = top;
                    }
                    if at.block != block {
                        at.block = block;
                    }
                }
            }
            (Some(block), None) => {
                let entity = spawn_controls(
                    &mut commands,
                    root,
                    block,
                    px(node.padding.top) + markdown.heights.top(block) as f32,
                    px(node.padding.left),
                    &theme,
                );
                markdown.gutter = Some(entity);
                // Last child, where `arrange` keeps it.
                markdown.children.push(entity);
            }
            (None, Some(entity)) => {
                commands.entity(entity).despawn();
                markdown.children.retain(|&child| child != entity);
                markdown.gutter = None;
            }
            (None, None) => {}
        }
    }
}

/// The controls: a row with the `+` button and the drag handle (two columns
/// of three dots), as children of the document node.
fn spawn_controls(
    commands: &mut Commands,
    root: Entity,
    block: usize,
    top: f32,
    padding_left: f32,
    theme: &MarkdownTheme,
) -> Entity {
    let button = commands
        .spawn((
            GutterPart,
            Node {
                width: Val::Px(BUTTON),
                height: Val::Px(BUTTON),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
        ))
        .with_child((
            GutterPart,
            Text::new("+"),
            TextFont {
                font: theme.body_font.clone(),
                font_size: bevy::text::FontSize::Px(theme.body_size),
                ..default()
            },
            TextColor(theme.muted),
        ))
        .id();
    let handle = commands
        .spawn((
            GutterPart,
            Node {
                width: Val::Px(BUTTON),
                height: Val::Px(BUTTON),
                display: Display::Grid,
                grid_template_columns: RepeatedGridTrack::px(2, DOT),
                justify_content: JustifyContent::Center,
                align_content: AlignContent::Center,
                column_gap: Val::Px(DOT),
                row_gap: Val::Px(DOT),
                ..default()
            },
        ))
        .id();
    for _ in 0..6 {
        commands.spawn((
            GutterPart,
            Node {
                width: Val::Px(DOT),
                height: Val::Px(DOT),
                border_radius: BorderRadius::MAX,
                ..default()
            },
            BackgroundColor(theme.muted),
            ChildOf(handle),
        ));
    }
    commands
        .spawn((
            GutterControls { block },
            GutterPart,
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(top),
                left: Val::Px(padding_left - GUTTER_WIDTH),
                width: Val::Px(GUTTER_WIDTH),
                height: Val::Px(GUTTER_HEIGHT),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::FlexEnd,
                align_items: AlignItems::Center,
                column_gap: Val::Px(2.0),
                padding: UiRect::right(Val::Px(6.0)),
                ..default()
            },
            ChildOf(root),
        ))
        .add_children(&[button, handle])
        .id()
}
