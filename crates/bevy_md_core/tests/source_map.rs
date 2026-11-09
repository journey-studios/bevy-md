//! The source map: rendered text offsets to source bytes and back.

use bevy_md_core::{Bias, BlockKind, Document, Inline};

/// Every mapped run is a verbatim copy of the source bytes it points at,
/// except that a soft line break is a space standing for a newline.
fn assert_verbatim(source: &str, base: usize, inline: &Inline) {
    for run in &inline.runs {
        let Some(range) = run.source_range() else {
            continue;
        };
        let copy = &source[base + range.start..base + range.end];
        let text = inline.run_text(run);
        assert_eq!(copy.len(), text.len(), "{copy:?} vs {text:?}");
        for (src, shown) in copy.chars().zip(text.chars()) {
            assert!(
                src == shown || (src == '\n' && shown == ' '),
                "run {text:?} is not a copy of {copy:?}"
            );
        }
    }
}

/// A document whose first block is checked to map verbatim.
fn paragraph(source: &str) -> Document {
    let doc = Document::parse(source);
    for block in doc.blocks() {
        assert_verbatim(doc.source(), block.span().start, &block.content);
    }
    doc
}

#[test]
fn plain_text_maps_one_to_one() {
    let doc = paragraph("hello world");
    let block = doc.block(0).unwrap();
    assert_eq!(block.content.runs.len(), 1);
    for at in 0..=11 {
        assert_eq!(block.source_offset(at, Bias::Upstream), Some(at));
        assert_eq!(block.source_offset(at, Bias::Downstream), Some(at));
        assert_eq!(block.text_offset(at), Some(at));
    }
    assert_eq!(block.source_offset(12, Bias::Upstream), None);
}

#[test]
fn syntax_is_skipped_between_runs() {
    //          0         1         2         3
    //          0123456789012345678901234567890123456789
    let source = "Some **bold** and *italic* text with `code`";
    let doc = paragraph(source);
    let block = doc.block(0).unwrap();
    assert_eq!(block.content.text, "Some bold and italic text with code");

    // "bold" is text 5..9 and source 7..11.
    assert_eq!(block.source_range(5..9), Some(7..11));
    assert_eq!(block.source_range(6..8), Some(8..10));
    // A caret on the boundary: before the `**`, or inside it.
    assert_eq!(block.source_offset(5, Bias::Upstream), Some(5));
    assert_eq!(block.source_offset(5, Bias::Downstream), Some(7));
    // After "bold": inside the closing `**`, or after it.
    assert_eq!(block.source_offset(9, Bias::Upstream), Some(11));
    assert_eq!(block.source_offset(9, Bias::Downstream), Some(13));

    // Back to text: the syntax collapses onto the boundary next to it.
    assert_eq!(block.text_offset(5), Some(5));
    assert_eq!(block.text_offset(6), Some(5));
    assert_eq!(block.text_offset(7), Some(5));
    assert_eq!(block.text_offset(11), Some(9));
    assert_eq!(block.text_offset(12), Some(9));
    assert_eq!(block.text_offset(13), Some(9));

    // The code span starts after its backtick.
    let code = source.find("code").unwrap();
    assert_eq!(block.source_range(31..35), Some(code..code + 4));
    assert_eq!(block.text_offset(code + 2), Some(33));
}

#[test]
fn every_text_offset_round_trips() {
    let doc = paragraph("a **b** c *d* `e` f [g](h) i ~~j~~ k");
    let block = doc.block(0).unwrap();
    let text = &block.content.text;
    for at in (0..=text.len()).filter(|&at| text.is_char_boundary(at)) {
        for bias in [Bias::Upstream, Bias::Downstream] {
            let source = block.source_offset(at, bias).unwrap();
            assert_eq!(block.text_offset(source), Some(at), "text {at} {bias:?}");
        }
    }
}

#[test]
fn escapes_map_the_escaped_byte() {
    //          01234567
    let doc = paragraph(r"a \* b");
    let block = doc.block(0).unwrap();
    assert_eq!(block.content.text, "a * b");
    assert_eq!(block.source_range(2..3), Some(3..4));
    // The backslash is skipped, not part of the text.
    assert_eq!(block.text_offset(2), Some(2));
    assert_eq!(block.text_offset(3), Some(2));
    assert_eq!(block.text_offset(4), Some(3));
}

#[test]
fn entities_are_not_mapped_but_deleted_whole() {
    //          0123456789012345
    let doc = paragraph("fish &amp; chips");
    let block = doc.block(0).unwrap();
    assert_eq!(block.content.text, "fish & chips");
    let entity = block
        .content
        .runs
        .iter()
        .find(|run| block.content.run_text(run) == "&");
    assert_eq!(entity.unwrap().source_start(), None);
    // Deleting the `&` takes the whole entity with it.
    assert_eq!(block.source_range(5..6), Some(5..10));
    // A caret on either side of it still lands in the source.
    assert_eq!(block.source_offset(5, Bias::Downstream), Some(5));
    assert_eq!(block.source_offset(6, Bias::Upstream), Some(10));
}

#[test]
fn soft_breaks_keep_a_run_whole() {
    let doc = paragraph("one\ntwo");
    let block = doc.block(0).unwrap();
    assert_eq!(block.content.text, "one two");
    assert_eq!(block.content.runs.len(), 1);
    assert_eq!(block.source_offset(4, Bias::Upstream), Some(4));
    // Deleting the space joins the lines.
    assert_eq!(block.source_range(3..4), Some(3..4));
}

#[test]
fn quote_prefixes_split_runs() {
    let doc = Document::parse("> one\n> two");
    let block = &doc.block(0).unwrap();
    assert_verbatim(doc.source(), block.span().start, &block.content);
    assert_eq!(block.content.text, "one two");
    // The second line's `> ` is not text: a second run.
    assert_eq!(block.content.runs.len(), 2);
    // "two" is text 4..7 and source 8..11.
    assert_eq!(block.source_range(4..7), Some(8..11));
    assert_eq!(block.text_offset(7), Some(4));
}

#[test]
fn list_items_and_headings_map_past_their_marker() {
    let doc = Document::parse("# Title\n\n- item **one**\n");
    let heading = &doc.block(0).unwrap();
    assert_eq!(heading.source_range(0..5), Some(2..7));
    let item = &doc.block(1).unwrap();
    assert!(matches!(item.kind, BlockKind::ListItem { .. }));
    let one = doc.source().find("one").unwrap();
    // "item " is 5 bytes of text, so "one" starts at text 5.
    assert_eq!(item.source_range(5..8), Some(one..one + 3));
    assert_eq!(item.text_offset(one + 1), Some(6));
    assert_eq!(heading.text_offset(one), None);
}

#[test]
fn hard_breaks_are_unmapped_but_neighbors_are() {
    let doc = paragraph("a  \nb");
    let block = doc.block(0).unwrap();
    assert_eq!(block.content.text, "a\nb");
    assert_eq!(block.content.runs.len(), 3);
    assert_eq!(block.source_range(2..3), Some(4..5));
}

#[test]
fn table_cells_map_from_the_table_start() {
    let doc = Document::parse("para\n\n| a | **b** |\n|---|---|\n| c | d |\n");
    let table = &doc.block(1).unwrap();
    let BlockKind::Table { rows, .. } = &table.kind else {
        panic!("not a table");
    };
    for cell in rows.iter().flatten() {
        assert_verbatim(doc.source(), table.span().start, cell);
    }
    let bold = &rows[0][1];
    let run = &bold.runs[0];
    let start = table.span().start + run.source_start().unwrap();
    assert_eq!(&doc.source()[start..=start], "b");
}

#[test]
fn offsets_follow_a_block_that_moves() {
    let mut doc = Document::parse("first\n\nsecond **bold** end\n");
    let before = doc.block(1).unwrap().source_range(7..11).unwrap();
    doc.edit(0..0, "xx");
    let after = doc.block(1).unwrap().source_range(7..11).unwrap();
    assert_eq!(after, before.start + 2..before.end + 2);
    assert_eq!(&doc.source()[after], "bold");
}

#[test]
fn edits_through_the_map_change_what_they_should() {
    let mut doc = Document::parse("Some **bold** text");
    // Delete the "ol" of "bold" (text 6..8) through the map.
    let range = doc.block(0).unwrap().source_range(6..8).unwrap();
    doc.edit(range, "");
    assert_eq!(doc.source(), "Some **bd** text");
    assert_eq!(doc.block(0).unwrap().content.text, "Some bd text");
    // Typing at the end of "bd" (text 7), continuing the bold run.
    let at = doc
        .block(0)
        .unwrap()
        .source_offset(7, Bias::Upstream)
        .unwrap();
    doc.edit(at..at, "X");
    assert_eq!(doc.source(), "Some **bdX** text");
    assert_eq!(doc.block(0).unwrap().content.text, "Some bdX text");
}

#[test]
fn code_blocks_and_empty_blocks_have_no_map() {
    let doc = Document::parse("```\ncode\n```\n");
    assert_eq!(doc.block(0).unwrap().source_offset(0, Bias::Upstream), None);
    let doc = Document::parse("---\n");
    assert_eq!(doc.block(0).unwrap().source_offset(0, Bias::Upstream), None);
}
