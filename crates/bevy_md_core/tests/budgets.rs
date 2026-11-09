//! Budgets of the document model, counted exactly: what an edit costs must
//! not depend on how large the document is (see `perf-budgets.json`).
//!
//! Time is measured by the `keystroke_in_1MB` benchmark; here only
//! allocations and the blocks an edit replaces are counted, because those do
//! not change with the load on the machine.

use std::path::PathBuf;

use bevy_md_core::{Document, synthetic};

fn limit(name: &str) -> u64 {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../perf-budgets.json");
    let text = std::fs::read_to_string(path).expect("perf-budgets.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    let entry = &json["budgets"][name];
    assert!(
        entry["measured"].is_number() && entry["why"].as_str().is_some_and(|w| !w.is_empty()),
        "budget {name} needs `measured` and `why` in perf-budgets.json"
    );
    entry["limit"]
        .as_u64()
        .unwrap_or_else(|| panic!("budget {name} has no `limit`"))
}

#[track_caller]
fn check(name: &str, value: usize) {
    let value = value as u64;
    if std::env::var_os("BEVY_MD_PRINT_BUDGETS").is_some() {
        println!("BUDGET {name} = {value}");
        return;
    }
    let limit = limit(name);
    assert!(
        value <= limit,
        "budget `{name}` exceeded: measured {value}, limit {limit} (perf-budgets.json)"
    );
}

/// What one keystroke in the middle of a document of `blocks` blocks costs.
struct Keystroke {
    allocations: usize,
    bytes: usize,
    replaced: usize,
}

fn keystroke(blocks: usize) -> Keystroke {
    let mut doc = synthetic::document(blocks);
    let middle = doc.block(doc.len() / 2).expect("a block").span().start + 3;
    // The first edit grows the source's buffer once; later ones reuse it.
    doc.edit(middle..middle, "y");
    let mut splices = None;
    let info = allocation_counter::measure(|| {
        splices = Some(doc.edit(middle..middle, "x"));
    });
    let splices = splices.expect("the edit ran");
    Keystroke {
        allocations: usize::try_from(info.count_total).expect("fits"),
        bytes: usize::try_from(info.bytes_total).expect("fits"),
        replaced: splices.removed().max(splices.inserted()),
    }
}

#[test]
fn a_keystroke_costs_the_same_in_any_document() {
    let small = keystroke(2_000);
    let big = keystroke(100_000);
    check("core_edit_allocations_2000", small.allocations);
    check("core_edit_allocations_100000", big.allocations);
    check("core_edit_bytes_100000", big.bytes);
    check(
        "core_edit_replaced_blocks",
        big.replaced.max(small.replaced),
    );
    // Independent of the absolute limits: 50 times the blocks may not cost
    // more than a few allocations extra (tree depth, not tree size).
    assert!(
        big.allocations <= small.allocations + 8,
        "edit allocations grow with the document: {} at 2000 blocks, {} at 100000",
        small.allocations,
        big.allocations
    );
}

#[test]
fn appending_a_chunk_costs_the_same_in_any_document() {
    let append = |blocks: usize| {
        let mut doc = synthetic::document(blocks);
        doc.push_str("one ");
        let info = allocation_counter::measure(|| {
            doc.push_str("more ");
        });
        usize::try_from(info.count_total).expect("fits")
    };
    let small = append(2_000);
    let big = append(100_000);
    check("core_append_allocations_2000", small);
    check("core_append_allocations_100000", big);
    assert!(
        big <= small + 8,
        "append allocations grow: {small} vs {big}"
    );
}

#[test]
fn block_lookup_does_not_allocate() {
    let doc: Document = synthetic::document(100_000);
    let at = doc.block(77_777).expect("a block").span().start + 1;
    let (mut found, mut block) = (None, None);
    let info = allocation_counter::measure(|| {
        found = doc.block_at(at);
        block = doc.block(77_777).map(|b| b.span());
    });
    assert_eq!(found, Some(77_777));
    assert!(block.is_some());
    check(
        "core_lookup_allocations",
        usize::try_from(info.count_total).expect("fits"),
    );
}

/// What one edit costs in a document full of references, definitions and
/// footnotes (`synthetic::references_source`).
struct Referenced {
    allocations: usize,
    bytes: usize,
    parsed: u64,
    refreshed: u64,
    replaced: usize,
}

/// Where `needle` first occurs after the middle of the source.
fn after_the_middle(doc: &Document, needle: &str) -> usize {
    let middle = doc.source().len() / 2;
    middle + doc.source()[middle..].find(needle).expect("the needle")
}

/// Runs `edit` (a range and its replacement, found in the document) in a
/// document of `copies` copies of the sample and counts what it costs.
fn referenced(
    copies: usize,
    edit: impl Fn(&Document) -> (std::ops::Range<usize>, String),
) -> Referenced {
    let mut doc = synthetic::references_document(copies);
    // A first edit grows the source's buffer once; later ones reuse it.
    let (warm_up, _) = edit(&doc);
    doc.edit(warm_up.start..warm_up.start, "y");
    let (range, text) = edit(&doc);
    let before = doc.stats();
    let mut splices = None;
    let info = allocation_counter::measure(|| {
        splices = Some(doc.edit(range.clone(), &text));
    });
    let splices = splices.expect("the edit ran");
    let after = doc.stats();
    assert_eq!(
        after.full_parses, before.full_parses,
        "the edit parsed the whole document"
    );
    assert_eq!(
        after.definition_fallbacks, before.definition_fallbacks,
        "the edit fell back to a full parse"
    );
    Referenced {
        allocations: usize::try_from(info.count_total).expect("fits"),
        bytes: usize::try_from(info.bytes_total).expect("fits"),
        parsed: after.parsed_bytes - before.parsed_bytes,
        refreshed: after.refreshes - before.refreshes,
        replaced: splices.removed().max(splices.inserted()),
    }
}

type Edit = fn(&Document) -> (std::ops::Range<usize>, String);

/// Typing in a paragraph that mentions a reference.
fn type_in_a_reference_paragraph(doc: &Document) -> (std::ops::Range<usize>, String) {
    let at = after_the_middle(doc, "Read [the map") + 6;
    (at..at, "x".into())
}

/// Changing where a definition points: its dependents are reparsed.
fn retarget_a_definition(doc: &Document) -> (std::ops::Range<usize>, String) {
    let at = after_the_middle(doc, "]: /s/") + 6;
    (at..at, "x".into())
}

/// A definition of a label nobody else defines, between two blocks.
fn add_a_definition(doc: &Document) -> (std::ops::Range<usize>, String) {
    let at = after_the_middle(doc, "\n\n[^n");
    (at..at, "\n[fresh label]: /fresh".into())
}

/// Deleting a definition: the references to it turn to text.
fn remove_a_definition(doc: &Document) -> (std::ops::Range<usize>, String) {
    let at = after_the_middle(doc, "[map");
    let at = at + doc.source()[at..].find("\n[map").expect("a definition") + 1;
    let end = at + doc.source()[at..].find('\n').expect("its end") + 1;
    (at..end, String::new())
}

/// Editing a footnote definition.
fn edit_a_footnote(doc: &Document) -> (std::ops::Range<usize>, String) {
    let at = after_the_middle(doc, "]: The note") + 4;
    (at..at, "x".into())
}

#[test]
fn an_edit_around_references_costs_the_same_in_any_document() {
    let cases: [(&str, Edit); 5] = [
        ("type", type_in_a_reference_paragraph),
        ("retarget", retarget_a_definition),
        ("add", add_a_definition),
        ("remove", remove_a_definition),
        ("footnote", edit_a_footnote),
    ];
    for (name, edit) in cases {
        let small = referenced(200, edit);
        let big = referenced(5_000, edit);
        println!(
            "{name}: {} allocations, {} bytes allocated, {} bytes parsed, {} refreshed, {} replaced at 5000 copies ({} allocations, {} parsed at 200)",
            big.allocations,
            big.bytes,
            big.parsed,
            big.refreshed,
            big.replaced,
            small.allocations,
            small.parsed
        );
        check(
            &format!("core_reference_edit_allocations_{name}"),
            big.allocations,
        );
        check(
            &format!("core_reference_edit_parsed_bytes_{name}"),
            big.parsed as usize,
        );
        check(
            &format!("core_reference_edit_replaced_{name}"),
            big.replaced,
        );
        // The same edit in a document 25 times larger: not more bytes parsed
        // or blocks replaced, and at most a few allocations more (the depth
        // of the trees).
        // (Labels and offsets have more digits in the larger document.)
        assert!(
            big.parsed <= small.parsed + small.parsed / 8 + 32,
            "{name}: the parse grows with the document: {} bytes at 200 copies, {} at 5000",
            small.parsed,
            big.parsed
        );
        assert_eq!(
            big.replaced, small.replaced,
            "{name}: the splice grows with the document"
        );
        assert_eq!(
            big.refreshed, small.refreshed,
            "{name}: refreshes grow with the document"
        );
        assert!(
            big.allocations <= small.allocations + 24,
            "{name}: allocations grow with the document: {} at 200 copies, {} at 5000",
            small.allocations,
            big.allocations
        );
    }
}

/// What one edit of a definition costs in a note whose three mentions of it
/// are `filler` paragraphs apart (see `synthetic::spread_references_source`).
struct Far {
    allocations: usize,
    bytes: usize,
    parsed: u64,
    splices: usize,
    replaced: usize,
    inserted: usize,
}

fn far(
    filler: usize,
    edit: fn(&Document) -> (std::ops::Range<usize>, String),
    defined: bool,
) -> Far {
    let mut doc = synthetic::spread_references_document(filler);
    if defined {
        let end = doc.source().len();
        doc.edit(end..end, &format!("\n{}", synthetic::SHARED_DEFINITION));
    }
    // A first edit grows the source's buffer once; later ones reuse it.
    let warm_up = doc.source().len() / 2;
    doc.edit(warm_up..warm_up, "y");
    let (range, text) = edit(&doc);
    let before = doc.stats();
    let mut splices = None;
    let info = allocation_counter::measure(|| {
        splices = Some(doc.edit(range.clone(), &text));
    });
    let splices = splices.expect("the edit ran");
    let after = doc.stats();
    assert_eq!(after.full_parses, before.full_parses, "a full parse");
    assert_eq!(after.refreshes - before.refreshes, 3, "one per mention");
    Far {
        allocations: usize::try_from(info.count_total).expect("fits"),
        bytes: usize::try_from(info.bytes_total).expect("fits"),
        parsed: after.parsed_bytes - before.parsed_bytes,
        splices: splices.len(),
        replaced: splices.removed(),
        inserted: splices.inserted(),
    }
}

fn add_the_definition_at_the_end(doc: &Document) -> (std::ops::Range<usize>, String) {
    let end = doc.source().len();
    (end..end, format!("\n{}", synthetic::SHARED_DEFINITION))
}

fn retarget_the_definition(doc: &Document) -> (std::ops::Range<usize>, String) {
    let at = doc.source().find("/dest").expect("the definition") + 1;
    (at..at, "x".into())
}

fn remove_the_definition(doc: &Document) -> (std::ops::Range<usize>, String) {
    let at = doc.source().find("[shared]:").expect("the definition");
    (at..doc.source().len(), String::new())
}

/// The list of splices an edit of a far definition reports is as long as the
/// mentions it reaches and as large as them, at 200, 2000 and 120 000
/// blocks: what is between the mentions is not in it, costs nothing to
/// report and nothing to parse. (Before M9's follow-up the one range covered
/// 119 978 blocks at the largest size.)
#[test]
fn a_far_definition_edit_reports_and_costs_the_same_in_any_document() {
    type Case = (&'static str, Edit, bool);
    let cases: [Case; 3] = [
        ("add", add_the_definition_at_the_end, false),
        ("retarget", retarget_the_definition, true),
        ("remove", remove_the_definition, true),
    ];
    for (name, edit, defined) in cases {
        let (small, medium, big) = (
            far(70, edit, defined),
            far(700, edit, defined),
            far(40_000, edit, defined),
        );
        println!(
            "{name}: {} allocations, {} bytes, {} bytes parsed, {} splices, {} replaced, {} inserted at 120003 blocks ({} allocations at 213, {} at 2103)",
            big.allocations,
            big.bytes,
            big.parsed,
            big.splices,
            big.replaced,
            big.inserted,
            small.allocations,
            medium.allocations
        );
        check(
            &format!("core_far_definition_allocations_{name}"),
            big.allocations,
        );
        check(&format!("core_far_definition_bytes_{name}"), big.bytes);
        check(
            &format!("core_far_definition_parsed_bytes_{name}"),
            big.parsed as usize,
        );
        check(&format!("core_far_definition_splices_{name}"), big.splices);
        check(
            &format!("core_far_definition_replaced_blocks_{name}"),
            big.replaced.max(big.inserted),
        );
        for other in [&small, &medium] {
            assert_eq!(
                big.splices, other.splices,
                "{name}: the list grows with the document"
            );
            assert_eq!(big.replaced, other.replaced, "{name}: blocks replaced grow");
            assert_eq!(big.inserted, other.inserted, "{name}: blocks inserted grow");
            assert!(
                big.parsed <= other.parsed + other.parsed / 8 + 32,
                "{name}: the parse grows with the document: {} vs {}",
                other.parsed,
                big.parsed
            );
            assert!(
                big.allocations <= other.allocations + 24,
                "{name}: allocations grow with the document: {} vs {}",
                other.allocations,
                big.allocations
            );
        }
    }
}

/// A definition at the end, the paragraph that uses it at the start: only
/// the two places are parsed, never the document.
///
/// The report is local too: a [`bevy_md_core::Splices`] with one entry, the
/// user (see `a_far_definition_edit_reports_and_costs_the_same_in_any_document`).
#[test]
fn a_far_definition_parses_two_windows_not_the_document() {
    let mut doc = synthetic::references_document(2_000);
    let at = doc
        .source()
        .find("https://example.com/far/0")
        .expect("the first far definition")
        + 25;
    let before = doc.stats();
    let splices = doc.edit(at..at, "x");
    let after = doc.stats();
    assert_eq!(splices.len(), 1, "only the user is reported: {splices:?}");
    assert_eq!(splices.removed(), 1);
    assert_eq!(splices.inserted(), 1);
    assert_eq!(after.full_parses, before.full_parses);
    assert_eq!(
        after.window_parses - before.window_parses,
        2,
        "the edit and the one user"
    );
    assert_eq!(after.refreshes - before.refreshes, 1);
    assert!(
        doc.block(0).unwrap().content.links.is_empty(),
        "the first heading uses no reference"
    );
    let user = (0..doc.len())
        .find(|&i| {
            doc.block(i)
                .unwrap()
                .content
                .text
                .starts_with("Read the map 0")
        })
        .expect("the paragraph that uses far0");
    assert!(
        doc.block(user)
            .unwrap()
            .content
            .links
            .iter()
            .any(|l| &**l == "https://example.com/far/0x"),
        "the user of the definition shows the new destination"
    );
}
