//! Height of a block before it has been laid out.
//!
//! A virtualized renderer needs a height for every block, including the ones
//! it never drew: the spacers standing in for them are as tall as the sum.
//! This guesses from the kind and the amount of text, without an engine: the
//! characters per line times a character width. It is only ever used until a
//! real measure replaces it, so it has to be cheap and in the right range,
//! not exact.

// Line counts are tiny next to what an `f32` holds exactly, and a height is
// only ever a guess: the casts cannot matter.
#![allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]

use crate::block::{Block, BlockKind};

/// What an estimate depends on: the theme and the width the text wraps at.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    /// Width of a block, in logical pixels.
    pub width: f32,
    /// Size of body text.
    pub body_size: f32,
    /// Size of code.
    pub code_size: f32,
    /// Size of `#` to `######` headings.
    pub heading_sizes: [f32; 6],
    /// Indent per list level.
    pub indent: f32,
    /// Gap below a block.
    pub block_gap: f32,
    /// Gap below a list item.
    pub list_gap: f32,
    /// Line height as a multiple of the font size.
    pub line_height: f32,
    /// Average glyph advance as a multiple of the font size.
    pub char_width: f32,
}

impl Default for Metrics {
    fn default() -> Self {
        Self {
            width: 720.0,
            body_size: 16.0,
            code_size: 14.0,
            heading_sizes: [30.0, 24.0, 20.0, 18.0, 16.0, 15.0],
            indent: 24.0,
            block_gap: 10.0,
            list_gap: 4.0,
            line_height: 1.2,
            char_width: 0.55,
        }
    }
}

/// Padding around code, top and bottom together.
const CODE_PADDING: f32 = 20.0;
/// Height added to a table row by the padding and border of its cells.
const CELL_PADDING: f32 = 13.0;

/// The estimated height of `block` in logical pixels, margins included.
#[must_use]
pub fn estimate_height(block: &Block, m: &Metrics) -> f32 {
    let list_levels = match block.kind {
        BlockKind::ListItem { .. } => block.list_depth.saturating_sub(1),
        _ => block.list_depth,
    };
    let quote_indent = f32::from(block.quote_depth) * (m.indent * 0.6 + 3.0);
    let indent = f32::from(list_levels) * m.indent + quote_indent;
    let width = (m.width - indent).max(m.indent);
    match &block.kind {
        BlockKind::Rule => 1.0 + 2.0 * m.block_gap,
        BlockKind::CodeBlock { .. } | BlockKind::Html | BlockKind::Frontmatter => {
            let lines = block
                .content
                .text
                .trim_end_matches('\n')
                .split('\n')
                .count();
            lines_height(lines, m.code_size, m) + CODE_PADDING + m.block_gap
        }
        BlockKind::Table { rows, .. } => {
            let row = m.body_size * m.line_height + CELL_PADDING;
            // Header and body rows, plus the outer border.
            rows.len().max(1) as f32 * row + 2.0 + m.block_gap
        }
        BlockKind::Heading(level) => {
            let size = m.heading_sizes[usize::from(level.clamp(&1, &6) - 1)];
            // A heading's margin is above and below.
            lines_height(wrapped_lines(&block.content.text, size, width, m), size, m)
                + 2.0 * m.block_gap
        }
        BlockKind::ListItem { .. } => {
            // The marker takes a column of one indent.
            let text_width = (width - m.indent).max(m.indent);
            lines_height(
                wrapped_lines(&block.content.text, m.body_size, text_width, m),
                m.body_size,
                m,
            ) + m.list_gap
        }
        BlockKind::Paragraph => {
            lines_height(
                wrapped_lines(&block.content.text, m.body_size, width, m),
                m.body_size,
                m,
            ) + m.block_gap
        }
    }
}

fn lines_height(lines: usize, size: f32, m: &Metrics) -> f32 {
    lines as f32 * size * m.line_height
}

/// Lines `text` takes when wrapped at `width`: each hard break starts a new
/// line, and every line wraps at the characters that fit.
fn wrapped_lines(text: &str, size: f32, width: f32, m: &Metrics) -> usize {
    let per_line = (width / (size * m.char_width)).floor().max(1.0);
    text.split('\n')
        .map(|line| {
            let chars = line.chars().count() as f32;
            (chars / per_line).ceil().max(1.0) as usize
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Document;

    fn heights(source: &str) -> Vec<f32> {
        let m = Metrics::default();
        Document::parse(source)
            .blocks()
            .map(|b| estimate_height(b.block(), &m))
            .collect()
    }

    #[test]
    fn longer_text_is_taller() {
        let short = heights("short")[0];
        let long = heights(&"word ".repeat(400))[0];
        assert!(long > short * 5.0, "{long} vs {short}");
    }

    #[test]
    fn headings_are_taller_than_a_line_of_body_text() {
        let h = heights("# Title\n\ntext");
        assert!(h[0] > h[1], "{h:?}");
    }

    #[test]
    fn code_grows_with_its_lines() {
        let one = heights("```\na\n```")[0];
        let five = heights("```\na\nb\nc\nd\ne\n```")[0];
        assert!(five > one + 3.0 * 14.0 * 1.2, "{five} vs {one}");
    }

    #[test]
    fn every_block_has_a_positive_finite_height() {
        let source = include_str!("../tests/fixtures/sample.md");
        for h in heights(source) {
            assert!(h.is_finite() && h > 0.0, "{h}");
        }
    }

    #[test]
    fn narrower_blocks_wrap_more() {
        let doc = Document::parse("word ".repeat(100));
        let block = doc.block(0).unwrap();
        let wide = estimate_height(block.block(), &Metrics::default());
        let narrow = estimate_height(
            block.block(),
            &Metrics {
                width: 200.0,
                ..Metrics::default()
            },
        );
        assert!(narrow > wide);
    }
}
