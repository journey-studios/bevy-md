//! Link reference and footnote definitions: the index must keep every edit
//! equal to a full parse, and keep the edit a window reparse.

mod support;

use std::ops::Range;

use bevy_md_core::{Definition, Document, Fate, Splices, synthetic};

const SAMPLE: &str = include_str!("fixtures/sample.md");

/// Tokens that change what a definition or a reference means. Labels repeat
/// on purpose, so definitions collide.
const TOKENS: &[&str] = &[
    "[a]: /u\n",
    "\n[b]: /v \"t\"\n",
    "[^1]: note\n",
    "[^1]",
    "[a]",
    "[a][b]",
    "![i][a]",
    "[a][]",
    "[b]",
    "]",
    "[",
    ":",
    "[^",
    "\n\n",
    "\n",
    "> ",
    "- ",
    "```\n",
    "    ",
    "|",
    "[A ]: /w\n",
    "[Ação]: /x\n",
    "[ação]",
    "[^é]: n\n",
    "[^é]",
    "[a]:\n/u2\n",
    "[a]:\n",
    "[f\ng]: /m\n",
    "<div>\n",
    "# ",
    "\\",
    "---",
    "x",
    "日本",
];

/// Tokens that never repeat a label, so the incremental path is the one
/// exercised (a repeated label is the documented full-parse case).
fn fresh_tokens(n: u64) -> Vec<String> {
    vec![
        format!("[k{n}]: /u{n}\n"),
        format!("\n[k{n}]: /v{n} \"t\"\n"),
        format!("[^k{n}]: note {n}\n"),
        format!("[^k{n}]"),
        format!("[k{n}]"),
        format!("[text][k{n}]"),
        format!("![img][k{n}]"),
        format!("[k{n}][]"),
        // Mentions of labels the document defines from the start.
        "[map3]".into(),
        "[x][map7]".into(),
        "[^n2]".into(),
        "[shortcut4]".into(),
        "\n\n".into(),
        "\n".into(),
        "> ".into(),
        "- ".into(),
        "]".into(),
        "[".into(),
        ":".into(),
        "x".into(),
    ]
}

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

#[track_caller]
fn assert_matches_full_parse(doc: &Document, context: &str) {
    doc.assert_consistent();
    let full = Document::parse(doc.source());
    if !doc.blocks().eq(full.blocks()) {
        let (a, b): (Vec<_>, Vec<_>) = (doc.blocks().collect(), full.blocks().collect());
        let first = (0..a.len().max(b.len()))
            .find(|&i| a.get(i) != b.get(i))
            .unwrap_or(0);
        panic!(
            "incremental blocks differ from a full parse after {context}\nfirst difference at block {first}:\nincremental: {:?}\nfull:        {:?}\nsource:\n{}",
            a.get(first),
            b.get(first),
            doc.source()
        );
    }
    assert_eq!(
        doc.definitions().collect::<Vec<_>>(),
        full.definitions().collect::<Vec<_>>(),
        "definitions differ from a full parse after {context}\nsource:\n{}",
        doc.source()
    );
    assert_uses_cover_a_full_parse(doc, &full, context);
    let (mine, theirs) = (
        doc.definition_index_snapshot(),
        full.definition_index_snapshot(),
    );
    if mine != theirs {
        let split = |text: &str| -> Vec<String> {
            text.replace(", (", ",\n(")
                .lines()
                .map(str::to_owned)
                .collect()
        };
        let (a, b) = (split(&mine), split(&theirs));
        let first = (0..a.len().max(b.len()))
            .find(|&i| a.get(i) != b.get(i))
            .unwrap_or(0);
        panic!(
            "definition index differs from a full parse after {context}\nfirst difference at entry {first}:\nincremental: {:?}\nfull:        {:?}\nnext incremental: {:?}\nnext full: {:?}\nsource:\n{}",
            a.get(first),
            b.get(first),
            a.get(first + 1),
            b.get(first + 1),
            doc.source()
        );
    }
}

/// Every mention a full parse knows of, the edited document knows of too.
fn assert_uses_cover_a_full_parse(doc: &Document, full: &Document, context: &str) {
    let mine = doc.reference_uses_snapshot();
    let missing: Vec<_> = full
        .reference_uses_snapshot()
        .into_iter()
        .filter(|entry| !mine.contains(entry))
        .collect();
    assert!(
        missing.is_empty(),
        "mentions a full parse finds are missing after {context}: {missing:?}"
    );
}

fn seeds(default: u64) -> u64 {
    std::env::var("BEVY_MD_SEEDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

fn random_edit(doc: &mut Document, rng: &mut Rng, token: &str, context: &str) {
    let len = doc.source().len();
    let start = char_boundary(doc.source(), rng.below(len + 1));
    let end = if rng.below(3) == 0 {
        char_boundary(doc.source(), (start + rng.below(14)).min(len))
    } else {
        start
    };
    let text = if rng.below(4) == 0 { "" } else { token };
    let before = support::snapshot(doc);
    let prior = doc.source().to_owned();
    let splices = doc.edit(start..end, text);
    support::assert_explains(
        &before,
        doc,
        &splices,
        Some(text.len().cast_signed() - (end - start).cast_signed()),
        &format!("{context}: {start}..{end} -> {text:?}"),
    );
    assert_matches_full_parse(
        doc,
        &format!(
            "{context}: {start}..{end} -> {text:?}\nsource before the edit:\n{prior}\n--- end of source before"
        ),
    );
}

/// Definitions that collide, and the text around them, edited at random.
#[test]
fn random_edits_with_colliding_labels_match_full_parse() {
    for seed in 1..=seeds(40) {
        let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let start = format!(
            "{SAMPLE}\nSee [the map][a], [b] and a note[^1].\n\n[a]: /first\n\n[^1]: The note.\n\n[B]: /second\n"
        );
        let mut doc = Document::parse(start);
        for step in 0..120 {
            let token = TOKENS[rng.below(TOKENS.len())];
            random_edit(
                &mut doc,
                &mut rng,
                token,
                &format!("seed {seed} step {step}"),
            );
        }
    }
}

/// A document with many definitions and references, edited with labels that
/// never collide: the window path, not the fallback.
#[test]
fn random_edits_with_fresh_labels_match_full_parse() {
    for seed in 1..=seeds(40) {
        let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03));
        let mut doc = synthetic::references_document(12);
        for step in 0..120 {
            let tokens = fresh_tokens(seed * 1000 + step);
            let token = tokens[rng.below(tokens.len())].clone();
            random_edit(
                &mut doc,
                &mut rng,
                &token,
                &format!("seed {seed} step {step}"),
            );
        }
    }
}

/// Streaming a document whose definitions come last: every prefix equals a
/// full parse, whatever the chunk size.
#[test]
fn streaming_a_document_with_definitions_matches_full_parse() {
    let source = format!(
        "# Notes\n\nSee [the map][map], the [site] and a note[^1] or [^2].\n\n> Quoted [map] and ![pic][map].\n\n- item [site]\n- item[^1]\n\n[map]: https://example.com/map \"Map\"\n[site]: <https://example.com/a b>\n\n[^1]: First note,\n    continued.\n\n[^2]: Second note.\n\nTail [Ação][ação].\n\n[AÇÃO]: /acao\n{SAMPLE}"
    );
    for chunk in [1, 2, 5, 17, 64] {
        let mut doc = Document::new();
        let mut at = 0;
        while at < source.len() {
            let end = char_boundary(&source, (at + chunk).min(source.len()));
            let end = if end == at {
                (at + chunk).min(source.len())
            } else {
                end
            };
            let end = (end..=source.len())
                .find(|&e| source.is_char_boundary(e))
                .unwrap_or(source.len());
            doc.push_str(&source[at..end]);
            assert_matches_full_parse(&doc, &format!("chunk {chunk} at {at}"));
            at = end;
        }
    }
}

fn link_of(doc: &Document, block: usize) -> Vec<String> {
    doc.block(block)
        .expect("a block")
        .content
        .links
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn adding_changing_and_removing_a_definition_updates_its_references() {
    let mut doc = Document::parse("See [the map][map] and [map].\n\nMiddle text.\n\nEnd text.\n");
    assert!(link_of(&doc, 0).is_empty(), "no definition yet");
    assert_eq!(doc.link_definition("map"), None);

    // Add one at the end: references before it become links.
    let end = doc.source().len();
    let splices = doc.edit(end..end, "\n[map]: https://example.com/a\n");
    assert_matches_full_parse(&doc, "adding");
    assert_eq!(
        link_of(&doc, 0),
        ["https://example.com/a", "https://example.com/a"]
    );
    assert_eq!(doc.link_definition("MAP"), Some("https://example.com/a"));
    assert_eq!(
        splices.as_slice().first().map(|s| s.old.start),
        Some(0),
        "the reference block was refreshed: {splices:?}"
    );
    assert_eq!(doc.stats().full_parses, 1, "only the first parse was full");

    // Change where it points.
    let at = doc.source().find("/a").expect("the destination");
    doc.edit(at + 1..at + 2, "b");
    assert_matches_full_parse(&doc, "changing");
    assert_eq!(link_of(&doc, 0), ["https://example.com/b"; 2]);
    assert_eq!(doc.stats().full_parses, 1);

    // Remove it: the references fall back to text.
    let start = doc.source().find("[map]:").expect("the definition");
    let end = doc.source().len();
    doc.edit(start..end, "");
    assert_matches_full_parse(&doc, "removing");
    assert_eq!(link_of(&doc, 0), Vec::<String>::new());
    assert_eq!(doc.link_definition("map"), None);
    assert_eq!(doc.stats().full_parses, 1);
}

#[test]
fn labels_match_without_regard_to_case_or_whitespace_runs() {
    let mut doc = Document::parse("A [Foo   Bar] and [x][FOO\tBAR].\n\n[foo bar]: /fb\n");
    assert_matches_full_parse(&doc, "start");
    assert_eq!(link_of(&doc, 0), ["/fb", "/fb"]);
    assert_eq!(doc.link_definition("FOO BAR"), Some("/fb"));
    assert_eq!(doc.link_definition("  Foo \n bar "), Some("/fb"));
    // Unicode case folding, and offsets past multi-byte characters.
    doc.edit(0..0, "Ação [ação] 日本.\n\n[AÇÃO]: /acao\n\n");
    assert_matches_full_parse(&doc, "unicode label");
    assert_eq!(doc.link_definition("ação"), Some("/acao"));
    let at = doc.source().find("日本").expect("text") + "日".len();
    doc.edit(at..at, " é");
    assert_matches_full_parse(&doc, "edit past multi-byte text");
}

#[test]
fn footnote_references_follow_their_definitions() {
    let mut doc = Document::parse("Claim[^x] and more[^y].\n\nOther text.\n");
    assert_eq!(
        doc.block(0).unwrap().content.text,
        "Claim[^x] and more[^y]."
    );
    assert!(!doc.has_footnote_definition("x"));
    let end = doc.source().len();
    doc.edit(end..end, "\n[^x]: Evidence.\n");
    assert_matches_full_parse(&doc, "adding a footnote");
    assert!(doc.has_footnote_definition("X"));
    assert_eq!(doc.block(0).unwrap().content.text, "Claim[x] and more[^y].");
    assert_eq!(doc.stats().full_parses, 1);
    // Rename the definition: the reference falls back to text.
    let at = doc.source().find("[^x]:").unwrap() + 2;
    doc.edit(at..at + 1, "z");
    assert_matches_full_parse(&doc, "renaming a footnote");
    assert_eq!(
        doc.block(0).unwrap().content.text,
        "Claim[^x] and more[^y]."
    );
    assert!(doc.has_footnote_definition("z"));
    assert_eq!(doc.stats().full_parses, 1);
}

#[test]
fn a_label_defined_twice_falls_back_and_the_first_wins() {
    let filler = "Filler paragraph.\n\n".repeat(40);
    let mut doc = Document::parse(format!(
        "A [dup] link.\n\n{filler}[dup]: /first\n\n{filler}[dup]: /second\n\n{filler}"
    ));
    assert_matches_full_parse(&doc, "start");
    assert_eq!(link_of(&doc, 0), ["/first"]);
    // Removing the winner reveals the loser the parser had hidden.
    let at = doc.source().find("[dup]: /first").unwrap();
    doc.edit(at..at + "[dup]: /first\n\n".len(), "");
    assert_matches_full_parse(&doc, "removing the winner");
    assert_eq!(link_of(&doc, 0), ["/second"]);
    // Adding a second definition elsewhere: the earlier one still wins.
    let end = doc.source().len();
    doc.edit(end..end, "\n[dup]: /third\n");
    assert_matches_full_parse(&doc, "a third definition");
    assert_eq!(link_of(&doc, 0), ["/second"]);
    assert!(doc.stats().definition_fallbacks >= 1, "{:?}", doc.stats());
}

#[test]
fn definitions_in_containers_and_after_references_resolve_everywhere() {
    let source = "> [q]: /quoted\n\n- item [q] and [l]\n\n  [l]: /listed\n\nPlain [q][L].\n";
    let mut doc = Document::parse(source);
    assert_matches_full_parse(&doc, "start");
    assert_eq!(doc.link_definition("q"), Some("/quoted"));
    assert_eq!(doc.link_definition("l"), Some("/listed"));
    let at = doc.source().find("/quoted").unwrap();
    doc.edit(at..at + 7, "/changed");
    assert_matches_full_parse(&doc, "changing a quoted definition");
    // `[q][L]` is the text `q` linked to label `L`, which is `l`.
    let last = doc.len() - 1;
    assert_eq!(link_of(&doc, last), ["/listed"]);
    let item = (0..doc.len())
        .find(|&i| doc.block(i).unwrap().content.text.starts_with("item"))
        .expect("the list item");
    assert_eq!(link_of(&doc, item), ["/changed", "/listed"]);
}

#[test]
fn unresolved_references_stay_text() {
    let mut doc = Document::parse("[nothing] and [x][y] and [^z] and [a]: not a def\n");
    assert_matches_full_parse(&doc, "start");
    assert_eq!(link_of(&doc, 0), Vec::<String>::new());
    assert_eq!(doc.definitions().count(), 0);
    doc.edit(0..0, "[y]: /y\n\n");
    assert_matches_full_parse(&doc, "defining y");
    assert_eq!(link_of(&doc, 0), ["/y"]);
}

#[test]
fn the_definitions_api_lists_what_the_parser_reads() {
    let doc = Document::parse("[a]: /a\n[A]: /hidden\n\n[^n]: note\n\nText [a][^n].\n");
    let list: Vec<_> = doc.definitions().map(|(_, d)| d).collect();
    assert_eq!(
        list,
        [
            Definition::Link {
                label: "a",
                destination: "/a"
            },
            Definition::Footnote { label: "n" },
        ]
    );
}

/// The three blocks of a [`synthetic::spread_references_document`] that
/// mention `[shared]`.
fn mentions(filler: usize) -> [usize; 3] {
    [0, filler + 1, 2 * filler + 2]
}

fn is_link(doc: &Document, block: usize) -> bool {
    !doc.block(block).unwrap().content.links.is_empty()
}

/// The ranges of every splice, as `(old, new)` pairs.
fn pairs(splices: &Splices) -> Vec<(Range<usize>, Range<usize>)> {
    splices
        .iter()
        .map(|s| (s.old.clone(), s.new.clone()))
        .collect()
}

/// What an edit of a definition reports in a note of `3 * filler + 3`
/// blocks: one splice per mention, none for the blocks between, whatever the
/// distance (the sizes below go from 200 to 120 000 blocks), checked against
/// the full parse and against the unchanged blocks.
fn far_definition_splices(filler: usize) {
    let [top, middle, bottom] = mentions(filler);
    let mut doc = synthetic::spread_references_document(filler);
    let context = |what: &str| format!("{what} with {filler} filler paragraphs");

    // Adding the definition at the very end.
    let before = support::snapshot(&doc);
    let stats = doc.stats();
    let end = doc.source().len();
    let splices = doc.edit(end..end, &format!("\n{}", synthetic::SHARED_DEFINITION));
    support::assert_explains(&before, &doc, &splices, None, &context("adding"));
    let one = |at: usize| (at..at + 1, at..at + 1);
    assert_eq!(
        pairs(&splices),
        [top, middle, bottom].map(one),
        "{}: {splices:?}",
        context("adding")
    );
    assert!([top, middle, bottom].iter().all(|&at| is_link(&doc, at)));
    let after = doc.stats();
    assert_eq!(after.full_parses, stats.full_parses, "no full parse");
    assert_eq!(after.refreshes - stats.refreshes, 3, "one per mention");
    assert_eq!(after.window_parses - stats.window_parses, 4);
    assert_eq!(splices.removed(), 3);
    assert_eq!(splices.inserted(), 3);

    // Pointing it elsewhere.
    let before = support::snapshot(&doc);
    let at = doc.source().find("/dest").unwrap() + 1;
    let splices = doc.edit(at..at, "x");
    support::assert_explains(&before, &doc, &splices, None, &context("retargeting"));
    assert_eq!(splices.len(), 3, "{}: {splices:?}", context("retargeting"));
    assert_eq!(splices.removed(), 3);
    assert_eq!(splices.inserted(), 3);
    assert_eq!(&*doc.block(middle).unwrap().content.links[0], "/xdest");

    // Removing it again.
    let before = support::snapshot(&doc);
    let at = doc.source().find("[shared]:").unwrap();
    let end = doc.source().len();
    let splices = doc.edit(at..end, "");
    support::assert_explains(&before, &doc, &splices, None, &context("removing"));
    assert_eq!(splices.len(), 3, "{}: {splices:?}", context("removing"));
    assert!(![top, middle, bottom].iter().any(|&at| is_link(&doc, at)));
    assert_matches_full_parse(&doc, &context("removing"));
}

#[test]
fn a_definition_far_from_its_mentions_reports_one_splice_per_mention() {
    for filler in [70, 700, 40_000] {
        far_definition_splices(filler);
    }
}

/// An edit that adds blocks at the top and defines a label mentioned far
/// below: the splices after the insertion are shifted by it, and the blocks
/// between the mentions are not in any.
#[test]
fn an_edit_that_adds_blocks_shifts_the_splices_after_it() {
    for filler in [70, 700, 20_000] {
        let [top, middle, bottom] = mentions(filler);
        let mut doc = synthetic::spread_references_document(filler);
        let before = support::snapshot(&doc);
        let text = format!("{}\nNew one.\n\nNew two.\n\n", synthetic::SHARED_DEFINITION);
        // (An edit in the first bytes is a full parse, because of front
        // matter; this one lands just after the first paragraph.)
        let at = doc.source().find("Filler paragraph 0 ").unwrap();
        let splices = doc.edit(at..at, &text);
        let context = format!("{filler} filler paragraphs: {splices:?}");
        support::assert_explains(&before, &doc, &splices, None, &context);
        assert_matches_full_parse(&doc, &context);
        // The top mention was reparsed with the pasted text (it is a link
        // now) and two paragraphs came with it; the other two mentions are
        // refreshed where they moved to.
        let got = pairs(&splices);
        assert_eq!(got.len(), 3, "{context}");
        assert_eq!(got[0].0.start, top, "{context}");
        assert!(got[0].0.len() <= 3, "{context}");
        assert_eq!(got[0].1.len(), got[0].0.len() + 2, "{context}");
        assert_eq!(
            got[1..],
            [
                (middle..middle + 1, middle + 2..middle + 3),
                (bottom..bottom + 1, bottom + 2..bottom + 3),
            ],
            "{context}"
        );
        assert_eq!(doc.stats().full_parses, 1);
    }
}

/// Joining two paragraphs (one block fewer), and then deleting a definition
/// elsewhere, which turns three mentions back into text: the second report
/// is three separate splices, shifted back by the block the first one took.
#[test]
fn a_join_and_a_far_refresh_are_separate_reports() {
    let filler = 300;
    let [_, middle, bottom] = mentions(filler);
    let source = format!(
        "{}\n{}",
        synthetic::spread_references_source(filler),
        synthetic::SHARED_DEFINITION
    );
    let mut doc = Document::parse(source);
    assert!(is_link(&doc, bottom));
    let before = support::snapshot(&doc);
    let join_at = doc.source().find("Middle [shared]").unwrap() - 2;
    let joined = doc.edit(join_at..join_at + 2, " ");
    support::assert_explains(&before, &doc, &joined, None, "joining");
    assert_eq!(joined.removed(), 2, "{joined:?}");
    assert_eq!(joined.inserted(), 1);
    assert_eq!(doc.len(), 3 * filler + 2);

    let before = support::snapshot(&doc);
    let at = doc.source().find("[shared]:").unwrap();
    let end = doc.source().len();
    let removed = doc.edit(at..end, "");
    support::assert_explains(&before, &doc, &removed, None, "removing the definition");
    // The blocks after the join sit one place earlier.
    let (middle, bottom) = (middle - 1, bottom - 1);
    assert_eq!(
        pairs(&removed),
        [
            (0..1, 0..1),
            (middle..middle + 1, middle..middle + 1),
            (bottom..bottom + 1, bottom..bottom + 1)
        ]
    );
    assert_matches_full_parse(&doc, "after both");
}

// ---------------------------------------------------------------------------
// The list a background parse reports: `Document::splice_from`
// ---------------------------------------------------------------------------

/// What a renderer showing `old` is told when `new`, parsed from scratch, is
/// installed after one change to the text (a paste, which may hold the
/// definition): the three mentions and the paste, with what lies between
/// them (and after them, shifted) left out.
#[test]
fn a_parse_from_scratch_reports_the_far_splices_of_one_paste() {
    for filler in [70, 700, 20_000] {
        let source = synthetic::spread_references_source(filler);
        let old = Document::parse(source.as_str());
        let snapshot = support::snapshot(&old);
        let blocks = old.len();
        let middle = old.block(filler + 5).unwrap().span().start;
        let pastes: [(&str, usize, String); 3] = [
            // At the end, the definition with some paragraphs after it.
            (
                "end",
                source.len(),
                format!("{}\nMore.\n\nAnd more.\n", synthetic::SHARED_DEFINITION),
            ),
            // Between the first and the middle mention.
            (
                "between",
                middle,
                format!(
                    "Pasted.\n\n{}\nPasted too.\n\n",
                    synthetic::SHARED_DEFINITION
                ),
            ),
            // No definition: only the paste.
            ("plain", middle, "Pasted.\n\nPasted too.\n\n".to_owned()),
        ];
        for (name, at, text) in pastes {
            let mut full = source.clone();
            full.insert_str(at, &text);
            let new = Document::parse(full.as_str());
            let splices = new.splice_from(&old);
            let context = format!("{name}, {blocks} blocks: {splices:?}");
            support::assert_explains(&snapshot, &new, &splices, None, &context);
            // The paste and the mentions, a few blocks each, never the ones
            // between them.
            assert!(splices.len() <= 5, "{context}");
            assert!(
                splices.removed() <= 6 && splices.inserted() <= 9,
                "{context}"
            );
            for kept in [5, filler / 2, filler + 9, 2 * filler] {
                if (name == "plain" && kept == filler + 9) || kept + 1 >= blocks {
                    continue;
                }
                assert!(
                    matches!(splices.locate_old(kept), Fate::Kept(_))
                        || name == "between" && kept > filler,
                    "{context}: block {kept} is named"
                );
            }
            if name != "plain" {
                for mention in [0, filler + 1, 2 * filler + 2] {
                    assert!(
                        matches!(splices.locate_old(mention), Fate::Replaced { .. }),
                        "{context}: the mention at {mention} is not named"
                    );
                }
            }
        }
    }
}

/// Two changes to the text in different places that are installed together
/// (two large pastes before a parse lands) are what the diff cannot line up:
/// the blocks between them moved by two different distances. They are one
/// covering splice, the same result a full parse would give and the cost
/// before the list of splices existed; the equality of the final document is
/// what the tests of the plugin check.
#[test]
fn a_parse_from_scratch_after_two_distant_changes_covers_what_lies_between() {
    let source = synthetic::spread_references_source(300);
    let old = Document::parse(source.as_str());
    let mut full = source.clone();
    full.push_str("At the end.\n\n");
    full.insert_str(40, "Near the top.\n\n");
    let new = Document::parse(full.as_str());
    let splices = new.splice_from(&old);
    support::assert_explains(&support::snapshot(&old), &new, &splices, None, "two");
    assert_eq!(splices.len(), 1, "{splices:?}");
}

/// Two texts with little in common are one splice, not hundreds: a renderer
/// would spend more bookkeeping on the list than on rebuilding.
#[test]
fn a_parse_from_scratch_with_too_many_stretches_reports_one_covering_splice() {
    let old = Document::parse(synthetic::source(2_000));
    // Every other block changed, without changing its length.
    let mut source = old.source().to_owned();
    let mut changed = 0;
    for ix in (0..old.len()).step_by(2) {
        let block = old.block(ix).unwrap().span();
        if let Some(c) = source[block.clone()].find('a').map(|p| block.start + p) {
            source.replace_range(c..=c, "b");
            changed += 1;
        }
    }
    assert!(
        changed > 2 * bevy_md_core::MAX_DIFF_SPLICES,
        "{changed} changed"
    );
    let new = Document::parse(source.as_str());
    let splices = new.splice_from(&old);
    assert_eq!(splices.len(), 1, "{splices:?}");
    support::assert_explains(&support::snapshot(&old), &new, &splices, None, "collapsed");
    // And with fewer, each one is named.
    let few = {
        let mut s = old.source().to_owned();
        for ix in [10, 200, 900] {
            let block = old.block(ix).unwrap().span();
            if let Some(p) = s[block.clone()].find('a') {
                s.replace_range(block.start + p..=block.start + p, "b");
            }
        }
        Document::parse(s)
    };
    let splices = few.splice_from(&old);
    assert!(
        (1..=3).contains(&splices.len()) && splices.removed() <= 3,
        "{splices:?}"
    );
}
