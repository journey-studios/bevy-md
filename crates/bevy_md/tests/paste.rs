//! Pasting five megabytes must not stall a frame (M6): the parse runs on the
//! async compute pool, and the frame that installs the result does only work
//! proportional to what is on screen.
//!
//! The time is measured, not assumed, and it is wall time of whole frames
//! (layout of the blocks near the screen included) in the headless app, so it
//! depends on the machine and the profile.
//!
//! Two kinds of test live here, and they are kept apart on purpose:
//!
//! - **Acceptance** (`*_never_stalls_a_frame`): the criterion of the plan, no
//!   frame above 16 ms, checked against **every** frame of **every** run.
//!   Nothing is selected: a run is not discarded for being slow, the maximum
//!   of each run is printed, and the test fails when any of them is over the
//!   limit. Under load it can fail, and that failure is a finding, not noise
//!   to be filtered out by taking the best round.
//! - **Diagnosis** (`*_frame_time_distribution`): many more runs, printing
//!   how the per-run maxima are distributed and which frames were the
//!   slowest. It asserts only correctness (the document is what a full parse
//!   gives, the parse ran off the frame), never a time, so it cannot be used to
//!   pass or fail the criterion; it is there to say where the time goes.

mod common;

use std::time::Duration;

use bevy::prelude::*;
use bevy::ui::UiSystems;
use bevy_md::core::{Document, synthetic};
use bevy_md::{Markdown, MarkdownSystems};
use common::*;

/// `(stage, wall, cpu)` of the stages of a frame.
type Stages = Vec<(&'static str, Duration, Duration)>;

/// Runs of the acceptance tests. Every one counts.
const ACCEPTANCE_RUNS: usize = 5;

/// Runs of the diagnostic tests, or `BEVY_MD_PASTE_DIAG_RUNS`.
const DIAGNOSTIC_RUNS: usize = 12;

/// About `bytes` of markdown, as a synthetic document.
fn source_of(bytes: usize) -> String {
    let sample = synthetic::source(10_000);
    let per_block = sample.len() / 10_000;
    synthetic::source(bytes / per_block)
}

/// One frame of a run.
#[derive(Clone, Copy, Debug)]
struct Frame {
    /// 0 is the frame that made the change.
    index: usize,
    took: Duration,
    /// CPU time of the main thread in the frame. `took - cpu` is time spent
    /// not running our code: descheduled, blocked, or in the kernel for page
    /// faults it did not cause.
    cpu: Duration,
    /// What the main thread allocated in the frame (diagnostic runs only).
    alloc: Option<AllocStats>,
    /// The document shown was still behind the source when the frame began.
    parsing: bool,
}

struct Run {
    /// Every frame, in order.
    frame_times: Vec<Frame>,
    parse_frames: usize,
    /// Where the time of the slowest frame went, by stage of the frame
    /// (see [`Marks`]); only gathered by the diagnostic tests.
    breakdown: Vec<(&'static str, Duration, Duration)>,
    /// The same for the frame with the most main-thread CPU time, which on a
    /// loaded machine is not the slowest by wall time (that one may only
    /// have waited to be scheduled): its wall time, its CPU time and the
    /// stages.
    cpu_breakdown: (Duration, Duration, Vec<(&'static str, Duration, Duration)>),
    /// Where the time of the change went inside the library (wall time).
    timings: bevy_md::ParseTimings,
}

/// Stage boundaries of a frame, noted by systems placed around the ones that
/// matter, to say where the time of the slowest frame went. Used by the
/// diagnostic tests only: the acceptance tests run without them.
#[derive(Resource, Default)]
struct Marks(
    Vec<(
        &'static str,
        std::time::Instant,
        Duration,
        std::thread::ThreadId,
    )>,
);

fn mark(name: &'static str) -> impl FnMut(ResMut<Marks>) {
    move |mut marks| {
        marks.0.push((
            name,
            std::time::Instant::now(),
            thread_cpu(),
            std::thread::current().id(),
        ));
    }
}

fn start_marks(mut marks: ResMut<Marks>) {
    marks.0.clear();
    marks.0.push((
        "frame start",
        std::time::Instant::now(),
        thread_cpu(),
        std::thread::current().id(),
    ));
}

/// Add the systems that note the stages of a frame.
fn add_marks(app: &mut App) {
    app.init_resource::<Marks>()
        .add_systems(First, start_marks)
        .add_systems(
            PreUpdate,
            mark("preupdate done").after(bevy::input::InputSystems),
        )
        .add_systems(Update, mark("update done"))
        .add_systems(
            PostUpdate,
            (
                mark("before sync").before(MarkdownSystems::Sync),
                mark("after sync").after(MarkdownSystems::Sync),
                mark("after ui content").after(UiSystems::Content),
                mark("before ui layout").before(UiSystems::Layout),
                mark("after ui layout").after(UiSystems::Layout),
                mark("after measure").after(MarkdownSystems::Measure),
                mark("after text_system").after(bevy::ui::widget::text_system),
                mark("after post layout").after(UiSystems::PostLayout),
            ),
        )
        .add_systems(Last, mark("last"));
}

impl Run {
    fn worst(&self) -> Frame {
        *self
            .frame_times
            .iter()
            .max_by_key(|f| f.took)
            .expect("a run has frames")
    }

    fn worst_ms(&self) -> f64 {
        ms(self.worst().took)
    }

    /// The `n` slowest frames, slowest first.
    fn slowest(&self, n: usize) -> Vec<Frame> {
        let mut frames = self.frame_times.clone();
        frames.sort_by_key(|f| std::cmp::Reverse(f.took));
        frames.truncate(n);
        frames
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// Apply `change` and run frames until the document has settled, timing every
/// frame from the one that made the change on.
/// What the `Markdown` shows before the change.
#[derive(Clone, Copy)]
enum Start {
    /// Nothing: the first text the app ever lays out is the pasted one, so
    /// the one-time cost of text layout (fonts, shaping data) falls in the
    /// frame that installs it. This is the scenario of the acceptance test.
    Empty,
    /// A few lines, laid out and settled: the same paste in an app that has
    /// shown text before. Diagnosis only, to tell the one-time cost apart.
    Warm,
    /// A document of this many blocks.
    Blocks(usize),
}

/// The document shown has the `expected` source and is what a full parse of
/// it gives, block by block.
fn assert_is_a_full_parse(markdown: &Markdown, expected: &str) {
    assert_eq!(markdown.document().source(), expected);
    let full = Document::parse(markdown.document().source());
    assert_eq!(markdown.document().len(), full.len());
    assert!(
        markdown
            .document()
            .blocks()
            .zip(full.blocks())
            .all(|(a, b)| a == b),
        "the document is not what a full parse gives"
    );
}

/// The middle of `text`, on a character boundary.
fn middle_of(text: &str) -> usize {
    let mut at = text.len() / 2;
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

#[expect(
    clippy::too_many_lines,
    reason = "one run of the scenario, read top to bottom"
)]
fn paste(
    start: Start,
    probes: bool,
    change: impl FnOnce(&mut Markdown, usize),
    expected: impl FnOnce(&str) -> String,
) -> Run {
    let mut app = headless_app();
    if probes {
        add_marks(&mut app);
    }
    // One frame with nothing in it: plugins' startup systems run.
    app.update();
    let (_, entity) = spawn_document(
        &mut app,
        &match start {
            Start::Empty => String::new(),
            Start::Warm => "# Warm-up\n\nWarm **up** the fonts, `code`, and *italic*.\n".to_owned(),
            Start::Blocks(n) => synthetic::source(n),
        },
    );
    settle(&mut app);
    let before = app
        .world()
        .get::<Markdown>(entity)
        .unwrap()
        .document()
        .source()
        .to_owned();
    let at = middle_of(&before);
    let expected = expected(&before);

    let mut frame_times = Vec::new();
    let mut parse_frames = 0;
    #[allow(clippy::type_complexity)]
    let mut breakdown: (Duration, Stages) = (Duration::ZERO, Vec::new());
    #[allow(clippy::type_complexity)]
    let mut cpu_breakdown: (Duration, Duration, Stages) =
        (Duration::ZERO, Duration::ZERO, Vec::new());
    let stages_of = |marks: &Marks| -> Stages {
        let mut last = marks.0.first().map(|(_, at, cpu, id)| (*at, *cpu, *id));
        marks
            .0
            .iter()
            .skip(1)
            .map(|(name, at, cpu, id)| {
                let (last_at, last_cpu, last_id) = last.unwrap_or((*at, *cpu, *id));
                last = Some((*at, *cpu, *id));
                // CPU time is per thread: across two threads the
                // difference means nothing, so it is reported as 0.
                let cpu = if *id == last_id {
                    cpu.saturating_sub(last_cpu)
                } else {
                    Duration::ZERO
                };
                (*name, at.duration_since(last_at), cpu)
            })
            .collect()
    };
    let mut note = |app: &App, took: Duration, cpu: Duration| {
        let Some(marks) = app.world().get_resource::<Marks>() else {
            return;
        };
        if took > breakdown.0 {
            breakdown = (took, stages_of(marks));
        }
        if cpu > cpu_breakdown.1 {
            cpu_breakdown = (took, cpu, stages_of(marks));
        }
    };
    let start = std::time::Instant::now();
    // The change, then the frame that sees it.
    let (took, cpu, alloc) = timed_change(&mut app, entity, probes, |m| change(m, at));
    note(&app, took, cpu);
    frame_times.push(Frame {
        index: 0,
        took,
        cpu,
        alloc,
        parsing: true,
    });
    loop {
        let parsing = app.world().get::<Markdown>(entity).unwrap().is_parsing();
        parse_frames += usize::from(parsing);
        let (took, cpu, alloc) = timed_frame_stats(&mut app, probes);
        note(&app, took, cpu);
        frame_times.push(Frame {
            index: frame_times.len(),
            took,
            cpu,
            alloc,
            parsing,
        });
        if !parsing {
            // A few frames after the result: layout of what came in.
            for _ in 0..6 {
                let (took, cpu, alloc) = timed_frame_stats(&mut app, probes);
                note(&app, took, cpu);
                frame_times.push(Frame {
                    index: frame_times.len(),
                    took,
                    cpu,
                    alloc,
                    parsing: false,
                });
            }
            break;
        }
        // A frame is 16 ms at 60 Hz; the parse is on another thread.
        std::thread::sleep(Duration::from_millis(4));
        assert!(start.elapsed() < Duration::from_secs(120), "never finished");
    }
    let markdown = app.world().get::<Markdown>(entity).unwrap();
    assert_is_a_full_parse(markdown, &expected);
    Run {
        frame_times,
        parse_frames,
        breakdown: breakdown.1,
        cpu_breakdown,
        timings: markdown.parse_timings(),
    }
}

/// One frame: wall time, main-thread CPU time, and (when counting) what the
/// main thread allocated.
fn timed_frame_stats(app: &mut App, count: bool) -> (Duration, Duration, Option<AllocStats>) {
    if count {
        let ((took, cpu), alloc) = count_allocations(|| timed_frame_cpu(app));
        (took, cpu, Some(alloc))
    } else {
        let (took, cpu) = timed_frame_cpu(app);
        (took, cpu, None)
    }
}

/// Make the change and run the frame right after it.
fn timed_change(
    app: &mut App,
    entity: Entity,
    count: bool,
    change: impl FnOnce(&mut Markdown),
) -> (Duration, Duration, Option<AllocStats>) {
    let run = || {
        let cpu = thread_cpu();
        let start = std::time::Instant::now();
        {
            let mut entity = app.world_mut().entity_mut(entity);
            change(&mut entity.get_mut::<Markdown>().unwrap());
        }
        app.update();
        (start.elapsed(), thread_cpu().saturating_sub(cpu))
    };
    if count {
        let ((took, cpu), alloc) = count_allocations(run);
        (took, cpu, Some(alloc))
    } else {
        let (took, cpu) = run();
        (took, cpu, None)
    }
}

/// The load average, as `uptime` prints it, for the record. Best effort.
fn load() -> String {
    std::process::Command::new("uptime")
        .output()
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map_or_else(
            || "unknown".to_owned(),
            |text| {
                text.split("load average")
                    .nth(1)
                    .map_or(text.trim().to_owned(), |load| {
                        format!("load average{}", load.trim_end())
                    })
            },
        )
}

fn describe(frames: &[Frame]) -> String {
    frames
        .iter()
        .map(|f| {
            format!(
                "#{} {:.2} ms (cpu {:.2}){}",
                f.index,
                ms(f.took),
                ms(f.cpu),
                if f.parsing { " (parsing)" } else { "" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn diagnostic_runs() -> usize {
    std::env::var("BEVY_MD_PASTE_DIAG_RUNS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(DIAGNOSTIC_RUNS)
}

/// Run `run` `runs` times, keeping all of them: no round is dropped.
fn all_runs(runs: usize, mut run: impl FnMut() -> Run) -> Vec<Run> {
    (0..runs).map(|_| run()).collect()
}

/// The acceptance check: print the maximum of every run, then fail when any
/// is over the budget.
#[track_caller]
fn accept(name: &str, label: &str, runs: &[Run]) {
    let maxima: Vec<f64> = runs.iter().map(Run::worst_ms).collect();
    let worst = maxima.iter().copied().fold(0.0, f64::max);
    println!("{label}: {load}", load = load());
    println!(
        "{label}: per-run maxima over {} runs (ms): {}",
        runs.len(),
        maxima
            .iter()
            .map(|m| format!("{m:.2}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    for (n, run) in runs.iter().enumerate() {
        println!(
            "{label}: run {n}: {} frames, {} while parsing; slowest (wall, main-thread cpu) {}",
            run.frame_times.len(),
            run.parse_frames,
            describe(&run.slowest(3))
        );
    }
    assert!(
        runs.iter().all(|run| run.parse_frames >= 2),
        "the parse took no frames: not in the background?"
    );
    report(name, worst);
    // The maximum of the maxima: over the limit when any frame of any run is.
    check(name, worst);
}

/// `name +wall ms (cpu ms)` for each stage of a frame.
fn stages(stages: &[(&'static str, Duration, Duration)]) -> String {
    stages
        .iter()
        .map(|(name, took, cpu)| format!("{name} +{:.2} ms (cpu {:.2})", ms(*took), ms(*cpu)))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The distribution of the per-run maxima, for the diagnosis. Never fails on
/// time.
fn diagnose(label: &str, runs: &[Run]) {
    let mut maxima: Vec<f64> = runs.iter().map(Run::worst_ms).collect();
    maxima.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
    let at = |percent: usize| maxima[(maxima.len() - 1) * percent / 100];
    println!("{label}: {}", load());
    println!(
        "{label}: {} runs, per-run maximum (ms): min {:.2}, p50 {:.2}, p90 {:.2}, max {:.2}; over 16 ms: {} of {}",
        maxima.len(),
        at(0),
        at(50),
        at(90),
        at(100),
        maxima.iter().filter(|m| **m > 16.0).count(),
        maxima.len()
    );
    // Which frame the maximum falls in, run by run.
    for (n, run) in runs.iter().enumerate() {
        let worst = run.worst();
        println!(
            "{label}: run {n}: max {:.2} ms at frame #{}{}; next slowest {}",
            ms(worst.took),
            worst.index,
            if worst.parsing { " (parsing)" } else { "" },
            describe(&run.slowest(3)[1..])
        );
    }
    // Where the time of the slowest frame of all went, stage by stage.
    if let Some(run) = runs
        .iter()
        .max_by(|a, b| a.worst_ms().total_cmp(&b.worst_ms()))
    {
        println!(
            "{label}: slowest frame of all ({:.2} ms), by stage: {}",
            run.worst_ms(),
            stages(&run.breakdown)
        );
    }
    // Where the main-thread CPU of the frame that used the most of it went:
    // layout and text are named, and the rest of the frame is what is left.
    if let Some(run) = runs.iter().max_by_key(|run| run.cpu_breakdown.1) {
        let (wall, cpu, by_stage) = &run.cpu_breakdown;
        println!(
            "{label}: frame with the most cpu of all (cpu {:.2} ms, wall {:.2} ms), by stage: {}",
            ms(*cpu),
            ms(*wall),
            stages(by_stage)
        );
    }
    // And of a typical one: the run with the median maximum.
    let mut by_max: Vec<&Run> = runs.iter().collect();
    by_max.sort_by(|a, b| a.worst_ms().total_cmp(&b.worst_ms()));
    if let Some(run) = by_max.get(by_max.len() / 2) {
        println!(
            "{label}: median run's slowest frame ({:.2} ms), by stage: {}",
            run.worst_ms(),
            stages(&run.breakdown)
        );
    }
    // Every run: how much of its slowest frame was CPU and how much was
    // waiting, what the main thread allocated in it, and where the library
    // says the change's time went.
    for (n, run) in runs.iter().enumerate() {
        let worst = run.worst();
        let alloc = worst.alloc.map_or_else(String::new, |a| {
            format!(", allocated {} B in {} calls", a.bytes, a.allocations)
        });
        println!(
            "{label}: run {n}: slowest frame wall {:.2} ms, main-thread cpu {:.2} ms, not running {:.2} ms{alloc}",
            ms(worst.took),
            ms(worst.cpu),
            ms(worst.took.saturating_sub(worst.cpu)),
        );
        let t = run.timings;
        println!(
            "{label}: run {n}: change of {} B, {} new blocks: main source assembly {:.2} ms, start clone {:.2} ms; worker parse {:.2} ms, splice_from {:.2} ms, estimate {:.2} ms; install: heights {:.2} ms, advance {:.2} ms",
            t.bytes,
            t.new_blocks,
            ms(t.source_copy),
            ms(t.start_clone),
            ms(t.parse),
            ms(t.splice_from),
            ms(t.estimate),
            ms(t.install_heights),
            ms(t.install_advance),
        );
    }
    assert!(
        runs.iter().all(|run| run.parse_frames >= 2),
        "the parse took no frames: not in the background?"
    );
}

fn open_run(text: &str, start: Start, probes: bool) -> Run {
    paste(
        start,
        probes,
        |m, _| m.set_source(text),
        |_| text.to_owned(),
    )
}

fn middle_run(pasted: &str, probes: bool) -> Run {
    paste(
        Start::Blocks(2_000),
        probes,
        |m, at| m.edit(at..at, pasted),
        |before| {
            let at = middle_of(before);
            let mut expected = before.to_owned();
            expected.insert_str(at, pasted);
            expected
        },
    )
}

// ---------------------------------------------------------------------------
// Acceptance
// ---------------------------------------------------------------------------

#[test]
fn opening_five_megabytes_in_a_markdown_never_stalls_a_frame() {
    let _serial = serial();
    let text = source_of(5_000_000);
    assert!(text.len() > 4_900_000, "{} bytes", text.len());
    let runs = all_runs(ACCEPTANCE_RUNS, || open_run(&text, Start::Empty, false));
    accept(
        "paste_5mb_worst_frame_ms",
        &format!("open 5 MB ({} bytes)", text.len()),
        &runs,
    );
}

#[test]
fn pasting_five_megabytes_in_the_middle_of_a_document_never_stalls_a_frame() {
    let _serial = serial();
    let pasted = source_of(5_000_000);
    let runs = all_runs(ACCEPTANCE_RUNS, || middle_run(&pasted, false));
    accept(
        "paste_5mb_into_document_worst_frame_ms",
        "paste 5 MB into 2000 blocks",
        &runs,
    );
}

// ---------------------------------------------------------------------------
// Budgets on what the main thread allocates (deterministic, unlike time)
// ---------------------------------------------------------------------------

impl Run {
    /// Bytes the main thread allocated in the frame that made the change:
    /// the text handed to the parse, and nothing else of its size.
    fn change_frame_bytes(&self) -> u64 {
        self.frame_times[0].alloc.expect("counted").bytes
    }

    /// Bytes the main thread allocated in the frame that installed the
    /// result: the last one that began while the parse was running.
    fn install_frame_bytes(&self) -> u64 {
        self.frame_times
            .iter()
            .rev()
            .find(|f| f.parsing)
            .and_then(|f| f.alloc)
            .expect("counted")
            .bytes
    }
}

#[test]
fn a_five_megabyte_change_is_copied_once_on_the_main_thread() {
    let _serial = serial();
    let text = source_of(5_000_000);
    // Opening: `set_source(&str)` copies the caller's text once; the task and
    // the document then share that copy.
    let open = open_run(&text, Start::Warm, true);
    let one_copy = text.len() as u64;
    report(
        "paste_5mb_open_change_frame_allocated_bytes",
        open.change_frame_bytes(),
    );
    assert!(
        open.change_frame_bytes() >= one_copy,
        "the text is copied at least once: {} bytes",
        open.change_frame_bytes()
    );
    check(
        "paste_5mb_open_change_frame_allocated_bytes",
        open.change_frame_bytes(),
    );
    // Pasting into a document: the new text is built once (old text with the
    // paste in it), and the parse works on that same allocation.
    let pasted = source_of(5_000_000);
    let middle = middle_run(&pasted, true);
    report(
        "paste_5mb_into_document_change_frame_allocated_bytes",
        middle.change_frame_bytes(),
    );
    assert!(middle.change_frame_bytes() >= pasted.len() as u64);
    check(
        "paste_5mb_into_document_change_frame_allocated_bytes",
        middle.change_frame_bytes(),
    );
    // The frame that installs it: the height index arrives built, so the
    // install allocates for the window (and a few nodes), not for 142,000
    // blocks; before M8 it allocated a vector of every new block's height.
    report(
        "paste_5mb_into_document_install_frame_allocated_bytes",
        middle.install_frame_bytes(),
    );
    check(
        "paste_5mb_into_document_install_frame_allocated_bytes",
        middle.install_frame_bytes(),
    );
}

// ---------------------------------------------------------------------------
// Diagnosis
// ---------------------------------------------------------------------------

#[test]
fn opening_five_megabytes_frame_time_distribution() {
    let _serial = serial();
    let text = source_of(5_000_000);
    let runs = all_runs(diagnostic_runs(), || open_run(&text, Start::Empty, true));
    diagnose(
        "open 5 MB, nothing laid out before (acceptance scenario)",
        &runs,
    );
    let runs = all_runs(diagnostic_runs(), || open_run(&text, Start::Warm, true));
    diagnose("open 5 MB, after a few lines of text were laid out", &runs);
}

#[test]
fn pasting_five_megabytes_in_the_middle_frame_time_distribution() {
    let _serial = serial();
    let pasted = source_of(5_000_000);
    let runs = all_runs(diagnostic_runs(), || middle_run(&pasted, true));
    diagnose("paste 5 MB into 2000 blocks", &runs);
}
