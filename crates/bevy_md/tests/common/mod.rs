//! Support for the budget tests: a headless Bevy app with real UI layout and
//! text layout (no window, no renderer), a counting allocator and the
//! readers of `perf-budgets.json`.
//!
//! Layout needs a camera with a render target, which a window normally
//! provides; here the camera's computed target info is filled in by hand, the
//! way Bevy's own UI layout tests do it.

#![allow(dead_code, missing_docs)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::app::TaskPoolPlugin;
use bevy::asset::AssetPlugin;
use bevy::camera::{Camera, ComputedCameraValues, RenderTargetInfo, Viewport};
use bevy::ecs::schedule::{Schedules, SingleThreadedExecutor};
use bevy::image::ImagePlugin;
use bevy::input::InputPlugin;
use bevy::prelude::*;
use bevy::text::{ComputedTextBlock, FontSource};
use bevy::time::TimePlugin;
use bevy::transform::TransformPlugin;
use bevy::ui::UiPlugin;
use bevy_md::{Markdown, MarkdownPlugin, MarkdownTheme};

static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Hold this for the whole test: the budget tests compare frame times, which
/// other tests running at the same moment would disturb.
pub fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Window size of the test app, in logical pixels.
pub const VIEWPORT: Vec2 = Vec2::new(1000.0, 720.0);

// ---------------------------------------------------------------------------
// Counting allocator
// ---------------------------------------------------------------------------

/// What the calling thread allocated while a closure ran. The counts come
/// from the `allocation-counter` crate, a `GlobalAlloc` that counts per
/// thread, so the workspace's `unsafe_code = "forbid"` stays untouched.
#[derive(Clone, Copy, Debug, Default)]
pub struct AllocStats {
    pub allocations: u64,
    pub bytes: u64,
    /// Bytes allocated minus bytes freed: what the closure left behind.
    pub retained: i64,
}

pub fn count_allocations<R>(f: impl FnOnce() -> R) -> (R, AllocStats) {
    let mut result = None;
    let info = allocation_counter::measure(|| result = Some(f()));
    let stats = AllocStats {
        allocations: info.count_total,
        bytes: info.bytes_total,
        retained: info.bytes_current,
    };
    (result.expect("the closure ran"), stats)
}

// ---------------------------------------------------------------------------
// The headless app
// ---------------------------------------------------------------------------

/// Counts gathered every frame, after Bevy's text layout.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub struct FrameStats {
    /// Entities that got a `Node` this frame.
    pub spawned: usize,
    /// Entities that lost their `Node` this frame.
    pub despawned: usize,
    /// Text blocks whose layout was computed again this frame.
    pub laid_out: usize,
    /// Entities with a `Node`, in total.
    pub nodes: usize,
}

/// The text entities counted in [`FrameStats::laid_out`].
#[derive(Resource, Default, Debug, Clone)]
pub struct LaidOut(pub Vec<Entity>);

fn gather_stats(
    mut stats: ResMut<FrameStats>,
    added: Query<(), Added<Node>>,
    mut removed: RemovedComponents<Node>,
    texts: Query<(Entity, Ref<ComputedTextBlock>)>,
    nodes: Query<(), With<Node>>,
    mut laid: ResMut<LaidOut>,
) {
    stats.spawned = added.iter().count();
    stats.despawned = removed.read().count();
    laid.0.clear();
    laid.0
        .extend(texts.iter().filter(|(_, c)| c.is_changed()).map(|(e, _)| e));
    stats.laid_out = laid.0.len();
    stats.nodes = nodes.iter().count();
}

/// Entities whose `Text`, `TextSpan`, `TextFont`, `Node` or
/// `BlockSurroundings` were written (or added) in the last frame.
#[derive(Resource, Default, Debug, Clone)]
pub struct Writes {
    /// Any of the five.
    pub all: Vec<Entity>,
    /// `Text`, `TextSpan` or `TextFont` only.
    pub texts: Vec<Entity>,
    /// `BlockSurroundings` only.
    pub surroundings: Vec<Entity>,
}

#[allow(clippy::type_complexity)]
fn gather_writes(
    mut writes: ResMut<Writes>,
    texts: Query<Entity, Or<(Changed<Text>, Changed<TextSpan>, Changed<TextFont>)>>,
    surroundings: Query<Entity, Changed<bevy_md::BlockSurroundings>>,
    changed: Query<
        Entity,
        Or<(
            Changed<Text>,
            Changed<TextSpan>,
            Changed<TextFont>,
            Changed<Node>,
            Changed<bevy_md::BlockSurroundings>,
        )>,
    >,
) {
    writes.all.clear();
    writes.all.extend(changed.iter());
    writes.texts.clear();
    writes.texts.extend(texts.iter());
    writes.surroundings.clear();
    writes.surroundings.extend(surroundings.iter());
}

/// A theme whose fonts are the one embedded in Bevy, so that text layout is
/// the same on every machine.
pub fn test_theme() -> MarkdownTheme {
    MarkdownTheme {
        body_font: FontSource::Handle(Handle::default()),
        mono_font: FontSource::Handle(Handle::default()),
        ..default()
    }
}

/// A headless app with the UI and text plugins, a camera that is
/// [`VIEWPORT`] big, and every schedule single threaded, so that the
/// counting allocator sees all the work.
pub fn headless_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        TaskPoolPlugin::default(),
        TimePlugin,
        AssetPlugin::default(),
        TransformPlugin,
        InputPlugin,
        ImagePlugin::default(),
        bevy::image::TextureAtlasPlugin,
        bevy::a11y::AccessibilityPlugin,
        bevy::picking::PickingPlugin,
        bevy::picking::InteractionPlugin,
        bevy::text::TextPlugin,
        UiPlugin,
    ));
    app.insert_resource(test_theme());
    let physical = VIEWPORT.as_uvec2();
    app.world_mut().spawn((
        Camera2d,
        Camera {
            computed: ComputedCameraValues {
                target_info: Some(RenderTargetInfo {
                    physical_size: physical,
                    scale_factor: 1.0,
                }),
                ..default()
            },
            viewport: Some(Viewport {
                physical_size: physical,
                ..default()
            }),
            ..default()
        },
    ));
    app.init_resource::<FrameStats>();
    app.init_resource::<Writes>();
    app.init_resource::<LaidOut>();
    app.add_systems(
        PostUpdate,
        (gather_stats, gather_writes).after(bevy::ui::widget::text_system),
    );
    app.add_plugins(MarkdownPlugin);
    single_threaded(&mut app);
    app
}

/// Every schedule on one thread: the counting allocator counts per thread.
pub fn single_threaded(app: &mut App) {
    let mut schedules = app.world_mut().resource_mut::<Schedules>();
    for (_, schedule) in schedules.iter_mut() {
        schedule.set_executor(SingleThreadedExecutor::new());
    }
}

/// Spawn a scroll area [`VIEWPORT`] big holding one [`Markdown`] column, and
/// return `(scroll area, markdown)`.
pub fn spawn_document(app: &mut App, source: &str) -> (Entity, Entity) {
    let world = app.world_mut();
    let markdown = world
        .spawn((
            Markdown::new(source),
            Node {
                flex_direction: FlexDirection::Column,
                align_self: AlignSelf::FlexStart,
                flex_shrink: 0.0,
                width: Val::Px(VIEWPORT.x - 40.0),
                padding: UiRect::all(Val::Px(20.0)),
                ..default()
            },
        ))
        .id();
    let area = world
        .spawn(Node {
            width: Val::Px(VIEWPORT.x),
            height: Val::Px(VIEWPORT.y),
            overflow: Overflow::scroll_y(),
            ..default()
        })
        .add_child(markdown)
        .id();
    (area, markdown)
}

/// Run `n` frames.
pub fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
}

/// Run frames until nothing is spawned, despawned or laid out any more, and no
/// [`Markdown`] is still parsing a large change in the background.
pub fn settle(app: &mut App) {
    let mut quiet = 0;
    let mut steady = 0;
    let waiting = Instant::now();
    loop {
        app.update();
        let parsing = {
            let world = app.world_mut();
            world
                .query::<&Markdown>()
                .iter(world)
                .any(Markdown::is_parsing)
        };
        if parsing {
            assert!(
                waiting.elapsed() < Duration::from_secs(120),
                "a background parse never finished"
            );
            std::thread::sleep(Duration::from_millis(1));
            quiet = 0;
            continue;
        }
        let stats = *app.world().resource::<FrameStats>();
        if stats.spawned + stats.despawned + stats.laid_out == 0 {
            quiet += 1;
            if quiet == 3 {
                return;
            }
        } else {
            quiet = 0;
        }
        steady += 1;
        assert!(steady < 200, "the app never settled");
    }
}

/// One frame, with its wall time.
pub fn timed_frame(app: &mut App) -> Duration {
    let start = Instant::now();
    app.update();
    start.elapsed()
}

/// CPU time the calling thread has used so far (user plus system), for
/// telling work apart from waiting: a frame whose wall time is far above its
/// CPU time spent the difference descheduled, blocked or page-faulting on
/// someone else's behalf, not running our code.
pub fn thread_cpu() -> Duration {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
    Duration::new(
        u64::try_from(time.tv_sec).unwrap_or(0),
        u32::try_from(time.tv_nsec).unwrap_or(0),
    )
}

/// One frame: its wall time and the CPU time of this thread in it.
pub fn timed_frame_cpu(app: &mut App) -> (Duration, Duration) {
    let cpu = thread_cpu();
    let start = Instant::now();
    app.update();
    (start.elapsed(), thread_cpu().saturating_sub(cpu))
}

pub fn set_scroll(app: &mut App, area: Entity, y: f32) {
    app.world_mut()
        .entity_mut(area)
        .get_mut::<bevy::ui::ScrollPosition>()
        .expect("scroll position")
        .0
        .y = y;
}

// ---------------------------------------------------------------------------
// perf-budgets.json
// ---------------------------------------------------------------------------

fn budgets_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../perf-budgets.json")
}

fn load_budgets() -> serde_json::Value {
    let text = std::fs::read_to_string(budgets_path()).expect("perf-budgets.json");
    serde_json::from_str(&text).expect("perf-budgets.json is valid JSON")
}

/// A number a budget can be checked against: counts and ratios, which are all
/// far below 2^52, so the conversion to `f64` is exact.
pub trait Measure {
    fn measure(self) -> f64;
}

macro_rules! measure {
    ($($t:ty),*) => {$(
        impl Measure for $t {
            #[allow(clippy::cast_precision_loss, clippy::cast_lossless)]
            fn measure(self) -> f64 {
                self as f64
            }
        }
    )*};
}
measure!(usize, u64, i64, f32, f64);

/// The `limit` of a budget; panics when the budget is missing, so a test can
/// never pass with no limit.
pub fn limit(name: &str) -> f64 {
    let budgets = load_budgets();
    let entry = &budgets["budgets"][name];
    assert!(
        entry["measured"].is_number() && entry["why"].as_str().is_some_and(|w| !w.is_empty()),
        "budget {name} needs `measured` and `why` in perf-budgets.json"
    );
    entry["limit"]
        .as_f64()
        .unwrap_or_else(|| panic!("budget {name} has no `limit` in perf-budgets.json"))
}

/// Check `value` against the budget `name`. With `BEVY_MD_PRINT_BUDGETS` set
/// the value is only printed, to be copied into the file.
#[track_caller]
pub fn check(name: &str, value: impl Measure) {
    let value = value.measure();
    if std::env::var_os("BEVY_MD_PRINT_BUDGETS").is_some() {
        println!("BUDGET {name} = {value}");
        // Calibrating: print every value, assert nothing.
        return;
    }
    let limit = limit(name);
    assert!(
        value <= limit,
        "budget `{name}` exceeded: measured {value}, limit {limit} (perf-budgets.json)"
    );
}

/// Print a measured value without checking it (for numbers that are only
/// recorded).
pub fn report(name: &str, value: impl Measure) {
    let value = value.measure();
    if std::env::var_os("BEVY_MD_PRINT_BUDGETS").is_some() {
        println!("BUDGET {name} = {value}");
    }
}

pub fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    samples[samples.len() / 2]
}
