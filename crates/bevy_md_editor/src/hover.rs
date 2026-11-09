//! The mouse cursor: an I-beam where a click puts the caret, a hand over a
//! link.
//!
//! "Where a click puts the caret" is the whole node of the document, not
//! only the glyphs: a click to the right of a short line, or in the space
//! between two blocks, lands on the nearest line (see
//! [`crate::pointer`]), so the pointer says so there too. Over the text of a
//! link, by the glyphs' own rectangles, it is a hand. Outside the document
//! the window gets its default cursor back.
//!
//! The icon is the window's [`CursorIcon`], written only when it changes.
//! An app that also uses Bevy's `CursorIconPlugin` (`bevy_feathers`) has a
//! system that sets the window's icon from the hovered entity every frame;
//! it wins over this one unless the app sets `OverrideCursor`.

use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use bevy_md::Markdown;

use crate::MarkdownEditor;
use crate::pointer::Hits;

/// Adds the cursor icon system; part of [`crate::MarkdownEditorPlugin`].
pub(crate) struct HoverPlugin;

impl Plugin for HoverPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, update_cursor_icon);
    }
}

/// What the pointer is over: the icon the window should show, or `None` for
/// the window's own.
fn icon_at(markdown: &Markdown, hits: &Hits, position: Vec2) -> Option<SystemCursorIcon> {
    if !hits.in_area(position) {
        return None;
    }
    let link = hits
        .locate(markdown, position, false)
        .is_some_and(|hit| hits.over_link(markdown, &hit));
    Some(if link {
        SystemCursorIcon::Pointer
    } else {
        SystemCursorIcon::Text
    })
}

fn update_cursor_icon(
    mut commands: Commands,
    window: Single<(Entity, &Window), With<PrimaryWindow>>,
    markdown: Single<&Markdown, With<MarkdownEditor>>,
    hits: Hits,
    // What this system put on the window, so that it only takes back what
    // it set.
    mut shown: Local<Option<SystemCursorIcon>>,
) {
    let (entity, window) = *window;
    // A pointer outside the window is outside the document too.
    let wanted = window
        .physical_cursor_position()
        .and_then(|position| icon_at(&markdown, &hits, position));
    if wanted == *shown {
        return;
    }
    commands
        .entity(entity)
        .insert(wanted.map_or_else(CursorIcon::default, CursorIcon::from));
    *shown = wanted;
}
