//! Stage-by-stage measurement of the document paths the budgets care about:
//! a keystroke in 1 MB, a streamed chunk, the parse of a large paste and the
//! work around it.
//!
//! This is a plain program (no criterion): every sample is kept and the
//! table prints the minimum, median, 95th and 99th percentile and the
//! maximum of **all** of them, in wall time *and* in the CPU time of the
//! measuring thread, so a slow sample can be told apart from a descheduled
//! one (wall far above CPU). Nothing is selected: warm-up samples are
//! discarded by count, never by value.
//!
//! Run with `cargo bench -p bevy_md_core --bench stages` (the `bench`
//! profile: fat LTO, optimized). Set `BEVY_MD_STAGES_ITERS` to change the
//! number of samples (default 5000; the parse of 5 MB uses a hundredth).
//! Allocation counts come from the `allocation-counter` crate, which counts
//! the calling thread only.

use std::hint::black_box;
use std::time::{Duration, Instant};

use bevy_md_core::estimate::{Metrics, estimate_height};
use bevy_md_core::{Document, synthetic};

const SAMPLE: &str = include_str!("../tests/fixtures/sample.md");

/// CPU time of the calling thread so far.
fn cpu_now() -> Duration {
    let time = rustix::time::clock_gettime(rustix::time::ClockId::ThreadCPUTime);
    Duration::new(
        u64::try_from(time.tv_sec).unwrap_or(0),
        u32::try_from(time.tv_nsec).unwrap_or(0),
    )
}

#[derive(Default)]
struct Samples {
    wall: Vec<f64>,
    cpu: Vec<f64>,
}

impl Samples {
    fn time<R>(&mut self, f: impl FnOnce() -> R) -> R {
        let cpu = cpu_now();
        let wall = Instant::now();
        let out = f();
        let wall = wall.elapsed();
        let cpu = cpu_now().saturating_sub(cpu);
        self.wall.push(wall.as_secs_f64() * 1e6);
        self.cpu.push(cpu.as_secs_f64() * 1e6);
        out
    }

    fn row(&self, name: &str) {
        let line = |label: &str, values: &[f64]| {
            let mut sorted = values.to_vec();
            sorted.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
            let at = |p: usize| sorted[(sorted.len() - 1) * p / 100];
            format!(
                "{label} min {:>9.2} p50 {:>9.2} p95 {:>9.2} p99 {:>9.2} max {:>9.2}",
                at(0),
                at(50),
                at(95),
                at(99),
                at(100)
            )
        };
        println!(
            "{name} (n={}, microseconds)\n    {}\n    {}",
            self.wall.len(),
            line("wall", &self.wall),
            line("cpu ", &self.cpu)
        );
    }
}

fn iterations(default: usize) -> usize {
    std::env::var("BEVY_MD_STAGES_ITERS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(default)
}

/// About 1 MB of markdown: the sample repeated.
fn large_source() -> String {
    let copies = 1_000_000 / SAMPLE.len() + 1;
    let mut source = String::with_capacity(copies * (SAMPLE.len() + 1));
    for _ in 0..copies {
        source.push_str(SAMPLE);
        source.push('\n');
    }
    source
}

fn allocations<R>(f: impl FnOnce() -> R) -> (R, u64, u64) {
    let mut out = None;
    let info = allocation_counter::measure(|| out = Some(f()));
    (out.expect("ran"), info.count_total, info.bytes_total)
}

fn keystroke() {
    let n = iterations(5000);
    let warmup = 500;
    let source = large_source();
    let base = Document::parse(source.as_str());
    let middle = base.block(base.len() / 2).expect("a block").span().start + 3;
    println!(
        "\n== keystroke in a {} byte document ({} blocks), insert 1 byte at {middle}",
        source.len(),
        base.len()
    );

    // A fresh copy for every sample (the copy is not timed): what the
    // criterion benchmark `keystroke_in_1MB` measures.
    let mut fresh = Samples::default();
    for i in 0..n + warmup {
        let mut doc = base.clone();
        if i < warmup {
            black_box(doc.edit(middle..middle, "x"));
        } else {
            fresh.time(|| black_box(doc.edit(middle..middle, "x")));
        }
    }
    fresh.row("Document::edit, fresh clone each sample");

    // Typing: the same document edited over and over, the buffer already
    // grown, the way a user's keystrokes arrive.
    let mut typing = Samples::default();
    let mut doc = base.clone();
    for i in 0..n + warmup {
        if i < warmup {
            black_box(doc.edit(middle..middle, "x"));
        } else {
            typing.time(|| black_box(doc.edit(middle..middle, "x")));
        }
    }
    typing.row("Document::edit, same document, repeated");

    // The floor: what the String alone costs for the same edit.
    let mut floor = Samples::default();
    let mut text = source.clone();
    text.reserve(n + warmup + 16);
    for i in 0..n + warmup {
        if i < warmup {
            text.replace_range(middle..middle, "x");
        } else {
            floor.time(|| {
                text.replace_range(middle..middle, black_box("x"));
            });
        }
    }
    floor.row("String::replace_range alone (the memmove of the suffix)");

    // The same edit near the end: the memmove is almost nothing, so what is
    // left is everything that is not the String.
    let end = base.block(base.len() - 2).expect("a block").span().start + 3;
    let mut near_end = Samples::default();
    let mut doc = base.clone();
    for i in 0..n + warmup {
        if i < warmup {
            black_box(doc.edit(end..end, "x"));
        } else {
            near_end.time(|| black_box(doc.edit(end..end, "x")));
        }
    }
    near_end.row("Document::edit, same document, edit near the END (memmove ~0)");

    let mut doc = base.clone();
    doc.edit(middle..middle, "y");
    let (_, count, bytes) = allocations(|| doc.edit(middle..middle, "x"));
    println!("allocations of one keystroke: {count} calls, {bytes} bytes");
}

fn stream() {
    let n = iterations(5000) / 5;
    let warmup = 200;
    for chunk in [4usize, 32] {
        let pieces: Vec<String> = SAMPLE
            .as_bytes()
            .chunks(chunk)
            .map(|piece| String::from_utf8_lossy(piece).into_owned())
            .collect();
        println!(
            "\n== stream: the {} byte sample in {} chunks of {chunk} bytes (whole document per sample)",
            SAMPLE.len(),
            pieces.len()
        );
        let mut samples = Samples::default();
        for i in 0..n + warmup {
            let run = || {
                let mut doc = Document::new();
                for piece in &pieces {
                    black_box(doc.push_str(piece));
                }
                doc
            };
            if i < warmup {
                black_box(run());
            } else {
                black_box(samples.time(run));
            }
        }
        samples.row("push_str of every chunk");
        let (_, count, bytes) = allocations(|| {
            let mut doc = Document::new();
            for piece in &pieces {
                black_box(doc.push_str(piece));
            }
            doc
        });
        // Counts far below 2^52: the conversions are exact.
        #[allow(clippy::cast_precision_loss)]
        let per_chunk = |n: u64| n as f64 / pieces.len() as f64;
        println!(
            "allocations of the whole document: {count} calls, {bytes} bytes ({:.1} calls, {:.0} bytes per chunk)",
            per_chunk(count),
            per_chunk(bytes)
        );
    }
}

fn large_paste() {
    let n = (iterations(5000) / 100).max(10);
    let bytes_wanted = 5_000_000;
    let per_block = synthetic::source(10_000).len() / 10_000;
    let source = synthetic::source(bytes_wanted / per_block);
    println!(
        "\n== the worker's side of a 5 MB paste ({} bytes), on this thread",
        source.len()
    );
    let old = Document::parse(synthetic::source(2_000));
    let metrics = Metrics::default();

    let mut parse = Samples::default();
    let mut diff = Samples::default();
    let mut estimate = Samples::default();
    let mut copy = Samples::default();
    let mut last = None;
    for i in 0..n + 2 {
        let text = if i < 2 {
            source.clone()
        } else {
            copy.time(|| source.clone())
        };
        let document = if i < 2 {
            Document::parse(text)
        } else {
            parse.time(|| Document::parse(text))
        };
        let splices = if i < 2 {
            document.splice_from(&old)
        } else {
            diff.time(|| document.splice_from(&old))
        };
        let covering = splices.covering();
        let heights: Vec<f32> = if i < 2 {
            document
                .blocks_from(covering.new.start)
                .take(covering.new.len())
                .map(|block| estimate_height(block.block(), &metrics))
                .collect()
        } else {
            estimate.time(|| {
                document
                    .blocks_from(covering.new.start)
                    .take(covering.new.len())
                    .map(|block| estimate_height(block.block(), &metrics))
                    .collect()
            })
        };
        last = Some((document, splices, heights));
    }
    copy.row("copy of the 5 MB source (String::clone)");
    parse.row("Document::parse(String)");
    diff.row("Document::splice_from(old of 2000 blocks)");
    estimate.row("estimate_height of every new block");
    drop(last);

    let ((), count, bytes) = allocations(|| {
        let document = Document::parse(source.clone());
        black_box(&document);
    });
    println!("allocations of source copy + parse: {count} calls, {bytes} bytes");
}

fn main() {
    println!(
        "bevy_md_core stage probe; load: {}",
        std::process::Command::new("uptime")
            .output()
            .ok()
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .unwrap_or_default()
            .trim()
    );
    // `stages keystroke`, `stages stream`, `stages paste`: only those.
    let only: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let wanted = |name: &str| only.is_empty() || only.iter().any(|o| o == name);
    if wanted("keystroke") {
        keystroke();
    }
    if wanted("stream") {
        stream();
    }
    if wanted("paste") {
        large_paste();
    }
}
