//! Streams a markdown document into a `Markdown` node a few bytes per frame,
//! the way a model's answer arrives. Press R to restart.
//!
//! `cargo run --example streaming -- --blocks 100000` opens a document of
//! that many blocks instead (no streaming): scroll it and only the blocks
//! near the screen have entities, whatever the length.

use bevy::prelude::*;
use bevy::ui_widgets::ScrollArea;
use bevy_md::{Markdown, MarkdownPlugin};

const SOURCE: &str = include_str!("stream.md");
/// Bytes appended per frame.
const CHUNK: usize = 6;

#[derive(Resource, Default)]
struct Cursor(usize);

/// Blocks of the synthetic document given with `--blocks`, if any.
#[derive(Resource, Default)]
struct Blocks(Option<usize>);

fn blocks_from_args() -> Option<usize> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--blocks" {
            return args.next().and_then(|n| n.parse().ok());
        }
        if let Some(n) = arg.strip_prefix("--blocks=") {
            return n.parse().ok();
        }
    }
    None
}

fn main() {
    let blocks = blocks_from_args();
    App::new()
        .add_plugins((DefaultPlugins, MarkdownPlugin))
        // A synthetic document is there from the first frame: nothing to stream.
        .insert_resource(Cursor(if blocks.is_some() { SOURCE.len() } else { 0 }))
        .insert_resource(Blocks(blocks))
        .add_systems(Startup, setup)
        .add_systems(Update, (stream, restart))
        .run();
}

fn setup(mut commands: Commands, blocks: Res<Blocks>) {
    commands.spawn(Camera2d);
    let source = blocks.0.map(bevy_md::core::synthetic::source);
    commands.spawn((
        Node {
            width: percent(100),
            height: percent(100),
            justify_content: JustifyContent::Center,
            overflow: Overflow::scroll_y(),
            ..default()
        },
        ScrollArea,
        BackgroundColor(Color::srgb(0.09, 0.09, 0.11)),
        children![(
            Markdown::new(source.as_deref().unwrap_or("")),
            Node {
                flex_direction: FlexDirection::Column,
                // As tall as the text, so the scroll area scrolls to its end.
                align_self: AlignSelf::FlexStart,
                flex_shrink: 0.0,
                width: px(720),
                padding: UiRect::all(px(32)),
                ..default()
            },
        )],
    ));
}

fn stream(mut cursor: ResMut<Cursor>, mut markdown: Single<&mut Markdown>) {
    if cursor.0 >= SOURCE.len() {
        return;
    }
    let mut end = (cursor.0 + CHUNK).min(SOURCE.len());
    while !SOURCE.is_char_boundary(end) {
        end += 1;
    }
    markdown.push_str(&SOURCE[cursor.0..end]);
    cursor.0 = end;
}

fn restart(
    keys: Res<ButtonInput<KeyCode>>,
    mut cursor: ResMut<Cursor>,
    mut markdown: Single<&mut Markdown>,
) {
    if keys.just_pressed(KeyCode::KeyR) {
        cursor.0 = 0;
        markdown.set_source("");
    }
}
