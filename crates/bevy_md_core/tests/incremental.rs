//! Incremental updates must always equal a full parse of the new source.

mod support;

use bevy_md_core::{BlockKind, BlockRef, Document, Inline, ListMarker, Splice, Splices, Style};

const SAMPLE: &str = include_str!("fixtures/sample.md");

/// Tokens that change markdown structure the most when typed or deleted.
const TOKENS: &[&str] = &[
    "a",
    " ",
    "\n",
    "\n\n",
    "# ",
    "- ",
    "1. ",
    "> ",
    "```",
    "```rust\n",
    "*",
    "**",
    "`",
    "~~",
    "|",
    "---",
    "    ",
    "[x] ",
    "===",
    "$",
    "[",
    "](u)",
    "é",
    "日本",
];

/// xorshift64*, so the test needs no dependency and stays reproducible.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn char_boundary(source: &str, mut pos: usize) -> usize {
    while !source.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// Mapped runs are verbatim copies of the source: a soft line break shows
/// as a space where the source has a newline, and nothing else differs.
fn assert_source_map_is_verbatim(doc: &Document, context: &str) {
    let check = |base: usize, inline: &Inline| {
        for run in &inline.runs {
            let Some(range) = run.source_range() else {
                continue;
            };
            let copy = &doc.source()[base + range.start..base + range.end];
            let text = inline.run_text(run);
            assert!(
                copy.len() == text.len()
                    && copy
                        .chars()
                        .zip(text.chars())
                        .all(|(src, shown)| src == shown || (src == '\n' && shown == ' ')),
                "run {text:?} is not a copy of {copy:?} after {context}\nsource:\n{}",
                doc.source()
            );
        }
    };
    for block in doc.blocks() {
        check(block.span().start, &block.content);
        if let BlockKind::Table { rows, .. } = &block.kind {
            for cell in rows.iter().flatten() {
                check(block.span().start, cell);
            }
        }
    }
}

fn assert_matches_full_parse(doc: &Document, context: &str) {
    doc.assert_consistent();
    assert_source_map_is_verbatim(doc, context);
    let full = Document::parse(doc.source());
    full.assert_consistent();
    assert!(
        doc.blocks().eq(full.blocks()),
        "incremental blocks differ from a full parse after {context}\nsource:\n{}",
        doc.source()
    );
    // Positions answer the same from both ends: block_at(span.start) finds
    // the block back (or the one before it that ends exactly there).
    for (ix, block) in doc.blocks().enumerate().step_by(3) {
        let found = doc
            .block_at(block.span().start)
            .expect("a block holds its start");
        assert!(found <= ix, "block_at went forward in {context}");
        assert_eq!(doc.block(ix).as_ref(), Some(&block));
    }
}

#[test]
fn random_edits_match_full_parse() {
    let seeds: u64 = std::env::var("BEVY_MD_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    for seed in 1..=seeds {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        // Without the definitions that force full parses, so the
        // incremental path is what gets exercised.
        let mut doc = Document::parse(SAMPLE);
        for step in 0..150 {
            let len = doc.source().len();
            let start = char_boundary(doc.source(), rng.below(len + 1));
            let end = if rng.below(3) == 0 {
                char_boundary(doc.source(), (start + rng.below(12)).min(len))
            } else {
                start
            };
            let text = if rng.below(4) == 0 {
                ""
            } else {
                TOKENS[rng.below(TOKENS.len())]
            };
            let before = support::snapshot(&doc);
            let splices = doc.edit(start..end, text);
            support::assert_explains(
                &before,
                &doc,
                &splices,
                Some(text.len().cast_signed() - (end - start).cast_signed()),
                &format!("seed {seed} step {step}: {start}..{end} -> {text:?}"),
            );
            assert_matches_full_parse(
                &doc,
                &format!("seed {seed} step {step}: {start}..{end} -> {text:?}"),
            );
        }
    }
}

/// Documents big enough for the block tree to have several levels, edited
/// at random places, some of them spanning many blocks.
#[test]
fn random_edits_in_a_large_document_match_a_full_parse() {
    let seeds: u64 = std::env::var("BEVY_MD_SEEDS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .map_or(3, |s| (s / 100).max(3));
    for seed in 1..=seeds {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut doc = Document::parse(bevy_md_core::synthetic::source(1_500));
        assert!(doc.len() >= 1_500);
        for step in 0..60 {
            let len = doc.source().len();
            let start = char_boundary(doc.source(), rng.below(len + 1));
            let reach = match rng.below(6) {
                0 => rng.below(4_000),
                1 => rng.below(40),
                _ => 0,
            };
            let end = char_boundary(doc.source(), (start + reach).min(len));
            let text = if rng.below(5) == 0 {
                ""
            } else if rng.below(8) == 0 {
                SAMPLE
            } else {
                TOKENS[rng.below(TOKENS.len())]
            };
            let before = support::snapshot(&doc);
            let splices = doc.edit(start..end, text);
            support::assert_explains(
                &before,
                &doc,
                &splices,
                Some(text.len().cast_signed() - (end - start).cast_signed()),
                &format!("seed {seed} step {step}: {start}..{end} -> {text:?}"),
            );
            assert_matches_full_parse(
                &doc,
                &format!("seed {seed} step {step}: {start}..{end} -> {text:?}"),
            );
        }
    }
}

#[test]
fn streaming_matches_full_parse() {
    for chunk in [1, 3, 7, 32] {
        let mut doc = Document::new();
        let mut at = 0;
        while at < SAMPLE.len() {
            let end = char_boundary(SAMPLE, (at + chunk).min(SAMPLE.len()));
            let end = if end == at { at + chunk } else { end };
            doc.push_str(&SAMPLE[at..end]);
            assert_matches_full_parse(&doc, &format!("chunk {chunk} at {at}"));
            at = end;
        }
    }
}

#[test]
fn edits_far_away_leave_blocks_alone() {
    let mut doc = Document::parse(SAMPLE);
    let last = doc.len() - 1;
    let pos = doc.block(last).unwrap().span().start;
    let splices = doc.edit(pos..pos, "Very ");
    assert_eq!(
        splices.as_slice(),
        [Splice {
            old: last..last + 1,
            new: last..last + 1
        }]
    );
}

#[test]
fn a_definition_added_later_reaches_its_references_without_a_full_parse() {
    let mut doc = Document::parse("See [the map][map].\n\nText.\n");
    assert_eq!(doc.block(0).unwrap().content.links.len(), 0);
    let end = doc.source().len();
    let parses = doc.stats().full_parses;
    doc.edit(end..end, "\n[map]: https://example.com\n");
    assert_eq!(
        &*doc.block(0).unwrap().content.links[0],
        "https://example.com"
    );
    assert_matches_full_parse(&doc, "adding a definition");
    assert_eq!(
        doc.stats().full_parses,
        parses,
        "the index finds the users itself"
    );
}

#[test]
fn block_model() {
    let doc = Document::parse(SAMPLE);
    let blocks: Vec<BlockRef> = doc.blocks().collect();
    assert_eq!(blocks[0].kind, BlockKind::Heading(1));
    let bold = blocks[1]
        .content
        .runs
        .iter()
        .find(|run| run.style.contains(Style::BOLD))
        .expect("a bold run");
    assert_eq!(blocks[1].content.run_text(bold), "Blackwater Keep");

    let nested = blocks
        .iter()
        .find(|b| b.content.text.starts_with("nested"))
        .expect("the nested item");
    assert_eq!(nested.list_depth, 2);

    let task = blocks
        .iter()
        .find(|b| b.content.text == "identify the amulet")
        .expect("the task");
    assert_eq!(
        task.kind,
        BlockKind::ListItem {
            marker: ListMarker::Bullet,
            task: Some(false)
        }
    );

    let quote = blocks
        .iter()
        .find(|b| b.content.text.contains("Nothing lives"))
        .expect("the inner quote");
    assert_eq!(quote.quote_depth, 2);

    assert!(blocks.iter().any(|b| matches!(
        &b.kind,
        BlockKind::CodeBlock { lang: Some(lang) } if &**lang == "rust"
    )));
    assert!(blocks.iter().any(|b| matches!(
        &b.kind,
        BlockKind::Table { rows, .. } if rows.len() == 3
    )));
}

#[test]
fn frontmatter_only_at_the_start() {
    let doc = Document::parse("---\ntitle: Keep\n---\n# Notes\n\n---\nnot: metadata\n---\n");
    let kinds: Vec<_> = doc.blocks().map(|b| b.kind.clone()).collect();
    assert_eq!(kinds[0], BlockKind::Frontmatter);
    assert_eq!(doc.block(0).unwrap().content.text, "title: Keep\n");
    assert_eq!(kinds[1], BlockKind::Heading(1));
    assert_eq!(kinds[2], BlockKind::Rule);
}

#[test]
fn random_edits_on_a_document_with_frontmatter() {
    let source = format!("---\ntitle: Keep\ntags: [rpg]\n---\n{SAMPLE}");
    let mut rng = Rng(7);
    let mut doc = Document::parse(source);
    for step in 0..400 {
        let len = doc.source().len();
        let start = char_boundary(doc.source(), rng.below(len + 1));
        let text = TOKENS[rng.below(TOKENS.len())];
        doc.edit(start..start, text);
        assert_matches_full_parse(&doc, &format!("step {step}: {start} -> {text:?}"));
    }
}

#[test]
fn closing_a_leading_rule_turns_it_into_frontmatter() {
    let mut doc = Document::parse("---\ntitle: Keep\n\n# Notes\n");
    assert_eq!(doc.block(0).unwrap().kind, BlockKind::Rule);
    let at = doc.source().find("\n# Notes").expect("heading");
    doc.edit(at..at, "\n---");
    assert_eq!(doc.block(0).unwrap().kind, BlockKind::Frontmatter);
    assert_matches_full_parse(&doc, "closing the front matter");
}

/// `splice_from` is what the background parse hands over: the blocks outside
/// it must be the same text at the same place (from the start) or at the same
/// distance from the end, and the block counts must add up.
#[test]
fn splice_from_names_the_blocks_between_two_documents() {
    let seeds: u64 = std::env::var("BEVY_MD_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(40, |s: u64| s.min(400));
    for seed in 1..=seeds {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let old = Document::parse(if seed % 2 == 0 {
            SAMPLE.to_owned()
        } else {
            bevy_md_core::synthetic::source(300)
        });
        let mut source = old.source().to_owned();
        for _ in 0..=rng.below(4) {
            let len = source.len();
            let start = char_boundary(&source, rng.below(len + 1));
            let reach = if rng.below(3) == 0 { rng.below(600) } else { 0 };
            let end = char_boundary(&source, (start + reach).min(len));
            let text = if rng.below(4) == 0 {
                ""
            } else {
                TOKENS[rng.below(TOKENS.len())]
            };
            source.replace_range(start..end, text);
        }
        let new = Document::parse(source.as_str());
        let splices = new.splice_from(&old);
        let context = format!("seed {seed}: {splices:?}");
        support::assert_explains(&support::snapshot(&old), &new, &splices, None, &context);
        // What is left out is where an edit leaves it: at the same start,
        // or at the same distance from the end of the source.
        for ix in 0..old.len() {
            let Some(now) = splices.map_old(ix) else {
                continue;
            };
            let (o, n) = (old.block(ix).unwrap(), new.block(now).unwrap());
            assert!(
                o.span().start == n.span().start
                    || old.source().len() - o.span().start == new.source().len() - n.span().start,
                "{context}: block {ix} (now {now}) is neither at the same place nor the same distance from the end"
            );
        }
        // Equal documents have an empty list.
        assert!(new.splice_from(&new).is_empty());
    }
}

/// Several edits made before anyone looks compose into one list that goes
/// straight from the first document to the last: what it leaves out is
/// unchanged, whatever the edits did to the number of blocks in between.
#[test]
fn edits_made_one_after_another_compose_into_what_happened() {
    let seeds: u64 = std::env::var("BEVY_MD_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map_or(40, |s: u64| s.min(600));
    for seed in 1..=seeds {
        let mut rng = Rng(seed.wrapping_mul(0xA24B_AED4_963E_E407));
        let mut doc = Document::parse(if seed % 2 == 0 {
            SAMPLE.to_owned()
        } else {
            bevy_md_core::synthetic::references_source(8)
        });
        let first = support::snapshot(&doc);
        let mut pending = Splices::new();
        for step in 0..=rng.below(8) {
            let len = doc.source().len();
            let start = char_boundary(doc.source(), rng.below(len + 1));
            let reach = if rng.below(3) == 0 { rng.below(80) } else { 0 };
            let end = char_boundary(doc.source(), (start + reach).min(len));
            let text = if rng.below(4) == 0 {
                ""
            } else {
                TOKENS[rng.below(TOKENS.len())]
            };
            let step_before = support::snapshot(&doc);
            let splices = doc.edit(start..end, text);
            let context = format!("seed {seed} step {step}: {start}..{end} -> {text:?}");
            support::assert_explains(&step_before, &doc, &splices, None, &context);
            pending.compose(&splices);
            support::assert_explains(
                &first,
                &doc,
                &pending,
                None,
                &format!("{context}, composed"),
            );
            assert_matches_full_parse(&doc, &context);
        }
    }
}
