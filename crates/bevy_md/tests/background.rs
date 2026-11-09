//! Large changes are parsed off the main thread, with revisions, and end up
//! exactly where a full parse of the final source would.

mod common;

use bevy::prelude::*;
use bevy_md::Markdown;
use bevy_md::core::{Document, synthetic};
use common::*;

struct Open {
    app: App,
    markdown: Entity,
}

impl Open {
    fn new(source: &str) -> Self {
        let mut app = headless_app();
        app.update();
        let (_, markdown) = spawn_document(&mut app, source);
        settle(&mut app);
        Self { app, markdown }
    }

    fn markdown(&self) -> &Markdown {
        self.app
            .world()
            .get::<Markdown>(self.markdown)
            .expect("markdown")
    }

    fn change(&mut self, f: impl FnOnce(&mut Markdown)) {
        let mut entity = self.app.world_mut().entity_mut(self.markdown);
        f(&mut entity.get_mut::<Markdown>().expect("markdown"));
    }

    /// The document shown is what a full parse of its own source gives.
    #[track_caller]
    fn assert_full_parse(&self) {
        let shown = self.markdown().document();
        let full = Document::parse(shown.source());
        shown.assert_consistent();
        assert_eq!(shown.len(), full.len(), "block count");
        for (ix, (a, b)) in shown.blocks().zip(full.blocks()).enumerate() {
            assert_eq!(a, b, "block {ix}");
        }
    }
}

/// A source well over the synchronous limit.
fn big(blocks: usize) -> String {
    let source = synthetic::source(blocks * 8);
    assert!(
        source.len() > Markdown::SYNC_LIMIT * 4,
        "{} bytes",
        source.len()
    );
    source
}

#[test]
fn small_changes_are_applied_before_any_frame() {
    let _serial = serial();
    let mut doc = Open::new("hello\n\nworld\n");
    doc.change(|m| m.push_str(" again"));
    assert!(!doc.markdown().is_parsing());
    assert!(
        doc.markdown()
            .document()
            .source()
            .ends_with("world\n again")
    );
    doc.change(|m| m.edit(0..5, "bye"));
    assert!(doc.markdown().document().source().starts_with("bye\n"));
    doc.change(|m| m.set_source("# replaced\n"));
    assert!(!doc.markdown().is_parsing());
    assert_eq!(doc.markdown().document().source(), "# replaced\n");
    assert_eq!(doc.markdown().parse_revision(), 0);
    settle(&mut doc.app);
    doc.assert_full_parse();
}

#[test]
fn a_large_replacement_is_parsed_in_the_background() {
    let _serial = serial();
    let mut doc = Open::new("# Before\n\nSome text.\n");
    let source = big(400);
    doc.change(|m| m.set_source(&source));
    // At once: the source is the new one, the document shown is the old one.
    assert!(doc.markdown().is_parsing());
    assert_eq!(doc.markdown().source(), source);
    assert_eq!(
        doc.markdown().document().source(),
        "# Before\n\nSome text.\n"
    );
    assert_eq!(doc.markdown().parse_revision(), 1);

    settle(&mut doc.app);
    assert!(!doc.markdown().is_parsing());
    assert_eq!(doc.markdown().document().source(), source);
    doc.assert_full_parse();
    assert_eq!(doc.markdown().discarded_parses(), 0);
    assert!(
        doc.markdown().live_blocks().len() > 10,
        "the new blocks are shown"
    );
}

#[test]
fn opening_a_text_in_an_empty_markdown_works_the_same() {
    let _serial = serial();
    let mut doc = Open::new("");
    let source = big(300);
    doc.change(|m| m.set_source(&source));
    assert!(doc.markdown().is_parsing());
    settle(&mut doc.app);
    assert_eq!(doc.markdown().document().source(), source);
    doc.assert_full_parse();
    assert!(doc.markdown().live_blocks().len() > 10);
}

#[test]
fn a_newer_large_change_throws_the_older_result_away() {
    let _serial = serial();
    let mut doc = Open::new("# Before\n");
    let first = big(500);
    let second = big(300);
    doc.change(|m| m.set_source(&first));
    // A frame starts the parse of the first; then the second comes.
    doc.app.update();
    doc.change(|m| m.set_source(&second));
    assert_eq!(doc.markdown().discarded_parses(), 1);
    assert_eq!(doc.markdown().parse_revision(), 2);
    for _ in 0..2_000 {
        doc.app.update();
        assert_ne!(
            doc.markdown().document().source(),
            first,
            "the first result was applied"
        );
        if !doc.markdown().is_parsing() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert_eq!(doc.markdown().document().source(), second);
    doc.assert_full_parse();
}

#[test]
fn appends_that_arrive_while_parsing_are_merged_and_applied_after() {
    let _serial = serial();
    let mut doc = Open::new("");
    let source = big(400);
    doc.change(|m| m.set_source(&source));
    doc.app.update();
    let mut expected = source.clone();
    for k in 0..50 {
        let chunk = format!("streamed {k} ");
        expected.push_str(&chunk);
        doc.change(|m| m.push_str(&chunk));
        assert_eq!(doc.markdown().source(), expected);
    }
    // Nothing of it is in the document shown yet.
    assert!(!doc.markdown().document().source().contains("streamed 0"));
    settle(&mut doc.app);
    assert_eq!(doc.markdown().document().source(), expected);
    assert!(!doc.markdown().is_parsing());
    doc.assert_full_parse();
}

#[test]
fn edits_made_while_parsing_refer_to_the_latest_source() {
    let _serial = serial();
    let mut doc = Open::new("# Old\n");
    let source = big(300);
    doc.change(|m| m.set_source(&source));
    doc.app.update();
    doc.change(|m| m.edit(0..0, "# New first\n\n"));
    doc.change(|m| {
        let at = m.source().len();
        m.edit(at..at, "\nThe end.\n");
    });
    let mut expected = format!("# New first\n\n{source}");
    expected.push_str("\nThe end.\n");
    assert_eq!(doc.markdown().source(), expected);
    settle(&mut doc.app);
    assert_eq!(doc.markdown().document().source(), expected);
    doc.assert_full_parse();
}

#[test]
fn a_large_edit_in_the_middle_keeps_the_blocks_around_it() {
    let _serial = serial();
    let mut doc = Open::new(&synthetic::source(2_000));
    let before = doc.markdown().block_entity(3).expect("block 3 is shown");
    let at = doc.markdown().document().block(1_000).unwrap().span().start;
    let pasted = big(300);
    let mut expected = doc.markdown().source().to_owned();
    expected.insert_str(at, &pasted);
    doc.change(|m| m.edit(at..at, &pasted));
    assert!(doc.markdown().is_parsing());
    settle(&mut doc.app);
    assert_eq!(doc.markdown().document().source(), expected);
    doc.assert_full_parse();
    // The blocks before the paste were not replaced: their nodes are the same.
    assert_eq!(doc.markdown().block_entity(3), Some(before));
    assert!(doc.markdown().document().len() > 2_000 + 300);
}

#[test]
fn after_the_result_small_changes_are_synchronous_again() {
    let _serial = serial();
    let mut doc = Open::new("");
    let source = big(200);
    doc.change(|m| m.set_source(&source));
    settle(&mut doc.app);
    doc.change(|m| m.push_str("x"));
    assert!(!doc.markdown().is_parsing());
    assert!(doc.markdown().document().source().ends_with('x'));
}

#[test]
fn a_small_replacement_cancels_a_parse_in_flight() {
    let _serial = serial();
    let mut doc = Open::new("");
    doc.change(|m| m.set_source(&big(400)));
    doc.app.update();
    doc.change(|m| m.set_source("# Small\n"));
    assert!(!doc.markdown().is_parsing());
    assert_eq!(doc.markdown().discarded_parses(), 1);
    assert_eq!(doc.markdown().document().source(), "# Small\n");
    settle(&mut doc.app);
    assert_eq!(doc.markdown().document().source(), "# Small\n");
    doc.assert_full_parse();
}

// ---------------------------------------------------------------------------
// Ownership of the source: one allocation from the call to the installed
// document
// ---------------------------------------------------------------------------

/// Run frames until the parse has been installed.
fn run_until_installed(doc: &mut Open) {
    for _ in 0..2_000 {
        doc.app.update();
        if !doc.markdown().is_parsing() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    panic!("the parse never landed");
}

#[test]
fn an_owned_text_reaches_the_installed_document_without_a_copy() {
    let _serial = serial();
    let mut doc = Open::new(
        "# Before
",
    );
    let text = big(400);
    let at = text.as_ptr();
    let len = text.len();
    doc.change(|m| m.set_source_owned(text));
    // The logical source is the very allocation the caller handed over...
    assert!(doc.markdown().is_parsing());
    assert_eq!(doc.markdown().source().as_ptr(), at, "logical source");
    assert_eq!(doc.markdown().source().len(), len);
    // ...the parse builds its document on it...
    doc.app.update();
    assert_eq!(doc.markdown().source().as_ptr(), at);
    // ...and it is still that allocation once the document is installed.
    run_until_installed(&mut doc);
    assert_eq!(doc.markdown().document().source().as_ptr(), at, "document");
    doc.assert_full_parse();
}

#[test]
fn opening_a_text_with_set_source_makes_one_copy_and_hands_it_on_without_another() {
    let _serial = serial();
    let mut doc = Open::new("");
    let text = big(400);
    doc.change(|m| m.set_source(&text));
    // `set_source` takes `&str`, so the text is copied once on the way in...
    let logical = doc.markdown().source().as_ptr();
    assert_ne!(logical, text.as_ptr());
    // ...and then travels to the task and the document as it is.
    run_until_installed(&mut doc);
    assert_eq!(doc.markdown().document().source().as_ptr(), logical);
    assert_eq!(doc.markdown().document().source(), text);
}

#[test]
fn a_large_edit_builds_its_text_once_and_the_task_and_document_share_it() {
    let _serial = serial();
    let mut doc = Open::new(&synthetic::source(2_000));
    let at = doc.markdown().document().block(1_000).unwrap().span().start;
    let shown = doc.markdown().document().source().as_ptr();
    let pasted = big(300);
    doc.change(|m| m.edit(at..at, &pasted));
    let logical = doc.markdown().source().as_ptr();
    assert_ne!(logical, shown, "the new text is a new allocation");
    assert_eq!(
        doc.markdown().document().source().as_ptr(),
        shown,
        "the document shown keeps its own"
    );
    run_until_installed(&mut doc);
    assert_eq!(doc.markdown().document().source().as_ptr(), logical);
    doc.assert_full_parse();
}

#[test]
fn a_change_while_the_task_holds_the_text_copies_it_once_and_loses_nothing() {
    let _serial = serial();
    let mut doc = Open::new("");
    let text = big(400);
    let owned = text.clone();
    let held = owned.as_ptr();
    doc.change(|m| m.set_source_owned(owned));
    // The task takes its handle in the next frame.
    doc.app.update();
    assert_eq!(doc.markdown().source().as_ptr(), held);
    let mut expected = text;
    doc.change(|m| m.push_str("one "));
    expected.push_str("one ");
    // The task keeps the text it was given, so this side has a copy now...
    let mine = doc.markdown().source().as_ptr();
    assert_ne!(mine, held, "written while shared: copied");
    assert_eq!(doc.markdown().source(), expected);
    // ...which later changes edit in place: the copy has room to grow.
    for k in 0..40 {
        let chunk = format!("more {k} ");
        doc.change(|m| m.push_str(&chunk));
        expected.push_str(&chunk);
        assert_eq!(doc.markdown().source().as_ptr(), mine, "chunk {k}");
    }
    assert_eq!(doc.markdown().source(), expected);
    // The document being parsed is not the one that was edited.
    assert_eq!(doc.markdown().document().source(), "");
    settle(&mut doc.app);
    assert_eq!(doc.markdown().document().source(), expected);
    doc.assert_full_parse();
}

#[test]
fn a_change_made_before_the_task_starts_edits_the_text_in_place() {
    let _serial = serial();
    let mut doc = Open::new("");
    let text = big(400);
    // With room to grow, as a caller that builds the text would have it.
    let mut owned = String::with_capacity(text.len() + 64);
    owned.push_str(&text);
    let at = owned.as_ptr();
    doc.change(|m| m.set_source_owned(owned));
    // No frame has run: nobody else holds the allocation.
    doc.change(|m| m.push_str("a small tail\n"));
    let mut expected = text;
    expected.push_str("a small tail\n");
    assert_eq!(doc.markdown().source(), expected);
    assert_eq!(doc.markdown().source().as_ptr(), at, "not copied");
    settle(&mut doc.app);
    assert_eq!(doc.markdown().document().source(), expected);
}

#[test]
fn changes_queued_during_a_parse_are_applied_to_the_installed_text_in_place() {
    let _serial = serial();
    let mut doc = Open::new("");
    let text = big(400);
    doc.change(|m| m.set_source_owned(text.clone()));
    doc.app.update();
    doc.change(|m| m.push_str("queued "));
    doc.change(|m| m.edit(0..0, "# first\n\n"));
    let mut expected = format!("# first\n\n{text}");
    expected.push_str("queued ");
    assert_eq!(doc.markdown().source(), expected);
    // What lands is the text the task was given...
    doc.markdown().document(); // (still the empty one)
    run_until_installed(&mut doc);
    settle(&mut doc.app);
    // ...with the queue on it: the same text as the logical source had.
    assert_eq!(doc.markdown().document().source(), expected);
    doc.assert_full_parse();
}

#[test]
fn an_install_keeps_the_heights_measured_before_it() {
    let _serial = serial();
    let mut doc = Open::new(&synthetic::source(2_000));
    // Everything on screen has been laid out and measured by now.
    let before: Vec<f64> = (0..30).map(|i| doc.markdown().block_top(i)).collect();
    let live: Vec<usize> = doc.markdown().live_blocks().map(|(i, _)| i).collect();
    assert!(live.contains(&29), "the first screen is live and measured");
    let total_before = doc.markdown().content_height();
    let at = doc.markdown().document().block(1_000).unwrap().span().start;
    let pasted = big(300);
    doc.change(|m| m.edit(at..at, &pasted));
    settle(&mut doc.app);
    let after: Vec<f64> = (0..30).map(|i| doc.markdown().block_top(i)).collect();
    assert_eq!(
        before, after,
        "the blocks before the paste were not touched"
    );
    assert!(
        doc.markdown().content_height() > total_before + 300.0 * 10.0,
        "the pasted blocks are in the index"
    );
    // The index has a height for every block of the new document, and the
    // tops are increasing all the way down.
    let len = doc.markdown().document().len();
    let mut previous = -1.0;
    for i in (0..len).step_by(len / 200) {
        let top = doc.markdown().block_top(i);
        assert!(top > previous, "block {i} top {top} after {previous}");
        previous = top;
    }
    assert_eq!(
        doc.markdown().block_top(len),
        doc.markdown().content_height(),
        "the end of the last block is the total"
    );
}

/// xorshift64*, so the test needs no dependency and stays reproducible.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    }
}

#[test]
fn any_mix_of_large_and_small_changes_ends_at_a_full_parse_of_the_final_source() {
    let _serial = serial();
    let seeds: u64 = std::env::var("BEVY_MD_SEEDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .map_or(12, |n: u64| n.min(40));
    for seed in 1..=seeds {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut doc = Open::new("# Start\n\nText.\n");
        let mut model = doc.markdown().source().to_owned();
        let mut log: Vec<String> = Vec::new();
        for _ in 0..25 {
            let op = rng.below(6);
            log.push(format!(
                "op {op} (model {} bytes, parsing: {})",
                model.len(),
                doc.markdown().is_parsing()
            ));
            match op {
                0 => {
                    let text = big(60 + rng.below(100));
                    model.clone_from(&text);
                    doc.change(|m| m.set_source(&text));
                }
                1 => {
                    let text = format!("replaced {}\n", rng.below(1000));
                    model.clone_from(&text);
                    doc.change(|m| m.set_source(&text));
                }
                2 => {
                    let text = format!("more {} ", rng.below(1000));
                    model.push_str(&text);
                    doc.change(|m| m.push_str(&text));
                }
                3 => {
                    let mut at = rng.below(model.len() + 1);
                    while !model.is_char_boundary(at) {
                        at -= 1;
                    }
                    let text = if rng.below(3) == 0 {
                        synthetic::source(40)
                    } else {
                        "\n\n- item\n\n".to_owned()
                    };
                    model.insert_str(at, &text);
                    doc.change(|m| m.edit(at..at, &text));
                }
                4 => {
                    let mut at = rng.below(model.len() + 1);
                    while !model.is_char_boundary(at) {
                        at -= 1;
                    }
                    let mut end = (at + rng.below(30)).min(model.len());
                    while !model.is_char_boundary(end) {
                        end -= 1;
                    }
                    model.replace_range(at..end, "");
                    doc.change(|m| m.edit(at..end, ""));
                }
                _ => {
                    for _ in 0..=rng.below(4) {
                        doc.app.update();
                    }
                }
            }
            if doc.markdown().source() != model {
                let source = doc.markdown().source();
                let at = source
                    .bytes()
                    .zip(model.bytes())
                    .take_while(|(a, b)| a == b)
                    .count();
                panic!(
                    "seed {seed}: source {} bytes, model {} bytes, differ at {at}\nlast ops: {:#?}",
                    source.len(),
                    model.len(),
                    &log[log.len().saturating_sub(4)..]
                );
            }
        }
        settle(&mut doc.app);
        assert_eq!(doc.markdown().document().source(), model, "seed {seed}");
        doc.assert_full_parse();
    }
}
