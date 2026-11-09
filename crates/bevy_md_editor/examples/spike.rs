//! Spike of decision D4: edit rich text in place with our own caret.
//!
//! Click inside the paragraph to place the caret; Left, Right, Home and End
//! move it; typing, Backspace and Delete edit the markdown **source**, shown
//! at the bottom together with how long the edit took.
//!
//! `SPIKE_AUTO=<directory>` plays a scripted session instead, saves
//! screenshots into the directory and exits with the result of its checks.

use std::time::Duration;

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::text::{ComputedTextBlock, FontSize, FontSource, GenericFontFamily, TextLayoutInfo};
use bevy::ui::{ComputedNode, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use bevy_md::{BlockText, Markdown, MarkdownGutter, MarkdownPlugin};
use bevy_md_editor::{EditMetrics, MarkdownEditor, MarkdownEditorPlugin, PlaceCaret};

const SOURCE: &str = "# Editing rich text\n\n\
Some **bold** and *italic* text with `code`, a [link](https://bevy.org) and an \\* escape.\n\n\
A second paragraph, never edited: only the block being edited is laid out again.\n";

/// Frames between two steps of the script, enough for layout and a screenshot.
const STEP_FRAMES: u32 = 12;

fn main() {
    let mut app = App::new();
    app.add_plugins((DefaultPlugins, MarkdownPlugin, MarkdownEditorPlugin))
        .add_systems(Startup, setup)
        .add_systems(Update, hud);
    if let Ok(directory) = std::env::var("SPIKE_AUTO") {
        app.insert_resource(Script {
            directory,
            frame: 0,
            step: 0,
            failed: false,
            burst: 0,
            seen_edits: 0,
            samples: Vec::new(),
        })
        .add_systems(Update, script);
    }
    app.run();
}

#[derive(Component)]
struct Hud;

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands
        .spawn((
            Node {
                width: percent(100),
                height: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgb(0.09, 0.09, 0.11)),
        ))
        .with_children(|root| {
            let mut editor = MarkdownEditor::new();
            let markdown = Markdown::new(SOURCE);
            editor.set_caret(&markdown, SOURCE.find("Some").unwrap() + 5);
            root.spawn((
                markdown,
                editor,
                // Hovering a block shows its "+" and drag handle in the
                // left padding: one entity, wherever the pointer goes.
                MarkdownGutter,
                Node {
                    flex_direction: FlexDirection::Column,
                    width: px(704),
                    padding: UiRect {
                        left: px(64),
                        ..UiRect::all(px(32))
                    },
                    ..default()
                },
            ));
            root.spawn((
                Hud,
                Text::default(),
                TextFont {
                    font: FontSource::Generic(GenericFontFamily::Monospace),
                    font_size: FontSize::Px(13.0),
                    ..default()
                },
                TextColor(Color::srgb(0.6, 0.6, 0.66)),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(16),
                    bottom: px(12),
                    ..default()
                },
            ));
        });
}

/// The markdown source with the caret drawn in it, and the timing report.
fn hud(
    metrics: Res<EditMetrics>,
    editor: Single<(&MarkdownEditor, &Markdown)>,
    mut text: Single<&mut Text, With<Hud>>,
) {
    let (editor, markdown) = *editor;
    let source = markdown.document().source();
    let (before, after) = source.split_at(editor.caret());
    let ms = |d: Option<std::time::Duration>| {
        d.map_or_else(
            || "-".to_owned(),
            |d| format!("{:.3} ms", d.as_secs_f64() * 1e3),
        )
    };
    text.0 = format!(
        "source, caret at byte {caret}:\n{before}|{after}\n\n\
         key -> edited block laid out: {key} (Document::edit {doc}, respawn + layout {sync}, worst {worst:.3} ms)\n\
         blocks laid out in that frame: {laid} of {blocks} | edits: {edits}",
        caret = editor.caret(),
        key = ms(metrics.key_to_layout),
        doc = ms(metrics.document_edit),
        sync = ms(metrics.sync_to_layout),
        worst = metrics.worst.as_secs_f64() * 1e3,
        laid = metrics.blocks_laid_out,
        blocks = metrics.blocks,
        edits = metrics.edits,
    );
}

/// Keystrokes of the typing burst that measures latency, one per frame.
const BURST: u32 = 150;

/// Timing of one edit: key to laid out, `Document::edit`, respawn and layout.
type Sample = (Duration, Duration, Duration);

#[derive(Resource)]
struct Script {
    directory: String,
    frame: u32,
    step: u32,
    failed: bool,
    /// Keystrokes of the burst still to send.
    burst: u32,
    seen_edits: u64,
    samples: Vec<Sample>,
}

/// The `percent`-th percentile of one column of the samples, in milliseconds.
fn percentile(samples: &[Sample], column: impl Fn(&Sample) -> Duration, percent: usize) -> f64 {
    let mut values: Vec<f64> = samples
        .iter()
        .map(|s| column(s).as_secs_f64() * 1e3)
        .collect();
    values.sort_by(f64::total_cmp);
    values
        .get((values.len().saturating_sub(1) * percent).div_ceil(100))
        .copied()
        .unwrap_or(0.0)
}

fn key(window: Entity, key_code: KeyCode, logical_key: Key, text: Option<&str>) -> KeyboardInput {
    KeyboardInput {
        key_code,
        logical_key,
        state: ButtonState::Pressed,
        text: text.map(Into::into),
        repeat: false,
        window,
    }
}

/// A scripted session: the same messages a user's clicks and keys produce.
#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "a Bevy system that reads as one linear script"
)]
fn script(
    mut script: ResMut<Script>,
    mut commands: Commands,
    mut keys: MessageWriter<KeyboardInput>,
    mut clicks: MessageWriter<PlaceCaret>,
    mut exit: MessageWriter<AppExit>,
    window: Single<Entity, With<PrimaryWindow>>,
    editor: Single<(&MarkdownEditor, &Markdown)>,
    metrics: Res<EditMetrics>,
    children: Query<&Children>,
    texts: Query<
        (
            &TextLayoutInfo,
            &ComputedNode,
            &UiGlobalTransform,
            &ComputedTextBlock,
        ),
        With<BlockText>,
    >,
) {
    let window = *window;
    if metrics.edits != script.seen_edits {
        script.seen_edits = metrics.edits;
        if let (Some(total), Some(doc), Some(sync)) = (
            metrics.key_to_layout,
            metrics.document_edit,
            metrics.sync_to_layout,
        ) {
            script.samples.push((total, doc, sync));
        }
    }
    if script.burst > 0 {
        // The typing burst: a letter per frame, then some Backspaces.
        let sent = BURST - script.burst;
        script.burst -= 1;
        keys.write(if sent < BURST * 2 / 3 {
            let ch = char::from(b'a' + (sent % 26) as u8).to_string();
            key(
                window,
                KeyCode::KeyA,
                Key::Character(ch.as_str().into()),
                Some(&ch),
            )
        } else {
            key(window, KeyCode::Backspace, Key::Backspace, Some("\u{8}"))
        });
        return;
    }
    script.frame += 1;
    if !script.frame.is_multiple_of(STEP_FRAMES) {
        return;
    }
    let (editor, markdown) = *editor;
    let source = markdown.document().source().to_owned();
    let check = |script: &mut Script, ok: bool, what: &str| {
        println!("[{}] {what}", if ok { "ok" } else { "FAIL" });
        script.failed |= !ok;
    };
    let shot = |commands: &mut Commands, script: &Script, name: &str| {
        let path = format!("{}/{name}.png", script.directory);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    };

    // Where the layout of the paragraph put its n-th run, in window pixels.
    let run_point = |run: u32, at_right_edge: bool| -> Option<Vec2> {
        let root = markdown.block_entity(1)?;
        let text = children
            .iter_descendants(root)
            .find(|c| texts.contains(*c))?;
        let (info, node, transform, _) = texts.get(text).ok()?;
        let bounds = info
            .run_geometry
            .iter()
            .find(|g| g.section_index == run)?
            .bounds;
        let x = if at_right_edge {
            bounds.max.x - 1.0
        } else {
            bounds.center().x
        };
        let local = node.content_box().min + Vec2::new(x, bounds.center().y);
        Some(transform.transform_point2(local))
    };

    let step = script.step;
    script.step += 1;
    match step {
        0 => shot(&mut commands, &script, "01-initial"),
        1 => {
            // Section 0 is the empty root text; "Some " is 1 and "bold" is 2.
            if let Some(position) = run_point(2, false) {
                clicks.write(PlaceCaret { position });
            } else {
                check(&mut script, false, "found the bold run to click on");
            }
        }
        2 => {
            let bold = source.find("bold").unwrap();
            let caret = editor.caret();
            check(
                &mut script,
                (bold + 1..=bold + 3).contains(&caret),
                &format!(
                    "click in the middle of \"bold\" put the caret inside it (byte {caret}, run starts at {bold})"
                ),
            );
            shot(&mut commands, &script, "02-clicked-in-bold");
        }
        3 => {
            if let Some(position) = run_point(2, true) {
                clicks.write(PlaceCaret { position });
            }
        }
        4 => {
            let end = source.find("bold").unwrap() + 4;
            check(
                &mut script,
                editor.caret() == end,
                &format!(
                    "click on the right edge of \"bold\" put the caret after its last letter (byte {} == {end})",
                    editor.caret()
                ),
            );
            for ch in ["X", "Y"] {
                keys.write(key(
                    window,
                    KeyCode::KeyX,
                    Key::Character(ch.into()),
                    Some(ch),
                ));
            }
        }
        5 => {
            check(
                &mut script,
                source.contains("**boldXY**"),
                "typing at the end of a bold run continues the bold run (`**boldXY**`)",
            );
            keys.write(key(
                window,
                KeyCode::Backspace,
                Key::Backspace,
                Some("\u{8}"),
            ));
        }
        6 => {
            check(
                &mut script,
                source.contains("**boldX**"),
                "Backspace deleted one letter (`**boldX**`)",
            );
            shot(&mut commands, &script, "03-typed-in-bold");
            keys.write(key(window, KeyCode::Home, Key::Home, None));
            keys.write(key(
                window,
                KeyCode::KeyZ,
                Key::Character("Z".into()),
                Some("Z"),
            ));
        }
        7 => {
            check(
                &mut script,
                source.contains("\n\nZSome **boldX**"),
                "Home then typing inserted at the start of the line",
            );
            keys.write(key(window, KeyCode::ArrowLeft, Key::ArrowLeft, None));
            keys.write(key(window, KeyCode::Delete, Key::Delete, Some("\u{7f}")));
        }
        8 => {
            check(
                &mut script,
                source.contains("\n\nSome **boldX**"),
                "Left then Delete removed the letter after the caret",
            );
            check(
                &mut script,
                metrics.blocks_laid_out == 1 && metrics.blocks == 3,
                &format!(
                    "only the edited block was laid out again ({} of {})",
                    metrics.blocks_laid_out, metrics.blocks
                ),
            );
            keys.write(key(window, KeyCode::ArrowRight, Key::ArrowRight, None));
            keys.write(key(window, KeyCode::ArrowRight, Key::ArrowRight, None));
            keys.write(key(window, KeyCode::ArrowRight, Key::ArrowRight, None));
            keys.write(key(window, KeyCode::ArrowRight, Key::ArrowRight, None));
            keys.write(key(window, KeyCode::ArrowRight, Key::ArrowRight, None));
            // Over "**" to the start of " and": the caret stays in the text.
            keys.write(key(
                window,
                KeyCode::KeyQ,
                Key::Character("Q".into()),
                Some("Q"),
            ));
        }
        9 => {
            println!("final source: {source:?}");
            println!(
                "last edit: key -> block laid out {:?}, Document::edit {:?}, respawn + layout {:?}",
                metrics.key_to_layout, metrics.document_edit, metrics.sync_to_layout
            );
            shot(&mut commands, &script, "04-final");
        }
        10 => {
            // Latency under a typing burst: a key per frame, then Backspaces.
            script.burst = BURST;
            script.samples.clear();
        }
        11 => {
            let n = script.samples.len();
            check(
                &mut script,
                n >= 100,
                &format!("{n} edits of the typing burst were timed"),
            );
            for (name, column) in [
                (
                    "key -> block laid out",
                    (|s: &Sample| s.0) as fn(&Sample) -> Duration,
                ),
                ("Document::edit", |s| s.1),
                ("respawn + layout", |s| s.2),
            ] {
                println!(
                    "{name:>22}: p50 {:.3} ms  p95 {:.3} ms  max {:.3} ms",
                    percentile(&script.samples, column, 50),
                    percentile(&script.samples, column, 95),
                    percentile(&script.samples, column, 100),
                );
            }
            shot(&mut commands, &script, "05-after-burst");
        }
        12 => {}
        _ => {
            println!(
                "{}",
                if script.failed {
                    "SPIKE FAILED"
                } else {
                    "SPIKE OK"
                }
            );
            exit.write(if script.failed {
                AppExit::error()
            } else {
                AppExit::Success
            });
        }
    }
}
