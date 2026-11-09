//! Blocks as the caret edits them: Enter splits one, and Backspace and Delete
//! at the edge of one join it to its neighbour.
//!
//! Like every edit here, these are replacements of the markdown source, and
//! each is one splice, so it is one step of the history that undoes byte for
//! byte:
//!
//! - **Enter** puts a block break at the caret. A paragraph or a heading
//!   (whose rest becomes a paragraph) gets the blank line markdown needs, with
//!   the prefix of the line it is on so that a quote continues with `> `. A list
//!   item gets a new item with the same marker (numbers go up, a task starts
//!   unchecked); Enter on an empty item leaves the list, or goes one level
//!   out of a nested one. In a code block it is a literal line break. Inside
//!   emphasis or code the delimiters are closed and opened again, so both halves
//!   keep the style.
//! - **Backspace at the start of a block** first strips what makes the block
//!   special (a list marker, the `#`s of a heading, one level of `>`), which
//!   turns it into a paragraph, and then appends the block to the one before.
//! - **Delete at the end of a block** appends the next block to it, dropping
//!   the marker or prefix the next one had.
//!
//! The caret of an empty block that markdown cannot represent (an empty
//! paragraph, which is only a blank line) sits in the gap between blocks;
//! Backspace there takes the blank lines back, so Enter and Backspace undo each
//! other.
//!
//! The engine gives the position of a block's text and where it starts, but a
//! block's span stops at its last character of text, before the closing
//! delimiters of its last run, while the joins and splits need the end of its
//! last line; the helpers here (`content_start`, `content_end`) find both from
//! the source.

use std::ops::Range;
use std::time::Duration;

use bevy::input::keyboard::Key;
use bevy_md::Markdown;
use bevy_md_core::{Bias, BlockKind, BlockRef, Document, Run, Style};

use crate::history::Change;
use crate::words::Direction;
use crate::{Action, MarkdownEditor, Modifiers, edit};

/// The action a key asks for when it is about blocks or words: Enter, and the
/// command key with the arrows, Backspace and Delete. Shift with an arrow
/// is left for selecting.
pub(crate) fn key_action(key: &Key, modifiers: Modifiers) -> Option<Action> {
    let command = modifiers.command();
    match key {
        Key::Enter if !command => Some(Action::Enter),
        Key::ArrowLeft if command && !modifiers.shift => {
            Some(Action::MoveWord(Direction::Backward))
        }
        Key::ArrowRight if command && !modifiers.shift => {
            Some(Action::MoveWord(Direction::Forward))
        }
        Key::Backspace if command => Some(Action::DeleteWord(Direction::Backward)),
        Key::Delete if command => Some(Action::DeleteWord(Direction::Forward)),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Rewrites
// ---------------------------------------------------------------------------

/// One replacement of the source and where the caret goes after it.
struct Rewrite {
    range: Range<usize>,
    text: String,
    /// A byte of the source after the replacement.
    caret: usize,
}

/// Apply `rewrite` as a step of the history of its own. Returns whether
/// anything changed (a caret that moved counts).
fn commit(
    editor: &mut MarkdownEditor,
    markdown: &mut Markdown,
    rewrite: Rewrite,
    now: Duration,
) -> bool {
    let Rewrite { range, text, caret } = rewrite;
    let Some(old) = markdown.document().source().get(range.clone()) else {
        return false;
    };
    // Keep only the bytes that differ: a step costs what changed, and the
    // blocks the splice touches are the ones that do.
    let prefix: usize = old
        .chars()
        .zip(text.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let suffix: usize = old[prefix..]
        .chars()
        .rev()
        .zip(text[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let (old, new) = (
        old[prefix..old.len() - suffix].to_owned(),
        &text[prefix..text.len() - suffix],
    );
    let before = editor.caret;
    if old.is_empty() && new.is_empty() {
        editor.set_caret(markdown, caret);
        return editor.caret != before;
    }
    let start = range.start + prefix;
    let change = Change {
        start,
        old: old.clone(),
        new: new.to_owned(),
        caret_before: before,
        caret_after: caret,
    };
    markdown.edit(start..start + old.len(), new);
    editor.set_caret(markdown, caret);
    editor.history.seal();
    editor.history.record(change, now);
    editor.history.seal();
    true
}

// ---------------------------------------------------------------------------
// Lines
// ---------------------------------------------------------------------------

/// The start of the line `at` is on.
fn line_start(source: &str, at: usize) -> usize {
    source[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// The end of the line `at` is on, before its line break.
fn line_end(source: &str, at: usize) -> usize {
    source[at..].find('\n').map_or(source.len(), |i| at + i)
}

/// Whether `line` says nothing: spaces and quote marks only.
fn blank(line: &str) -> bool {
    line.chars().all(|c| matches!(c, ' ' | '\t' | '>' | '\r'))
}

/// Whether the line before the one starting at `line` is blank, or there is
/// none.
fn blank_before(source: &str, line: usize) -> bool {
    line == 0 || blank(&source[line_start(source, line - 1)..line - 1])
}

/// Whether the line after the one that ends at `end` is blank, or there is
/// none.
fn blank_after(source: &str, end: usize) -> bool {
    let next = end + 1;
    next >= source.len() || blank(&source[next..line_end(source, next)])
}

/// The prefix of the lines of a block as the line after it must carry it:
/// quote marks as they are and everything else (an outer list marker on the
/// same line) as spaces.
fn continuation(prefix: &str) -> String {
    prefix
        .chars()
        .map(|c| {
            if matches!(c, '>' | '\t' | ' ') {
                c
            } else {
                ' '
            }
        })
        .collect()
}

/// The quote marks and indentation the line at `at` starts with, up to `at`.
fn line_prefix(source: &str, at: usize) -> &str {
    let line = line_start(source, at);
    let rest = &source[line..at];
    let len = rest.len() - rest.trim_start_matches([' ', '\t', '>']).len();
    &rest[..len]
}

// ---------------------------------------------------------------------------
// Where the parts of a block are
// ---------------------------------------------------------------------------

/// A list marker as written: `-`, `3.`, `2)`, then its spaces and perhaps
/// a task box.
struct Marker {
    /// `-` or the digits and their delimiter.
    symbol: Range<usize>,
    /// The spaces after the symbol.
    gap: Range<usize>,
    /// `[ ]` or `[x]`.
    task: Option<Range<usize>>,
    /// The spaces after the task box.
    task_gap: Range<usize>,
    /// Where the item's text starts.
    end: usize,
}

/// The marker that starts at `start`.
fn marker(source: &str, start: usize) -> Option<Marker> {
    let bytes = source.as_bytes();
    let mut i = start;
    while bytes.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i > start {
        if !matches!(bytes.get(i), Some(b'.' | b')')) {
            return None;
        }
        i += 1;
    } else if matches!(bytes.get(i), Some(b'-' | b'*' | b'+')) {
        i += 1;
    } else {
        return None;
    }
    let symbol = start..i;
    while matches!(bytes.get(i), Some(b' ' | b'\t')) {
        i += 1;
    }
    let gap = symbol.end..i;
    let mut task = None;
    let mut task_gap = i..i;
    if bytes.get(i) == Some(&b'[')
        && matches!(bytes.get(i + 1), Some(b' ' | b'x' | b'X'))
        && bytes.get(i + 2) == Some(&b']')
        && matches!(bytes.get(i + 3), None | Some(b' ' | b'\t' | b'\n' | b'\r'))
    {
        task = Some(i..i + 3);
        i += 3;
        while matches!(bytes.get(i), Some(b' ' | b'\t')) {
            i += 1;
        }
        task_gap = task.as_ref().map_or(i..i, |t| t.end..i);
    }
    Some(Marker {
        symbol,
        gap,
        task,
        task_gap,
        end: i,
    })
}

/// The marker of the item that follows an item with marker `marker`: the
/// same symbol, one number up in an ordered list, and an unchecked box when
/// the item has one.
fn next_marker(source: &str, marker: &Marker) -> String {
    let symbol = &source[marker.symbol.clone()];
    let digits = symbol.trim_end_matches(['.', ')']);
    let mut out = match digits.parse::<u64>() {
        Ok(number) if digits.len() < symbol.len() => {
            format!("{}{}", number + 1, &symbol[digits.len()..])
        }
        _ => symbol.to_owned(),
    };
    let spaces = |range: &Range<usize>| {
        if range.is_empty() {
            " "
        } else {
            &source[range.clone()]
        }
    };
    out.push_str(spaces(&marker.gap));
    if marker.task.is_some() {
        out.push_str("[ ]");
        out.push_str(spaces(&marker.task_gap));
    }
    out
}

/// A heading written with `#`, as opposed to an underlined one.
fn atx(source: &str, block: &BlockRef) -> bool {
    source.as_bytes().get(block.span().start) == Some(&b'#')
}

/// Where the text of the block starts: after the list marker and the task
/// box, or after the `#`s, and before any delimiter the text opens with.
fn content_start(source: &str, block: &BlockRef) -> usize {
    let start = block.span().start;
    match &block.kind {
        BlockKind::ListItem { .. } => marker(source, start).map_or(start, |marker| marker.end),
        BlockKind::Heading(_) if atx(source, block) => {
            let bytes = source.as_bytes();
            let mut i = start;
            while bytes.get(i) == Some(&b'#') {
                i += 1;
            }
            while matches!(bytes.get(i), Some(b' ' | b'\t')) {
                i += 1;
            }
            i
        }
        _ => start,
    }
}

/// Where the last line of the block ends, before its line break. The span of
/// a block stops after its last character of text, so the closing delimiters
/// (`**`, a closing sequence of `#`s) are found by going on to the end of the
/// line.
fn content_end(source: &str, block: &BlockRef) -> usize {
    let span = block.span();
    let bytes = source.as_bytes();
    let mut end = span.end.min(source.len());
    if matches!(block.kind, BlockKind::Heading(_)) && !atx(source, block) {
        // An underlined heading ends with its underline, which is not text.
        let text_end = block
            .source_offset(block.content.text.len(), Bias::Upstream)
            .unwrap_or(end);
        end = line_end(source, text_end);
    } else if end > span.start && bytes[end - 1] == b'\n' {
        end -= 1;
    } else {
        end = line_end(source, end);
    }
    if end > span.start && bytes[end - 1] == b'\r' {
        end -= 1;
    }
    end.max(span.start)
}

// ---------------------------------------------------------------------------
// Enter
// ---------------------------------------------------------------------------

/// Where a block is cut, and what keeps the style of the text on both sides.
struct Split {
    /// The source byte to cut at.
    pos: usize,
    /// Delimiters that close the style on the left side.
    close: String,
    /// Delimiters that open it again on the right side.
    open: String,
}

/// The delimiter characters of emphasis, strike, code, math and scripts.
const DELIMITERS: [char; 6] = ['*', '_', '~', '`', '$', '^'];

/// Where to cut the block with the caret at rendered-text byte `at`
/// (`caret` is the same place as a byte of the source).
fn split_point(source: &str, block: &BlockRef, at: usize, caret: usize) -> Option<Split> {
    let plain = |pos| {
        Some(Split {
            pos,
            close: String::new(),
            open: String::new(),
        })
    };
    let text = &block.content.text;
    if at == 0 {
        return plain(content_start(source, block));
    }
    if at >= text.len() {
        return plain(content_end(source, block));
    }
    let runs = &block.content.runs;
    let styled = |run: &Run| run.style != Style::PLAIN || run.link.is_some();
    let base = block.span().start;
    let index = runs.partition_point(|run| run.end as usize <= at);
    let run = runs.get(index)?;
    if run.start as usize == at {
        // Between two runs: cut where no delimiter is cut in half. A run that
        // has a style closes with the delimiters that follow its text, and the
        // next one opens with the ones that come before it.
        let left = &runs[index.checked_sub(1)?];
        let (Some(up), Some(down)) = (left.source_range(), run.source_start()) else {
            return plain(caret);
        };
        let (up, down) = (base + up.end, base + down);
        let pos = if !styled(left) || up >= down || styled(run) {
            up
        } else {
            down
        };
        return plain(pos);
    }
    // Inside a run. A style is closed on the left and opened again on the
    // right when the delimiters around the run are the same on both sides.
    if styled(run)
        && run.link.is_none()
        && let Some(range) = run.source_range()
    {
        let (start, end) = (base + range.start, base + range.end);
        let before = source[..start].trim_end_matches(DELIMITERS);
        let open = &source[before.len()..start];
        let after = source[end..].trim_start_matches(DELIMITERS);
        let close = &source[end..source.len() - after.len()];
        if !open.is_empty() && open.chars().rev().eq(close.chars()) {
            return Some(Split {
                pos: caret,
                close: close.to_owned(),
                open: open.to_owned(),
            });
        }
    }
    plain(caret)
}

/// Enter: break the block at the caret. Returns whether the source changed.
pub(crate) fn enter(editor: &mut MarkdownEditor, markdown: &mut Markdown, now: Duration) -> bool {
    let caret = editor.caret;
    let rewrite = {
        let document = markdown.document();
        let source = document.source();
        match edit::caret_text(document, caret) {
            None => leave_quote(source, caret),
            Some((index, at)) => document.block(index).and_then(|block| match &block.kind {
                BlockKind::Paragraph | BlockKind::Heading(_) => {
                    split_paragraph(source, &block, at, caret)
                }
                BlockKind::ListItem { .. } => split_item(document, index, &block, at, caret),
                BlockKind::CodeBlock { .. } => code_newline(source, &block, caret),
                _ => None,
            }),
        }
    };
    rewrite.is_some_and(|rewrite| commit(editor, markdown, rewrite, now))
}

/// Enter in a paragraph or a heading: a blank line, and the prefix of the
/// line so that a quote goes on. Cutting before the first character would need
/// an empty block above, which markdown cannot hold: nothing happens.
fn split_paragraph(source: &str, block: &BlockRef, at: usize, caret: usize) -> Option<Rewrite> {
    if at == 0 && !block.content.text.is_empty() {
        return None;
    }
    let split = split_point(source, block, at, caret)?;
    let prefix = line_prefix(source, split.pos);
    let mut text = split.close;
    text.push('\n');
    text.push_str(prefix.trim_end());
    text.push('\n');
    text.push_str(prefix);
    text.push_str(&split.open);
    Some(Rewrite {
        range: split.pos..split.pos,
        caret: split.pos + text.len() + blanks_after(source, split.pos, &split.open),
        text,
    })
}

/// How many blanks the new block starts with: the parser drops them, and a
/// caret before them would be between blocks, so the caret goes after them.
fn blanks_after(source: &str, pos: usize, open: &str) -> usize {
    if !open.is_empty() {
        return 0;
    }
    let rest = &source[pos..];
    rest.len() - rest.trim_start_matches([' ', '\t']).len()
}

/// `caret`, moved past the blanks a line starts with when they hide it from
/// its block. The parser drops them, so a caret in them is between blocks,
/// where it is not drawn and where no motion starts.
pub(crate) fn into_block(document: &Document, caret: usize) -> usize {
    if edit::caret_text(document, caret).is_some() {
        return caret;
    }
    let source = document.source();
    if !blank(&source[line_start(source, caret)..caret]) {
        return caret;
    }
    let rest = &source[caret..];
    let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    if blanks > 0 && document.block_at(caret + blanks).is_some() {
        caret + blanks
    } else {
        caret
    }
}

/// Enter in a list item: a new item with the same marker where the caret is.
/// An empty item leaves the list.
fn split_item(
    document: &Document,
    index: usize,
    block: &BlockRef,
    at: usize,
    caret: usize,
) -> Option<Rewrite> {
    let source = document.source();
    let start = block.span().start;
    let marker = marker(source, start)?;
    let line = line_start(source, start);
    if block.content.text.is_empty() {
        return Some(leave_list(document, index, block, &marker, line));
    }
    let split = split_point(source, block, at, caret)?;
    let mut text = split.close;
    text.push('\n');
    text.push_str(&continuation(&source[line..start]));
    text.push_str(&next_marker(source, &marker));
    text.push_str(&split.open);
    Some(Rewrite {
        range: split.pos..split.pos,
        caret: split.pos + text.len() + blanks_after(source, split.pos, &split.open),
        text,
    })
}

/// The item that holds the one at `index`, which sits `depth` lists deep.
fn parent_item(document: &Document, index: usize, depth: u8) -> Option<BlockRef<'_>> {
    (0..index)
        .rev()
        .filter_map(|i| document.block(i))
        .find(|block| matches!(block.kind, BlockKind::ListItem { .. }) && block.list_depth < depth)
}

/// Enter in an empty item: a nested one goes out a level (it takes the
/// indentation of its parent), any other leaves the list and its line is
/// left empty, with a blank line on each side so that the next block is not
/// read as a part of the list.
fn leave_list(
    document: &Document,
    index: usize,
    block: &BlockRef,
    marker: &Marker,
    line: usize,
) -> Rewrite {
    let source = document.source();
    let start = block.span().start;
    if block.list_depth > 1
        && let Some(parent) = parent_item(document, index, block.list_depth)
    {
        let parent_start = parent.span().start;
        let prefix = continuation(&source[line_start(source, parent_start)..parent_start]);
        return Rewrite {
            range: line..start,
            caret: line + prefix.len() + (marker.end - start),
            text: prefix,
        };
    }
    let prefix = &source[line..start];
    let end = content_end(source, block).max(marker.end);
    isolated(source, line, end, prefix, prefix.len(), prefix.trim_end())
}

/// Enter in a code block: a literal line break, indented like the block. On
/// the fences, which are not code, nothing happens.
fn code_newline(source: &str, block: &BlockRef, caret: usize) -> Option<Rewrite> {
    let start = block.span().start;
    let first_end = line_end(source, start);
    if fence(&source[start..first_end]) {
        if caret <= first_end {
            return None;
        }
        let end = content_end(source, block);
        let last = line_start(source, end);
        if last > first_end && fence(&source[last..end]) && caret > last {
            return None;
        }
    }
    let line = line_start(source, start);
    let text = format!("\n{}", continuation(&source[line..start]));
    Some(Rewrite {
        range: caret..caret,
        caret: caret + text.len(),
        text,
    })
}

/// Whether `line` opens or closes a fenced code block.
fn fence(line: &str) -> bool {
    let text = line.trim_start();
    line.len() - text.len() < 4 && (text.starts_with("```") || text.starts_with("~~~"))
}

/// Enter on the empty line a quote leaves behind (`> ` and nothing else):
/// take off its last `>`, so the next text is outside the quote.
fn leave_quote(source: &str, caret: usize) -> Option<Rewrite> {
    let line = line_start(source, caret);
    let before = &source[line..caret];
    if !blank(before) {
        return None;
    }
    let mark = before.rfind('>')?;
    let end = mark + 1 + usize::from(before[mark + 1..].starts_with(' '));
    Some(Rewrite {
        range: line + mark..line + end,
        text: String::new(),
        caret: line + mark,
    })
}

// ---------------------------------------------------------------------------
// Backspace and Delete at the edge of a block
// ---------------------------------------------------------------------------

/// The lines `line..end` of a block replaced by `body`, with a blank line
/// (`blank_line`, which keeps the quote marks of the context) on each side
/// that lacks one: a line that was part of a list or a heading must not stay
/// glued to the text around it, which would read it as a continuation.
fn isolated(
    source: &str,
    line: usize,
    end: usize,
    body: &str,
    caret_in_body: usize,
    blank_line: &str,
) -> Rewrite {
    let before = if blank_before(source, line) {
        String::new()
    } else {
        format!("{blank_line}\n")
    };
    let after = if blank_after(source, end) {
        String::new()
    } else {
        format!("\n{blank_line}")
    };
    Rewrite {
        range: line..end,
        caret: line + before.len() + caret_in_body,
        text: format!("{before}{body}{after}"),
    }
}

/// A list item or a heading as a paragraph: its marker and the indentation
/// before it are gone.
fn demote(source: &str, block: &BlockRef) -> Rewrite {
    let start = block.span().start;
    let line = line_start(source, start);
    let prefix = &source[line..start];
    let (keep, blank_line) = if prefix.chars().all(|c| matches!(c, ' ' | '\t' | '>')) {
        // Indentation goes; quote marks stay, with the one space after them.
        let marks = prefix.trim_end_matches([' ', '\t']);
        let keep = if marks.ends_with('>') && prefix.len() > marks.len() {
            &prefix[..=marks.len()]
        } else {
            marks
        };
        (keep, keep.trim_end())
    } else {
        // An item on the line of the marker of its parent: the parent's
        // marker is not ours to take away.
        (prefix, "")
    };
    let from = content_start(source, block);
    let end = content_end(source, block).max(from);
    let body = format!("{keep}{}", &source[from..end]);
    isolated(source, line, end, &body, keep.len(), blank_line)
}

/// `line` without the innermost `>` of its prefix, and the space after it.
fn strip_quote(line: &str) -> String {
    let prefix = line.len() - line.trim_start_matches([' ', '\t', '>']).len();
    match line[..prefix].rfind('>') {
        Some(mark) => {
            let space = usize::from(line[mark + 1..].starts_with(' '));
            format!("{}{}", &line[..mark], &line[mark + 1 + space..])
        }
        None => line.to_owned(),
    }
}

/// A block of a quote taken out of its innermost quote: every line of it
/// loses one `>`.
fn unquote(source: &str, block: &BlockRef) -> Rewrite {
    let start = block.span().start;
    let line = line_start(source, start);
    let end = content_end(source, block).max(start);
    let mut body = String::new();
    let mut caret = 0;
    for (n, text) in source[line..end].split_inclusive('\n').enumerate() {
        let stripped = strip_quote(text);
        if n == 0 {
            caret = (start - line).saturating_sub(text.len() - stripped.len());
        }
        body.push_str(&stripped);
    }
    let blank_line = body[..caret].trim_end().to_owned();
    isolated(source, line, end, &body, caret, &blank_line)
}

/// The block at `index` appended to the text block `before` it.
fn join(source: &str, before: &BlockRef, block: &BlockRef) -> Option<Rewrite> {
    let at = content_end(source, before);
    let start = content_start(source, block);
    if at > start {
        return None;
    }
    let end = content_end(source, block).max(start);
    let rest = &source[start..end];
    // The lines after the first of a quote or a list item carry a prefix
    // that is not text, which would not be one outside it.
    if (block.quote_depth > 0 || block.list_depth > 0) && rest.contains('\n') {
        let text = rest
            .split_inclusive('\n')
            .enumerate()
            .map(|(n, line)| {
                if n == 0 {
                    line
                } else {
                    line.trim_start_matches([' ', '\t', '>'])
                }
            })
            .collect();
        return Some(Rewrite {
            range: at..end,
            text,
            caret: at,
        });
    }
    Some(Rewrite {
        range: at..start,
        text: String::new(),
        caret: at,
    })
}

/// Whether the block has text a join can read and write.
fn has_text(block: &BlockRef) -> bool {
    matches!(
        block.kind,
        BlockKind::Paragraph | BlockKind::Heading(_) | BlockKind::ListItem { .. }
    )
}

/// Take away the rule at `index`: its lines up to the next block's.
fn remove_rule(document: &Document, index: usize, caret: Option<usize>) -> Option<Rewrite> {
    let source = document.source();
    let start = line_start(source, document.block(index)?.span().start);
    let end = document
        .block(index + 1)
        .map_or(source.len(), |next| line_start(source, next.span().start));
    Some(Rewrite {
        range: start..end,
        text: String::new(),
        caret: caret.unwrap_or(start),
    })
}

/// Backspace at the start of a block, which takes off what is not text
/// before it joins the block to the one above it. `None` when the caret is
/// not at the start of a block, and the key is a plain one-character delete.
pub(crate) fn backspace(
    editor: &mut MarkdownEditor,
    markdown: &mut Markdown,
    now: Duration,
) -> Option<bool> {
    let caret = editor.caret;
    let rewrite = {
        let document = markdown.document();
        let source = document.source();
        let Some((index, at)) = edit::caret_text(document, caret) else {
            return Some(
                gap_backspace(source, caret).is_some_and(|r| commit(editor, markdown, r, now)),
            );
        };
        if at != 0 {
            return None;
        }
        let block = document.block(index)?;
        match &block.kind {
            BlockKind::ListItem { .. } => Some(demote(source, &block)),
            BlockKind::Heading(_) if atx(source, &block) => Some(demote(source, &block)),
            BlockKind::Paragraph | BlockKind::Heading(_) if block.quote_depth > 0 => {
                Some(unquote(source, &block))
            }
            BlockKind::Paragraph | BlockKind::Heading(_) => {
                index.checked_sub(1).and_then(|before| {
                    let above = document.block(before)?;
                    if has_text(&above) {
                        join(source, &above, &block)
                    } else if above.kind == BlockKind::Rule {
                        remove_rule(document, before, None)
                    } else {
                        None
                    }
                })
            }
            _ => return None,
        }
    };
    Some(rewrite.is_some_and(|rewrite| commit(editor, markdown, rewrite, now)))
}

/// Delete at the end of a block: the next block is appended to it. `None`
/// when the caret is not at the end of a block, and the key is a plain
/// one-character delete.
pub(crate) fn delete(
    editor: &mut MarkdownEditor,
    markdown: &mut Markdown,
    now: Duration,
) -> Option<bool> {
    let caret = editor.caret;
    let rewrite = {
        let document = markdown.document();
        let source = document.source();
        let Some((index, at)) = edit::caret_text(document, caret) else {
            return Some(
                gap_delete(source, caret).is_some_and(|r| commit(editor, markdown, r, now)),
            );
        };
        let block = document.block(index)?;
        if !has_text(&block) || at != block.content.text.len() {
            return None;
        }
        document.block(index + 1).and_then(|next| {
            if has_text(&next) {
                join(source, &block, &next)
            } else if next.kind == BlockKind::Rule {
                remove_rule(document, index + 1, Some(caret))
            } else {
                None
            }
        })
    };
    Some(rewrite.is_some_and(|rewrite| commit(editor, markdown, rewrite, now)))
}

/// Backspace on a blank line between blocks (what Enter at the end of a block
/// leaves): the blank lines go and the caret is back at the end of the text
/// above.
fn gap_backspace(source: &str, caret: usize) -> Option<Rewrite> {
    let line = line_start(source, caret);
    if !blank(&source[line..caret]) {
        return None;
    }
    let mut first = line;
    while first > 0 {
        let above = line_start(source, first - 1);
        if !blank(&source[above..first - 1]) {
            break;
        }
        first = above;
    }
    let mut from = first.saturating_sub(1);
    if from > 0 && source.as_bytes()[from - 1] == b'\r' {
        from -= 1;
    }
    Some(Rewrite {
        range: from..caret,
        text: String::new(),
        caret: from,
    })
}

/// Delete on a blank line between blocks: the blank lines after the caret
/// go, and the next text comes up to it.
fn gap_delete(source: &str, caret: usize) -> Option<Rewrite> {
    let mut end = line_end(source, caret);
    if !blank(&source[caret..end]) {
        return None;
    }
    while end < source.len() {
        let next = end + 1;
        let next_end = line_end(source, next);
        if !blank(&source[next..next_end]) {
            break;
        }
        end = next_end;
    }
    Some(Rewrite {
        range: caret..(end + 1).min(source.len()),
        text: String::new(),
        caret,
    })
}
