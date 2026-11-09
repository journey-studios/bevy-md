//! pulldown-cmark events to the flat block list.

use std::ops::Range;

use pulldown_cmark::{
    Alignment, BrokenLink, CodeBlockKind, CowStr, Event, HeadingLevel, LinkType, Options, Parser,
    Tag, TagEnd,
};

use crate::block::{Align, Block, BlockKind, ListMarker};
use crate::definitions::{
    Def, DefKind, Found, link_use, normalize_label, scan_candidates, scan_note_labels,
    scan_note_uses,
};
use crate::inline::{Inline, Style};

/// The markdown flavor the parser reads: GFM plus the extensions a notes app
/// needs.
///
/// Front matter is not a pulldown-cmark option here: the parser recognizes
/// it itself, and only at the very start of the document. pulldown-cmark
/// accepts a `---` block anywhere, which makes whether a `---` is a rule
/// depend on a line arbitrarily far below it.
pub fn default_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM
        | Options::ENABLE_MATH
        | Options::ENABLE_WIKILINKS
}

/// A parsed block and where it starts in the document.
pub(crate) struct Placed {
    pub start: usize,
    pub block: Block,
}

impl Placed {
    /// A block over the bytes `span` of the document, inside the root `root`.
    fn new(
        kind: BlockKind,
        content: Inline,
        span: Range<usize>,
        root: &Range<usize>,
        quote_depth: u8,
        list_depth: u8,
    ) -> Self {
        let relative = |pos: usize| pos.cast_signed() - span.start.cast_signed();
        Self {
            start: span.start,
            block: Block {
                kind,
                content,
                quote_depth,
                list_depth,
                len: span.len(),
                root_start: relative(root.start),
                root_end: relative(root.end),
            },
        }
    }
}

/// What a parse of a slice produced.
pub(crate) struct Parsed {
    pub blocks: Vec<Placed>,
    /// The definitions in the slice, text that looks like one, and which
    /// blocks mention which labels.
    pub found: Found,
}

/// What the rest of the document says about the labels a slice mentions.
pub(crate) struct Outside<'a> {
    /// The destination of the link definition of a label elsewhere.
    pub link: &'a dyn Fn(&str) -> Option<Box<str>>,
    /// Whether a footnote label is defined elsewhere.
    pub note: &'a dyn Fn(&str) -> bool,
}

impl Outside<'_> {
    /// For a parse of the whole document: there is no elsewhere.
    pub const NOWHERE: Outside<'static> = Outside {
        link: &|_| None,
        note: &|_| false,
    };
}

/// A terminator for the footnote definitions put before a slice: an HTML
/// block, which a footnote definition cannot continue into (an indented
/// slice start would be read as the definition's body otherwise).
const NOTE_PREFIX_END: &str = "<!-- -->\n\n";

/// Parse `source`, a slice that starts at byte `offset` of the document.
///
/// `outside` answers for the labels the slice does not define itself. A
/// parser given only a slice would leave `[text][label]` unresolved, and
/// `[^note]` plain text.
///
/// - Link labels go through the parser's broken-link callback.
/// - The parser has no such hook for footnotes, which resolve only against
///   definitions it has read. So for each footnote label the slice mentions
///   that is defined elsewhere, a definition of it is read first, in front
///   of the slice, and what it produces (a block, a definition, positions)
///   is dropped again.
pub(crate) fn parse(
    source: &str,
    offset: usize,
    options: Options,
    outside: &Outside<'_>,
) -> Parsed {
    let mut front = None;
    let (source, offset) = match (offset, frontmatter(source)) {
        (0, Some((text, end))) => {
            let span = 0..end;
            front = Some(Placed::new(
                BlockKind::Frontmatter,
                Inline {
                    text: text.to_owned(),
                    ..Inline::default()
                },
                span.clone(),
                &span,
                0,
                0,
            ));
            (&source[end..], end)
        }
        _ => (source, offset),
    };
    let prefix = note_prefix(source, options, outside);
    let combined;
    let text = if prefix.is_empty() {
        source
    } else {
        combined = format!("{prefix}{source}");
        combined.as_str()
    };
    // The parser works in positions of `text`, shifted by `offset`; what was
    // read from the added `prefix` sits before `offset + skip`, and the
    // rest is `skip` bytes too far.
    let skip = prefix.len();
    let added = |pos: usize| pos < offset + skip;
    let unshift = |pos: usize| pos - skip;

    // Labels the parser was asked about (it found no definition in the
    // text), by position.
    let mut unresolved: Vec<(usize, u64)> = Vec::new();
    let mut callback = |link: BrokenLink<'_>| {
        unresolved.push((link.span.start + offset, link_use(&link.reference)));
        let dest = (outside.link)(&link.reference)?;
        Some((CowStr::Boxed(dest), CowStr::Borrowed("")))
    };
    let parser = Parser::new_with_broken_link_callback(text, options, Some(&mut callback));
    let mut defs: Vec<(usize, Def)> = parser
        .reference_definitions()
        .iter()
        .map(|(label, def)| {
            (
                offset + def.span.start,
                Def {
                    label: label.into(),
                    kind: DefKind::Link(def.dest.as_ref().into()),
                },
            )
        })
        .collect();
    let mut builder = Builder::new(text, offset, Vec::new());
    for (event, range) in parser.into_offset_iter() {
        builder.event(event, range);
    }
    builder.finish();
    let Builder {
        blocks: parsed_blocks,
        refs,
        notes,
        ..
    } = builder;
    defs.extend(notes);
    let defs: Vec<(usize, Def)> = {
        defs.retain(|&(pos, _)| !added(pos));
        defs.sort_by_key(|&(pos, _)| pos);
        defs.into_iter()
            .map(|(pos, def)| (unshift(pos), def))
            .collect()
    };
    let blocks: Vec<Placed> = front
        .into_iter()
        .chain(
            parsed_blocks
                .into_iter()
                .filter(|placed| !added(placed.start))
                .map(|mut placed| {
                    placed.start = unshift(placed.start);
                    placed
                }),
        )
        .collect();
    let refs: Vec<(usize, u64)> = refs
        .into_iter()
        .chain(unresolved)
        .filter(|&(pos, _)| !added(pos))
        .map(|(pos, label)| (unshift(pos), label))
        .collect();
    let cands = if source.contains('[') {
        scan_candidates(source, offset)
    } else {
        Vec::new()
    };
    let uses = uses_by_block(&blocks, text, offset, skip, refs);
    Parsed {
        blocks,
        found: Found { defs, cands, uses },
    }
}

/// The footnote definitions to read before `source`: one for each label it
/// mentions that is defined elsewhere in the document.
fn note_prefix(source: &str, options: Options, outside: &Outside<'_>) -> String {
    if !options.contains(Options::ENABLE_FOOTNOTES) || !source.contains("[^") {
        return String::new();
    }
    let mut prefix = String::new();
    for label in scan_note_labels(source) {
        if (outside.note)(&label) {
            prefix.push_str("[^");
            prefix.push_str(&label);
            prefix.push_str("]: .\n\n");
        }
    }
    if !prefix.is_empty() {
        prefix.push_str(NOTE_PREFIX_END);
    }
    prefix
}

/// Which labels each block mentions: the reference links the parser read or
/// was asked about, and text that has the shape of a footnote reference
/// (which the parser keeps as text when no definition exists, so it never
/// says). Entries are the block's start; blocks with none are left out.
fn uses_by_block(
    blocks: &[Placed],
    text: &str,
    offset: usize,
    skip: usize,
    refs: Vec<(usize, u64)>,
) -> Vec<(usize, Box<[u64]>)> {
    let mut mentions: Vec<(usize, u64)> = Vec::new();
    let owner = |pos: usize| blocks.partition_point(|b| b.start <= pos).saturating_sub(1);
    if !blocks.is_empty() {
        mentions.extend(refs.into_iter().map(|(pos, label)| (owner(pos), label)));
    }
    if text.contains("[^") {
        let mut labels = Vec::new();
        for (ix, placed) in blocks.iter().enumerate() {
            if matches!(
                placed.block.kind,
                BlockKind::CodeBlock { .. } | BlockKind::Html | BlockKind::Frontmatter
            ) {
                continue;
            }
            // Back from the document to the text that was parsed.
            let from = placed.start + skip - offset;
            let Some(block_text) = text.get(from..from + placed.block.len) else {
                continue;
            };
            labels.clear();
            scan_note_uses(block_text, &mut labels);
            mentions.extend(labels.iter().map(|&label| (ix, label)));
        }
    }
    mentions.sort_unstable();
    mentions.dedup();
    let mut uses: Vec<(usize, Box<[u64]>)> = Vec::new();
    let mut at = 0;
    while at < mentions.len() {
        let block = mentions[at].0;
        let run = mentions[at..].iter().take_while(|m| m.0 == block).count();
        let labels: Box<[u64]> = mentions[at..at + run].iter().map(|m| m.1).collect();
        let start = blocks[block].start;
        // Blocks sharing a start share an entry.
        match uses.last_mut() {
            Some((last, existing)) if *last == start => {
                let mut merged = existing.to_vec();
                merged.extend(labels.iter());
                merged.sort_unstable();
                merged.dedup();
                *existing = merged.into();
            }
            _ => uses.push((start, labels)),
        }
        at += run;
    }
    uses
}

/// Front matter at the start of `source`: a `---` line, then lines up to a
/// closing `---` or `...` line. Returns the metadata text and where the
/// block ends, its closing newline included.
fn frontmatter(source: &str) -> Option<(&str, usize)> {
    let body_start = source
        .strip_prefix("---\n")
        .or_else(|| source.strip_prefix("---\r\n"))
        .map(|rest| source.len() - rest.len())?;
    let mut line_start = body_start;
    for line in source[body_start..].split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed == "---" || trimmed == "..." {
            return Some((&source[body_start..line_start], line_start + line.len()));
        }
        line_start += line.len();
    }
    None
}

/// A block whose inline content is still arriving.
struct Leaf {
    kind: BlockKind,
    span: Range<usize>,
    content: Inline,
}

/// A block whose raw text is still arriving (code, HTML, front matter).
struct Raw {
    kind: BlockKind,
    span: Range<usize>,
    text: String,
}

struct Table {
    span: Range<usize>,
    align: Box<[Align]>,
    rows: Vec<Vec<Inline>>,
    cell: Option<Inline>,
}

struct PendingItem {
    marker: ListMarker,
    task: Option<bool>,
    span: Range<usize>,
}

struct Builder<'a> {
    /// The slice being parsed; event ranges index into it.
    source: &'a str,
    offset: usize,
    blocks: Vec<Placed>,
    /// Open Start events; the top-level one defines the root.
    nesting: u32,
    root: Range<usize>,
    quote_depth: u8,
    list_depth: u8,
    /// Next number of each open list, `None` for bullet lists.
    lists: Vec<Option<u64>>,
    item: Option<PendingItem>,
    leaf: Option<Leaf>,
    raw: Option<Raw>,
    table: Option<Table>,
    style: Style,
    link: Option<u32>,
    inline_stack: Vec<(Style, Option<u32>)>,
    /// Reference links the parser resolved, by position in the document.
    refs: Vec<(usize, u64)>,
    /// Footnote definitions, by position in the document.
    notes: Vec<(usize, Def)>,
}

impl<'a> Builder<'a> {
    fn new(source: &'a str, offset: usize, blocks: Vec<Placed>) -> Self {
        Self {
            source,
            offset,
            blocks,
            nesting: 0,
            root: 0..0,
            quote_depth: 0,
            list_depth: 0,
            lists: Vec::new(),
            item: None,
            leaf: None,
            raw: None,
            table: None,
            style: Style::PLAIN,
            link: None,
            inline_stack: Vec::new(),
            refs: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn shift(&self, range: Range<usize>) -> Range<usize> {
        range.start + self.offset..range.end + self.offset
    }

    /// Where `text` starts in the document, when `locate` finds it verbatim
    /// in the slice of source the event covers.
    fn source_pos(
        &self,
        range: &Range<usize>,
        text: &str,
        locate: impl Fn(&str, &str) -> Option<usize>,
    ) -> Option<usize> {
        let slice = self.source.get(range.clone())?;
        Some(range.start + self.offset + locate(slice, text)?)
    }

    fn event(&mut self, event: Event<'_>, range: Range<usize>) {
        let local = range.clone();
        let range = self.shift(range);
        match event {
            Event::Start(tag) => {
                if self.nesting == 0 {
                    self.root = range.clone();
                }
                self.nesting += 1;
                self.start(tag, range);
            }
            Event::End(tag) => {
                self.end(tag);
                self.nesting -= 1;
            }
            Event::Text(text) => {
                if let Some(raw) = &mut self.raw {
                    raw.text.push_str(&text);
                } else {
                    let src = self.source_pos(&local, &text, verbatim);
                    self.push_inline(&text, Style::PLAIN, &range, src);
                }
            }
            Event::Code(text) => {
                let src = self.source_pos(&local, &text, |slice, text| delimited(slice, text, '`'));
                self.push_inline(&text, Style::CODE, &range, src);
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => {
                let src = self.source_pos(&local, &text, |slice, text| delimited(slice, text, '$'));
                self.push_inline(&text, Style::MATH, &range, src);
            }
            Event::Html(text) => {
                if let Some(raw) = &mut self.raw {
                    raw.text.push_str(&text);
                }
            }
            Event::InlineHtml(text) => {
                let src = self.source_pos(&local, &text, verbatim);
                self.push_inline(&text, Style::PLAIN, &range, src);
            }
            Event::FootnoteReference(label) => {
                let text = format!("[{label}]");
                self.push_inline(&text, Style::SUPERSCRIPT, &range, None);
            }
            Event::SoftBreak => {
                // A newline renders as a space of the same size, so the
                // text around it can stay one run.
                let src = self.source_pos(&local, " ", |slice, _| (slice == "\n").then_some(0));
                self.push_inline(" ", Style::PLAIN, &range, src);
            }
            Event::HardBreak => self.push_inline("\n", Style::PLAIN, &range, None),
            Event::Rule => {
                if self.nesting == 0 {
                    self.root = range.clone();
                }
                self.close_leaf();
                self.flush_item();
                self.push_block(BlockKind::Rule, Inline::default(), range);
            }
            Event::TaskListMarker(checked) => {
                if let Some(Leaf {
                    kind: BlockKind::ListItem { task, .. },
                    ..
                }) = &mut self.leaf
                {
                    *task = Some(checked);
                } else if let Some(item) = &mut self.item {
                    item.task = Some(checked);
                }
            }
        }
    }

    #[expect(
        clippy::too_many_lines,
        reason = "one arm for each kind of tag, read top to bottom"
    )]
    fn start(&mut self, tag: Tag<'_>, range: Range<usize>) {
        match tag {
            Tag::Paragraph => self.open_leaf(BlockKind::Paragraph, range),
            Tag::Heading { level, .. } => {
                self.item = None;
                self.open_leaf(BlockKind::Heading(heading_level(level)), range);
            }
            Tag::BlockQuote(_) => {
                self.close_leaf();
                self.quote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(info) => {
                        let lang = info.split_whitespace().next().unwrap_or("");
                        (!lang.is_empty()).then(|| lang.into())
                    }
                    CodeBlockKind::Indented => None,
                };
                self.open_raw(BlockKind::CodeBlock { lang }, range);
            }
            Tag::HtmlBlock => self.open_raw(BlockKind::Html, range),
            Tag::MetadataBlock(_) => self.open_raw(BlockKind::Frontmatter, range),
            Tag::List(start) => {
                self.close_leaf();
                self.lists.push(start);
                self.list_depth += 1;
            }
            Tag::Item => {
                self.close_leaf();
                let marker = match self.lists.last_mut() {
                    Some(Some(next)) => {
                        let number = *next;
                        *next += 1;
                        ListMarker::Ordered(number)
                    }
                    _ => ListMarker::Bullet,
                };
                self.item = Some(PendingItem {
                    marker,
                    task: None,
                    span: range,
                });
            }
            Tag::Table(align) => {
                self.close_leaf();
                self.flush_item();
                self.table = Some(Table {
                    span: range,
                    align: align.into_iter().map(map_align).collect(),
                    rows: Vec::new(),
                    cell: None,
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.rows.push(Vec::new());
                }
            }
            Tag::TableCell => {
                if let Some(table) = &mut self.table {
                    table.cell = Some(Inline::default());
                }
            }
            Tag::FootnoteDefinition(label) => {
                self.close_leaf();
                self.notes.push((
                    range.start,
                    Def {
                        label: normalize_label(&label).into(),
                        kind: DefKind::Note,
                    },
                ));
            }
            Tag::DefinitionList | Tag::DefinitionListTitle | Tag::DefinitionListDefinition => {
                self.close_leaf();
            }
            Tag::Emphasis => self.push_style(Style::ITALIC),
            Tag::Strong => self.push_style(Style::BOLD),
            Tag::Strikethrough => self.push_style(Style::STRIKE),
            Tag::Superscript => self.push_style(Style::SUPERSCRIPT),
            Tag::Subscript => self.push_style(Style::SUBSCRIPT),
            Tag::Link {
                dest_url,
                link_type,
                id,
                ..
            } => {
                self.note_reference(link_type, &id, range.start);
                let link = self.target_inline(&range).add_link(&dest_url);
                self.inline_stack.push((self.style, self.link));
                self.link = Some(link);
            }
            Tag::Image {
                dest_url,
                link_type,
                id,
                ..
            } => {
                self.note_reference(link_type, &id, range.start);
                let link = self.target_inline(&range).add_link(&dest_url);
                self.inline_stack.push((self.style, self.link));
                self.style = self.style.union(Style::IMAGE);
                self.link = Some(link);
            }
        }
    }

    /// Remember that a link at `pos` is a reference to the label `id`.
    fn note_reference(&mut self, link_type: LinkType, id: &str, pos: usize) {
        if !id.is_empty()
            && !matches!(
                link_type,
                LinkType::Inline | LinkType::Autolink | LinkType::Email | LinkType::WikiLink { .. }
            )
        {
            self.refs.push((pos, link_use(id)));
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph
            | TagEnd::Heading(_)
            | TagEnd::FootnoteDefinition
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition => self.close_leaf(),
            TagEnd::BlockQuote(_) => {
                self.close_leaf();
                self.quote_depth -= 1;
            }
            TagEnd::CodeBlock | TagEnd::HtmlBlock | TagEnd::MetadataBlock(_) => {
                if let Some(raw) = self.raw.take() {
                    let content = Inline {
                        runs: Vec::new(),
                        links: Vec::new(),
                        text: raw.text,
                    };
                    self.push_block(raw.kind, content, raw.span);
                }
            }
            TagEnd::List(_) => {
                self.close_leaf();
                self.lists.pop();
                self.list_depth -= 1;
            }
            TagEnd::Item => {
                self.close_leaf();
                // An item with no text still shows its bullet.
                if let Some(item) = self.item.take() {
                    self.push_empty_item(item);
                }
            }
            TagEnd::TableCell => {
                if let Some(table) = &mut self.table
                    && let Some(mut cell) = table.cell.take()
                    && let Some(row) = table.rows.last_mut()
                {
                    cell.rebase(table.span.start);
                    row.push(cell);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    let kind = BlockKind::Table {
                        align: table.align,
                        rows: table.rows,
                    };
                    self.push_block(kind, Inline::default(), table.span);
                }
            }
            TagEnd::Emphasis
            | TagEnd::Strong
            | TagEnd::Strikethrough
            | TagEnd::Superscript
            | TagEnd::Subscript
            | TagEnd::Link
            | TagEnd::Image => {
                if let Some((style, link)) = self.inline_stack.pop() {
                    self.style = style;
                    self.link = link;
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {}
        }
    }

    /// Emit the pending list item as an empty one, now: the block that
    /// follows it must not come before it in the list.
    fn flush_item(&mut self) {
        if let Some(item) = self.item.take() {
            self.push_empty_item(item);
        }
    }

    fn push_empty_item(&mut self, item: PendingItem) {
        let kind = BlockKind::ListItem {
            marker: item.marker,
            task: item.task,
        };
        self.push_block(kind, Inline::default(), item.span);
    }

    fn push_style(&mut self, style: Style) {
        self.inline_stack.push((self.style, self.link));
        self.style = self.style.union(style);
    }

    fn push_inline(&mut self, text: &str, extra: Style, range: &Range<usize>, src: Option<usize>) {
        let style = self.style.union(extra);
        let link = self.link;
        self.target_inline(range).push(text, style, link, src);
    }

    /// Where inline content goes: the open table cell, or the open leaf,
    /// opening an implicit one for text a tight list item holds directly.
    fn target_inline(&mut self, range: &Range<usize>) -> &mut Inline {
        if self
            .table
            .as_ref()
            .is_some_and(|table| table.cell.is_some())
        {
            return self
                .table
                .as_mut()
                .and_then(|table| table.cell.as_mut())
                .expect("checked above");
        }
        if self.leaf.is_none() {
            self.open_leaf(BlockKind::Paragraph, range.clone());
        }
        let leaf = self.leaf.as_mut().expect("leaf opened above");
        leaf.span.end = leaf.span.end.max(range.end);
        &mut leaf.content
    }

    fn open_leaf(&mut self, kind: BlockKind, range: Range<usize>) {
        self.close_leaf();
        let (kind, span) = match (kind, self.item.take()) {
            (BlockKind::Paragraph, Some(item)) => (
                BlockKind::ListItem {
                    marker: item.marker,
                    task: item.task,
                },
                item.span.start..range.end,
            ),
            (kind, _) => (kind, range),
        };
        self.leaf = Some(Leaf {
            kind,
            span,
            content: Inline::default(),
        });
    }

    fn open_raw(&mut self, kind: BlockKind, range: Range<usize>) {
        self.close_leaf();
        self.item = None;
        self.raw = Some(Raw {
            kind,
            span: range,
            text: String::new(),
        });
    }

    fn close_leaf(&mut self) {
        if let Some(leaf) = self.leaf.take() {
            self.push_block(leaf.kind, leaf.content, leaf.span);
        }
    }

    fn push_block(&mut self, kind: BlockKind, mut content: Inline, span: Range<usize>) {
        content.rebase(span.start);
        self.blocks.push(Placed::new(
            kind,
            content,
            span,
            &self.root,
            self.quote_depth,
            self.list_depth,
        ));
    }

    fn finish(&mut self) {
        self.close_leaf();
    }
}

/// Where `text` starts in `slice`, the source of an event, when it is a
/// verbatim copy of it: the whole slice, or what follows a backslash escape.
fn verbatim(slice: &str, text: &str) -> Option<usize> {
    if slice == text {
        return Some(0);
    }
    (slice.strip_prefix('\\')? == text).then_some(1)
}

/// Where `text` starts in `slice`, the source of a code span or math: after
/// the delimiters, and after the one space of padding both sides can add.
fn delimited(slice: &str, text: &str, delimiter: char) -> Option<usize> {
    let count = slice.len() - slice.trim_start_matches(delimiter).len();
    [count, count + 1]
        .into_iter()
        .find(|&at| slice.get(at..at + text.len()) == Some(text))
}

fn heading_level(level: HeadingLevel) -> u8 {
    level as u8
}

fn map_align(align: Alignment) -> Align {
    match align {
        Alignment::None => Align::None,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}
