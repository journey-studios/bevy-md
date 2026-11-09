//! The flat block list a document is made of.
//!
//! Nesting (lists inside quotes inside lists) is flattened into depths, the
//! way a Notion page is a list of indented blocks. A flat list is what a
//! virtualized renderer wants: block `i` is row `i`.

use std::ops::Range;

use crate::inline::{Bias, Inline};

/// Column alignment of a table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// No alignment given.
    None,
    /// `:--`
    Left,
    /// `:-:`
    Center,
    /// `--:`
    Right,
}

/// The bullet of a list item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListMarker {
    /// `-`, `*` or `+`.
    Bullet,
    /// `1.` or `1)`, with the item's number.
    Ordered(u64),
}

/// What a block is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockKind {
    /// A paragraph, also a later paragraph of a list item.
    Paragraph,
    /// `#` to `######`.
    Heading(u8),
    /// The first paragraph of a list item. `task` is set for `- [ ]` items.
    ListItem {
        /// Bullet or number.
        marker: ListMarker,
        /// `Some(checked)` for task items.
        task: Option<bool>,
    },
    /// A fenced or indented code block; the text is the code.
    CodeBlock {
        /// The fence's info string, when it has one.
        lang: Option<Box<str>>,
    },
    /// `---`
    Rule,
    /// A raw HTML block; the text is the HTML.
    Html,
    /// YAML or TOML front matter; the text is the raw metadata.
    Frontmatter,
    /// A GFM table. The block's own inline content is empty.
    Table {
        /// Alignment of each column.
        align: Box<[Align]>,
        /// Rows of cells; the first row is the header.
        rows: Vec<Vec<Inline>>,
    },
}

/// One rendered block of a document.
///
/// A block does not know where it sits in the source: positions are kept
/// relative (the block's length, and where its root starts and ends compared
/// with its own start), and the document derives the absolute ones from a
/// sum tree. An edit therefore leaves every block after it untouched. Read
/// absolute positions through [`BlockRef`], which [`Document::block`] and
/// [`Document::blocks`] hand out.
///
/// [`Document::block`]: crate::Document::block
/// [`Document::blocks`]: crate::Document::blocks
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    /// What the block is.
    pub kind: BlockKind,
    /// Text and formatting.
    pub content: Inline,
    /// How many quotes the block sits in.
    pub quote_depth: u8,
    /// How many lists the block sits in.
    pub list_depth: u8,
    /// Bytes of the source this block came from.
    pub(crate) len: usize,
    /// Where the top-level construct (list, quote, paragraph...) the block
    /// belongs to starts, as a signed distance from the block's own start.
    /// Blocks sharing a root are always reparsed together.
    pub(crate) root_start: isize,
    /// Where that construct ends, as a signed distance from the block's start.
    pub(crate) root_end: isize,
}

impl Block {
    /// How many bytes of the source the block covers.
    pub fn span_len(&self) -> usize {
        self.len
    }

    /// Whether `other` is the same block, wherever it sits: everything but
    /// the extent of the root, which an edit elsewhere in the root changes.
    pub(crate) fn same_content(&self, other: &Block) -> bool {
        self.len == other.len
            && self.quote_depth == other.quote_depth
            && self.list_depth == other.list_depth
            && self.kind == other.kind
            && self.content == other.content
    }
}

/// A block together with where it sits in the source of its document.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlockRef<'a> {
    pub(crate) block: &'a Block,
    pub(crate) start: usize,
}

impl std::ops::Deref for BlockRef<'_> {
    type Target = Block;

    fn deref(&self) -> &Block {
        self.block
    }
}

impl<'a> BlockRef<'a> {
    /// The block itself, without its position.
    pub fn block(&self) -> &'a Block {
        self.block
    }

    /// Bytes of the source this block came from.
    pub fn span(&self) -> Range<usize> {
        self.start..self.start + self.block.len
    }

    /// Bytes of the top-level construct (list, quote, paragraph...) the
    /// block belongs to. Blocks sharing a root are always reparsed together.
    pub fn root(&self) -> Range<usize> {
        self.start.wrapping_add_signed(self.block.root_start)
            ..self.start.wrapping_add_signed(self.block.root_end)
    }

    /// The source byte (in the whole document) that the rendered-text byte
    /// `text` of [`Block::content`] stands for; see [`Inline::source_offset`].
    pub fn source_offset(&self, text: usize, bias: Bias) -> Option<usize> {
        Some(self.start + self.block.content.source_offset(text, bias)?)
    }

    /// The rendered-text byte for the source byte `source` of the document,
    /// or `None` when it lies outside this block; see [`Inline::text_offset`].
    pub fn text_offset(&self, source: usize) -> Option<usize> {
        let relative = source.checked_sub(self.start)?;
        (source <= self.start + self.block.len).then(|| self.block.content.text_offset(relative))
    }

    /// The source bytes (in the whole document) behind the rendered text
    /// `text` of [`Block::content`]; see [`Inline::source_range`].
    pub fn source_range(&self, text: Range<usize>) -> Option<Range<usize>> {
        let range = self.block.content.source_range(text)?;
        Some(self.start + range.start..self.start + range.end)
    }
}
