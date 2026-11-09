//! The native gate: a real window and renderer, a scripted session of
//! opening, idling, scrolling and typing in a document of `GATE_BLOCKS`
//! blocks, and a JSON report of what every frame cost.
//!
//! This is the measurement `PLAN.md` asks for ("scroll a 120 fps medido no
//! gate calibrado"); the headless budgets in `perf-budgets.json` count work,
//! this one times frames. Run it through `scripts/native-gate.sh`, which
//! refuses to run on a locked screen, runs 200, 2000 and 100000 blocks one
//! process each and puts the result next to the load and the machine.
//!
//! What is measured, per frame of every phase (raw samples are in the
//! report, with the p50, p95, p99 and max):
//!
//! - `interval_ms`: the time between the starts of two consecutive frames of
//!   the main world. With vsync this is what the user sees as the frame rate
//!   (the main loop waits for the renderer), without it the loop's own pace.
//! - `update_ms`: wall time from `First` to `Last` of the main world: the
//!   systems of the frame (`bevy_md`'s sync, layout, the script), the part
//!   that is CPU work of the document and of Bevy UI. It does not include
//!   waiting for the renderer or the display.
//! - `cpu_ms`: CPU time of the whole process during the frame (all threads),
//!   which tells a frame that did work from one that was waiting.
//! - `render_interval_ms`: the time between two consecutive ends of the
//!   render world's frame (`RenderSystems::Cleanup`, after the commands were
//!   submitted and the frame presented). Bevy gives no timestamp of the
//!   moment a frame reaches the display, so this is the closest it offers:
//!   it paces the presented frames within a frame of pipelining.
//!
//! Settings (environment variables): `GATE_BLOCKS` (default 2000),
//! `GATE_OUT` (path of the JSON report, else stdout), `GATE_PRESENT`
//! (`vsync`, the default, or `novsync`), `GATE_WARP_CURSOR=1` (see below), `GATE_SHOTS` (a directory for
//! screenshots of the top, the scrolled document, the middle and the caret
//! with the gutter), `GATE_STEP` (pixels scrolled per frame, default 20).

#![allow(
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "a measurement tool: counts of frames and nanoseconds are far from the limits"
)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyCode, KeyboardInput};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::ui::{ComputedNode, ScrollPosition};
use bevy::ui_widgets::ScrollArea;
use bevy::window::{Monitor, PresentMode, PrimaryWindow, WindowOccluded};
use bevy_md::{Markdown, MarkdownGutter, MarkdownPlugin};
use bevy_md_editor::{EditMetrics, MarkdownEditor, MarkdownEditorPlugin};
#[cfg(unix)]
use rustix::time::{ClockId, clock_gettime};
use serde_json::{Value, json};

/// One thing the script does for a number of frames.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Action {
    /// Wait for the document to be parsed and drawn (at least `frames`).
    Open,
    Nothing,
    Down,
    Up,
    /// Jump to the middle of the document, then do nothing for a frame.
    Middle,
    Top,
    /// A character typed every few frames, with the gutter shown.
    Type,
    /// Wait for screenshots to be written.
    Finish,
}

struct Step {
    name: &'static str,
    action: Action,
    frames: u32,
    recorded: bool,
}

const fn step(name: &'static str, action: Action, frames: u32, recorded: bool) -> Step {
    Step {
        name,
        action,
        frames,
        recorded,
    }
}

/// The session. Lengths are in frames, not seconds, so that the same
/// distance is scrolled whatever the frame rate.
const STEPS: &[Step] = &[
    step("open", Action::Open, 30, true),
    step("settle", Action::Nothing, 120, false),
    step("idle", Action::Nothing, 240, true),
    // Warm-up: the text caches settle by the third pass over the same text.
    step("warm_down_1", Action::Down, 240, false),
    step("warm_up_1", Action::Up, 240, false),
    step("warm_down_2", Action::Down, 240, false),
    step("warm_up_2", Action::Up, 240, false),
    step("scroll_down", Action::Down, 480, true),
    step("scroll_up", Action::Up, 480, true),
    step("jump_middle", Action::Middle, 1, true),
    step("middle_idle", Action::Nothing, 60, true),
    step("scroll_middle", Action::Down, 240, true),
    step("back_to_top", Action::Top, 60, false),
    step("typing", Action::Type, 160, true),
    step("finish", Action::Finish, 45, false),
];

/// Frames of the `Open` step at most, whatever the document.
const OPEN_LIMIT: u32 = 1200;
/// A character is typed every this many frames.
const TYPE_EVERY: u32 = 4;

#[derive(Resource)]
struct Config {
    blocks: usize,
    out: Option<String>,
    present: &'static str,
    shots: Option<String>,
    step_px: f32,
    warp_cursor: bool,
}

/// Samples of one frame.
struct Sample {
    interval_ms: f64,
    update_ms: f64,
    cpu_ms: f64,
    /// Index into the render timestamps when the frame ended.
    renders_seen: usize,
}

#[derive(Resource)]
struct Gate {
    /// The step the current frame belongs to, and the frame within it.
    step: usize,
    frame: u32,
    begun: Option<Instant>,
    previous_begin: Option<Instant>,
    cpu_at_begin: Duration,
    interval: f64,
    /// Samples by step.
    samples: Vec<Vec<Sample>>,
    /// Render stamps seen when each step started.
    renders_at_start: Vec<usize>,
    started: Instant,
    opened_frames: u32,
    caret_set: bool,
    scrolled_px: f64,
    typed: u32,
    frames_total: u64,
    shots_taken: u8,
    finished_report: bool,
}

#[derive(Resource, Clone, Default)]
struct RenderStamps(Arc<Mutex<Vec<Instant>>>);

#[derive(Resource, Default)]
struct Occlusions(u32);

#[derive(Component)]
struct Area;

fn main() {
    let env = |name: &str| std::env::var(name).ok();
    let present = match env("GATE_PRESENT").as_deref() {
        Some("novsync") => "novsync",
        _ => "vsync",
    };
    let config = Config {
        blocks: env("GATE_BLOCKS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(2000),
        out: env("GATE_OUT"),
        present,
        shots: env("GATE_SHOTS"),
        step_px: env("GATE_STEP")
            .and_then(|v| v.parse().ok())
            .unwrap_or(20.0),
        warp_cursor: env("GATE_WARP_CURSOR").is_some_and(|v| v == "1"),
    };
    let stamps = RenderStamps::default();
    let mut app = App::new();
    app.add_plugins((
        DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "bevy_md native gate".into(),
                resolution: (1000, 720).into(),
                present_mode: if present == "novsync" {
                    PresentMode::AutoNoVsync
                } else {
                    PresentMode::AutoVsync
                },
                ..default()
            }),
            ..default()
        }),
        MarkdownPlugin,
        MarkdownEditorPlugin,
    ))
    .insert_resource(Gate {
        step: 0,
        frame: 0,
        begun: None,
        previous_begin: None,
        cpu_at_begin: Duration::ZERO,
        interval: 0.0,
        samples: STEPS.iter().map(|_| Vec::new()).collect(),
        renders_at_start: vec![0; STEPS.len()],
        started: Instant::now(),
        opened_frames: 0,
        caret_set: false,
        scrolled_px: 0.0,
        typed: 0,
        frames_total: 0,
        shots_taken: 0,
        finished_report: false,
    })
    .insert_resource(config)
    .insert_resource(stamps.clone())
    .init_resource::<Occlusions>()
    .add_systems(Startup, setup)
    .add_systems(First, begin_frame)
    .add_systems(Update, script)
    .add_systems(Last, end_frame);
    app.sub_app_mut(RenderApp)
        .insert_resource(stamps)
        .add_systems(Render, stamp_render.in_set(RenderSystems::Cleanup));
    app.run();
}

#[cfg(not(unix))]
fn process_cpu() -> Duration {
    Duration::ZERO
}

#[cfg(unix)]
fn process_cpu() -> Duration {
    let t = clock_gettime(ClockId::ProcessCPUTime);
    Duration::new(t.tv_sec.cast_unsigned(), t.tv_nsec as u32)
}

fn stamp_render(stamps: Res<RenderStamps>) {
    stamps
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(Instant::now());
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands.spawn((
        Area,
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
            Markdown::new(""),
            MarkdownEditor::new(),
            MarkdownGutter,
            Node {
                flex_direction: FlexDirection::Column,
                align_self: AlignSelf::FlexStart,
                flex_shrink: 0.0,
                width: px(768),
                padding: UiRect {
                    left: px(64),
                    ..UiRect::all(px(32))
                },
                ..default()
            },
        )],
    ));
}

fn begin_frame(mut gate: ResMut<Gate>) {
    let now = Instant::now();
    gate.interval = gate
        .previous_begin
        .map_or(0.0, |previous| (now - previous).as_secs_f64() * 1e3);
    gate.previous_begin = Some(now);
    gate.begun = Some(now);
    gate.cpu_at_begin = process_cpu();
}

fn end_frame(mut gate: ResMut<Gate>, stamps: Res<RenderStamps>) {
    let Some(begun) = gate.begun else { return };
    let update_ms = begun.elapsed().as_secs_f64() * 1e3;
    let cpu_ms = process_cpu()
        .saturating_sub(gate.cpu_at_begin)
        .as_secs_f64()
        * 1e3;
    let renders_seen = stamps
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .len();
    gate.frames_total += 1;
    let step = gate.step;
    if step >= STEPS.len() {
        return;
    }
    // The very first frame has no previous one to measure an interval from.
    let interval_ms = gate.interval;
    if STEPS[step].recorded {
        gate.samples[step].push(Sample {
            interval_ms,
            update_ms,
            cpu_ms,
            renders_seen,
        });
    }
}

fn key(window: Entity, key_code: KeyCode, logical_key: Key, text: &str) -> KeyboardInput {
    KeyboardInput {
        key_code,
        logical_key,
        state: ButtonState::Pressed,
        text: Some(text.into()),
        repeat: false,
        window,
    }
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "a Bevy system that reads as one linear script"
)]
fn script(
    mut gate: ResMut<Gate>,
    config: Res<Config>,
    stamps: Res<RenderStamps>,
    mut commands: Commands,
    mut keys: MessageWriter<KeyboardInput>,
    mut exit: MessageWriter<AppExit>,
    mut occluded: MessageReader<WindowOccluded>,
    mut occlusions: ResMut<Occlusions>,
    mut window: Single<(Entity, &mut Window), With<PrimaryWindow>>,
    mut area: Single<(&mut ScrollPosition, &ComputedNode), With<Area>>,
    mut markdown: Single<(&mut Markdown, &mut MarkdownEditor)>,
    metrics: Res<EditMetrics>,
    monitors: Query<&Monitor>,
) {
    occlusions.0 += occluded.read().filter(|e| e.occluded).count() as u32;
    if gate.step >= STEPS.len() {
        // The report is written and the exit requested; the frame that
        // finishes the app is not part of the session.
        return;
    }
    let (window_entity, window) = &mut *window;
    let window_entity = *window_entity;
    let (markdown, editor) = &mut *markdown;
    let (scroll, node) = &mut *area;

    // The document is handed over in the first frame, like an app opening a
    // file: above the synchronous limit it is parsed in the background.
    if gate.frames_total == 0 {
        markdown.set_source_owned(bevy_md::core::synthetic::source(config.blocks));
    }

    let step = gate.step;
    let action = STEPS[step].action;
    let viewport = f64::from(node.size().y * node.inverse_scale_factor());
    let max_scroll = (markdown.content_height() - viewport).max(0.0) as f32;

    match action {
        Action::Open => {
            gate.opened_frames += 1;
            if !markdown.is_parsing()
                && markdown.is_rendered()
                && !markdown.document().is_empty()
                && markdown.live_blocks().len() > 0
                && !gate.caret_set
            {
                // The caret goes into the second block's text.
                let source = markdown.source();
                let at = source.find("\n\n").map_or(0, |at| at + 3);
                editor.set_caret(markdown, at);
                gate.caret_set = true;
            }
        }
        Action::Nothing | Action::Finish => {}
        Action::Down | Action::Up => {
            let wanted = if action == Action::Down { 1.0 } else { -1.0 };
            // Bounce at the ends so that a short document keeps scrolling.
            let mut direction = wanted;
            if (wanted > 0.0 && scroll.y >= max_scroll) || (wanted < 0.0 && scroll.y <= 0.0) {
                direction = -wanted;
            }
            let before = scroll.y;
            scroll.y = (scroll.y + direction * config.step_px).clamp(0.0, max_scroll);
            gate.scrolled_px += f64::from((scroll.y - before).abs());
        }
        Action::Middle => {
            if gate.frame == 0 {
                scroll.y = max_scroll / 2.0;
            }
        }
        Action::Top => {
            if gate.frame == 0 {
                scroll.y = 0.0;
            }
        }
        Action::Type => {
            // `GATE_WARP_CURSOR=1` moves the operating system's pointer onto
            // the first block of the window, which is the only way to show
            // the gutter: the plugin follows the window's cursor. It moves
            // the user's pointer, so the timed runs leave it alone.
            if config.warp_cursor {
                let scale = window.scale_factor();
                window.set_physical_cursor_position(Some(bevy::math::DVec2::new(
                    f64::from(400.0 * scale),
                    f64::from(140.0 * scale),
                )));
            }
            if gate.frame.is_multiple_of(TYPE_EVERY) {
                let ch = char::from(b'a' + (gate.typed % 26) as u8).to_string();
                keys.write(key(
                    window_entity,
                    KeyCode::KeyA,
                    Key::Character(ch.as_str().into()),
                    &ch,
                ));
                gate.typed += 1;
            }
        }
    }

    // Screenshots at fixed points of the script, to look at the result.
    if let Some(dir) = &config.shots {
        let name = match (STEPS[step].name, gate.frame) {
            ("idle", 200) => Some("1-top"),
            ("scroll_down", 300) => Some("2-scrolled"),
            ("middle_idle", 40) => Some("3-middle"),
            ("typing", 120) => Some("4-caret-gutter"),
            _ => None,
        };
        if let Some(name) = name {
            commands
                .spawn(Screenshot::primary_window())
                .observe(save_to_disk(format!("{dir}/{name}-{}.png", config.blocks)));
            gate.shots_taken += 1;
        }
    }

    // Advance the script.
    gate.frame += 1;
    let done = match action {
        Action::Open => {
            let ready = gate.caret_set && gate.frame >= STEPS[step].frames;
            ready || gate.frame >= OPEN_LIMIT
        }
        _ => gate.frame >= STEPS[step].frames,
    };
    if done {
        gate.step += 1;
        gate.frame = 0;
        let seen = stamps
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len();
        if gate.step < STEPS.len() {
            let next = gate.step;
            gate.renders_at_start[next] = seen;
        }
        if gate.step >= STEPS.len() && !gate.finished_report {
            gate.finished_report = true;
            let report = report(
                &gate,
                &config,
                &*window,
                &monitors,
                &occlusions,
                markdown,
                &metrics,
                &stamps,
            );
            let text = serde_json::to_string_pretty(&report).expect("the report is JSON");
            match &config.out {
                Some(path) => std::fs::write(path, text).expect("the report can be written"),
                None => println!("{text}"),
            }
            exit.write(AppExit::Success);
        }
    }
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    // Nearest rank.
    let rank = ((p / 100.0) * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted[rank.min(sorted.len()) - 1]
}

fn stats(values: &[f64]) -> Value {
    let mut sorted: Vec<f64> = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let mean = if sorted.is_empty() {
        f64::NAN
    } else {
        sorted.iter().sum::<f64>() / sorted.len() as f64
    };
    json!({
        "n": sorted.len(),
        "p50": percentile(&sorted, 50.0),
        "p95": percentile(&sorted, 95.0),
        "p99": percentile(&sorted, 99.0),
        "max": sorted.last().copied().unwrap_or(f64::NAN),
        "mean": mean,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "everything the report is made of"
)]
fn report(
    gate: &Gate,
    config: &Config,
    window: &Window,
    monitors: &Query<&Monitor>,
    occlusions: &Occlusions,
    markdown: &Markdown,
    metrics: &EditMetrics,
    stamps: &RenderStamps,
) -> Value {
    let renders = stamps
        .0
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    let mut phases = serde_json::Map::new();
    for (index, step) in STEPS.iter().enumerate() {
        if !step.recorded {
            continue;
        }
        let samples = &gate.samples[index];
        // The first frame of the run has no interval; a jump's first frame
        // is one sample like any other.
        let intervals: Vec<f64> = samples
            .iter()
            .map(|s| s.interval_ms)
            .filter(|&v| v > 0.0)
            .collect();
        let updates: Vec<f64> = samples.iter().map(|s| s.update_ms).collect();
        let cpus: Vec<f64> = samples.iter().map(|s| s.cpu_ms).collect();
        // Render-world frame intervals of the frames this step drew: the
        // stamps the main world had seen when it began the step until it
        // ended it. The renderer runs a frame behind, so the edges blur by
        // one frame.
        let (from, to) = (
            samples
                .first()
                .map_or(0, |s| s.renders_seen.saturating_sub(1)),
            samples.last().map_or(0, |s| s.renders_seen),
        );
        let render_intervals: Vec<f64> = (from.max(1)..to.min(renders.len()))
            .map(|k| (renders[k] - renders[k - 1]).as_secs_f64() * 1e3)
            .collect();
        let round = |values: &[f64]| -> Vec<f64> {
            values
                .iter()
                .map(|v| (v * 1000.0).round() / 1000.0)
                .collect()
        };
        let fps = |values: &[f64]| -> Value {
            let mut sorted = values.to_vec();
            sorted.sort_by(f64::total_cmp);
            let p50 = percentile(&sorted, 50.0);
            let mean = sorted.iter().sum::<f64>() / sorted.len().max(1) as f64;
            json!({"from_p50": 1000.0 / p50, "from_mean": 1000.0 / mean})
        };
        phases.insert(
            step.name.to_owned(),
            json!({
                "frames": samples.len(),
                "interval_ms": stats(&intervals),
                "update_ms": stats(&updates),
                "cpu_ms": stats(&cpus),
                "render_interval_ms": stats(&render_intervals),
                "fps_main": fps(&intervals),
                "fps_render": fps(&render_intervals),
                "raw": {
                    "interval_ms": round(&intervals),
                    "update_ms": round(&updates),
                    "cpu_ms": round(&cpus),
                    "render_interval_ms": round(&render_intervals),
                },
            }),
        );
    }
    let monitors: Vec<Value> = monitors
        .iter()
        .map(|m| {
            json!({
                "name": m.name,
                "physical_size": [m.physical_width, m.physical_height],
                "scale_factor": m.scale_factor,
                "refresh_rate_millihertz": m.refresh_rate_millihertz,
            })
        })
        .collect();
    json!({
        "blocks": config.blocks,
        "present_mode_requested": config.present,
        "scroll_px_per_frame": config.step_px,
        "window": {
            "logical": [window.width(), window.height()],
            "physical": [window.physical_width(), window.physical_height()],
            "scale_factor": window.scale_factor(),
            "focused": window.focused,
            "occluded_events": occlusions.0,
        },
        "monitors": monitors,
        "document": {
            "blocks": markdown.document().len(),
            "source_bytes": markdown.source().len(),
            "live_blocks": markdown.live_blocks().len(),
            "content_height_px": markdown.content_height(),
        },
        "frames_total": gate.frames_total,
        "scrolled_px_total": gate.scrolled_px,
        "characters_typed": gate.typed,
        "screenshots_requested": gate.shots_taken,
        "open_frames": gate.opened_frames,
        "wall_seconds": gate.started.elapsed().as_secs_f64(),
        "render_frames": renders.len(),
        "last_key_to_layout_ms": metrics.key_to_layout.map(|d| d.as_secs_f64() * 1e3),
        "phases": Value::Object(phases),
    })
}
