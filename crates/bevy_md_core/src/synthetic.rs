//! Synthetic documents of an exact number of blocks, for tests and
//! benchmarks.
//!
//! The text is the fixture `tests/fixtures/sample.md` (headings, paragraphs,
//! lists, quotes, code, a table...) repeated with a distinct title each time,
//! so that blocks are not all equal. Whole copies come first and one-line
//! paragraphs fill the last few blocks, which makes the count exact.

use std::fmt::Write;

use crate::Document;

/// The fixture every copy is made from.
pub const SAMPLE: &str = include_str!("../tests/fixtures/sample.md");

/// A markdown source that parses to exactly `blocks` blocks.
///
/// # Panics
///
/// Never in practice; it asserts the count it promises.
pub fn source(blocks: usize) -> String {
    let (title, rest) = SAMPLE
        .split_once('\n')
        .expect("the sample has a title line");
    debug_assert!(title.starts_with("# "));
    let per_copy = Document::parse(SAMPLE).len();
    let copies = blocks / per_copy;
    let mut out = String::with_capacity(blocks * (SAMPLE.len() / per_copy + 8));
    for copy in 0..copies {
        out.push_str(title);
        let _ = writeln!(out, " {copy}");
        out.push_str(rest);
        out.push('\n');
    }
    for filler in 0..blocks - copies * per_copy {
        let _ = write!(out, "Filler paragraph {filler} of the document.\n\n");
    }
    out
}

/// A parsed document of exactly `blocks` blocks.
///
/// # Panics
///
/// Never in practice; it asserts the count it promises.
pub fn document(blocks: usize) -> Document {
    let doc = Document::parse(source(blocks));
    assert_eq!(doc.len(), blocks, "the generator promises an exact count");
    doc
}

/// A markdown source of `copies` copies of the fixture, each followed by a
/// paragraph that mentions references and a footnote, with their definitions
/// next to them. Every fifth copy also points at a definition at the very
/// end of the document (`[far0]` ... ), far from the text that uses it.
///
/// It is the shape of a long note with links and footnotes: the document
/// every edit used to reparse in full.
///
/// # Panics
///
/// Never in practice; the sample has a title line.
pub fn references_source(copies: usize) -> String {
    let (title, rest) = SAMPLE
        .split_once('\n')
        .expect("the sample has a title line");
    let mut out = String::new();
    for copy in 0..copies {
        out.push_str(title);
        let _ = writeln!(out, " {copy}");
        out.push_str(rest);
        let _ = write!(
            out,
            "\nRead [the map {copy}][map{copy}], the [shortcut{copy}] and a note[^n{copy}]."
        );
        if copy % 5 == 0 {
            let _ = write!(out, " Also [far {copy}][far{copy}].");
        }
        let _ = write!(
            out,
            "\n\n[map{copy}]: https://example.com/map/{copy}\n[shortcut{copy}]: /s/{copy} \"title {copy}\"\n\n[^n{copy}]: The note {copy}.\n\n"
        );
    }
    // Far from the paragraphs that use them, in runs of ten: a run of
    // definitions with no blank line is one block of text, reparsed whole.
    for (n, copy) in (0..copies).step_by(5).enumerate() {
        if n > 0 && n % 10 == 0 {
            out.push('\n');
        }
        let _ = writeln!(out, "[far{copy}]: https://example.com/far/{copy}");
    }
    out
}

/// [`references_source`], parsed.
pub fn references_document(copies: usize) -> Document {
    Document::parse(references_source(copies))
}

/// A note of `3 * filler + 3` blocks whose first, middle and last paragraphs
/// (blocks `0`, `filler + 1` and `2 * filler + 2`) mention the link label
/// `[shared]`, with `filler` one-line paragraphs between and after them. It
/// defines nothing: adding [`SHARED_DEFINITION`] at the end (or anywhere)
/// turns the three mentions into links, three places that can be a hundred
/// thousand blocks apart.
pub fn spread_references_source(filler: usize) -> String {
    let mut out = String::with_capacity(filler * 3 * 48 + 128);
    let paragraphs = |out: &mut String, label: &str| {
        let _ = write!(out, "{label} [shared] mention of the label.\n\n");
        for n in 0..filler {
            let _ = write!(out, "Filler paragraph {n} of the document.\n\n");
        }
    };
    paragraphs(&mut out, "Top");
    paragraphs(&mut out, "Middle");
    paragraphs(&mut out, "Bottom");
    out
}

/// [`spread_references_source`], parsed.
///
/// # Panics
///
/// If the generator's count of blocks is wrong.
pub fn spread_references_document(filler: usize) -> Document {
    let doc = Document::parse(spread_references_source(filler));
    assert_eq!(doc.len(), 3 * filler + 3, "the generator promises a count");
    doc
}

/// The definition of the label [`spread_references_source`] mentions.
pub const SHARED_DEFINITION: &str = "[shared]: /dest\n";
