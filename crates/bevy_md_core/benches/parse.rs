//! Budgets these benchmarks guard (see PLAN.md): a keystroke in a large
//! document and a streamed chunk must cost microseconds, never a full parse.

use std::hint::black_box;

use bevy_md_core::Document;
use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};

const SAMPLE: &str = include_str!("../tests/fixtures/sample.md");

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

fn full_parse(c: &mut Criterion) {
    let source = large_source();
    let mut group = c.benchmark_group("full_parse");
    group.throughput(Throughput::Bytes(source.len() as u64));
    group.bench_function("1MB", |b| {
        b.iter(|| Document::parse(black_box(source.as_str())));
    });
    group.finish();
}

fn keystroke(c: &mut Criterion) {
    let doc = Document::parse(large_source());
    let middle = doc.block(doc.len() / 2).expect("a block").span().start + 3;
    c.bench_function("keystroke_in_1MB", |b| {
        b.iter_batched_ref(
            || doc.clone(),
            |doc| black_box(doc.edit(middle..middle, "x")),
            BatchSize::LargeInput,
        );
    });
}

fn stream(c: &mut Criterion) {
    let mut group = c.benchmark_group("stream");
    group.throughput(Throughput::Bytes(SAMPLE.len() as u64));
    for chunk in [4, 32] {
        group.bench_function(format!("sample_in_{chunk}_byte_chunks"), |b| {
            b.iter(|| {
                let mut doc = Document::new();
                for piece in SAMPLE.as_bytes().chunks(chunk) {
                    // The sample's multibyte characters sit inside chunks
                    // only when the chunk boundaries allow it.
                    if let Ok(piece) = std::str::from_utf8(piece) {
                        black_box(doc.push_str(piece));
                    } else {
                        black_box(doc.push_str(&String::from_utf8_lossy(piece)));
                    }
                }
                doc
            });
        });
    }
    group.finish();
}

criterion_group!(benches, full_parse, keystroke, stream);
criterion_main!(benches);
