//! Moving the caret by lines and across blocks.
//!
//! The caret lives in the markdown source, but a vertical move is a question
//! about glyphs: which line is above, and which character of it is at the
//! same horizontal position. Both are answered on the Parley layout Bevy
//! computed for each block's text, and the answer is turned back into a
//! source byte with the source map, like every other motion.
//!
//! - **Sticky column.** Moving up or down remembers where the caret started
//!   horizontally (in window pixels, so that blocks with another indent
//!   line up on screen) and keeps aiming at it through shorter lines, until
//!   the caret changes for some other reason: typing, a click, a horizontal
//!   move. The memory is tied to the caret's byte, so anything that moves
//!   the caret forgets it without having to say so.
//! - **Across blocks.** Past the first or last line of a block the caret
//!   enters the previous or next block that has editable text (a divider or a
//!   table has none and is skipped), on its last or first line. Left and
//!   Right do the same from the start or the end of a block.
//! - **No layout.** A block scrolled out of the window has no node and so no
//!   layout. Its lines are then the `\n`-separated lines of its text and the
//!   column is counted in characters: an estimate, but the caret still moves
//!   and nothing panics.

use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy::text::TextBrush;
use bevy::ui::{ComputedNode, OverflowAxis, UiGlobalTransform};
use bevy::window::PrimaryWindow;
use bevy_md::{BlockText, Markdown};
use bevy_md_core::Document;
use parley::{Affinity, Cursor, Layout};
use std::ops::Range;

use crate::edit;
use crate::{BlockTexts, MarkdownEditor, Motion, clamp_to_boundary};

/// Height of a line, in pixels, when the caret's block has no layout to ask.
const FALLBACK_LINE_HEIGHT: f32 = 20.0;
/// How many lines a page is when no viewport can be found.
const FALLBACK_PAGE_LINES: usize = 30;
/// The most lines one Page Up or Page Down moves.
const MAX_PAGE_LINES: usize = 1_000;

/// A caret movement that is not a step left or right.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Navigate {
    /// One line up.
    Up,
    /// One line down.
    Down,
    /// A viewport up.
    PageUp,
    /// A viewport down.
    PageDown,
    /// The start of the document.
    DocumentStart,
    /// The end of the document.
    DocumentEnd,
}

/// Where a run of vertical moves aims horizontally.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Preferred {
    /// Window position of the caret when the run started, in the space of
    /// [`UiGlobalTransform`]. `None` when it started in a block with no
    /// layout.
    x: Option<f32>,
    /// Characters between the start of the caret's line and the caret: what
    /// is aimed at in a block that has no layout.
    column: usize,
    /// The caret this was measured for: when the caret is anywhere else, the
    /// memory is stale.
    caret: usize,
}

/// A place in the document: a block and a rendered-text byte of it.
#[derive(Clone, Copy, Debug)]
struct Spot {
    block: usize,
    at: usize,
}

/// A block's layout with the node that places it in the window.
struct View<'a> {
    layout: &'a Layout<TextBrush>,
    node: &'a ComputedNode,
    transform: &'a UiGlobalTransform,
}

impl View<'_> {
    /// Window x of the point `(x, y)` of the layout.
    fn global_x(&self, x: f32, y: f32) -> f32 {
        let point = self.node.content_box().min + Vec2::new(x, y);
        self.transform.transform_point2(point).x
    }

    /// The layout x that is at window x `global`, on the line at layout `y`.
    fn local_x(&self, global: f32, y: f32) -> Option<f32> {
        let content = self.node.content_box();
        let at_y = self
            .transform
            .transform_point2(content.min + Vec2::new(0.0, y))
            .y;
        let inverse = self.transform.try_inverse()?;
        Some(inverse.transform_point2(Vec2::new(global, at_y)).x - content.min.x)
    }
}

/// Everything a movement reads: block layouts and where they are on screen.
#[derive(SystemParam)]
pub(crate) struct Navigator<'w, 's> {
    blocks: BlockTexts<'w, 's>,
    nodes: Query<'w, 's, (&'static ComputedNode, &'static UiGlobalTransform), With<BlockText>>,
    parents: Query<'w, 's, &'static ChildOf>,
    clips: Query<'w, 's, (&'static Node, &'static ComputedNode)>,
    windows: Query<'w, 's, &'static Window, With<PrimaryWindow>>,
}

impl Navigator<'_, '_> {
    /// Move the caret of `editor` as `what` says. With `extend` the anchor
    /// stays and the selection grows, otherwise the selection collapses.
    /// Returns whether the caret or the selection changed.
    pub(crate) fn apply(
        &self,
        editor: &mut MarkdownEditor,
        markdown: &Markdown,
        what: Navigate,
        extend: bool,
    ) -> bool {
        let target = match what {
            Navigate::DocumentStart => document_edge(markdown.document(), false).map(|c| (c, None)),
            Navigate::DocumentEnd => document_edge(markdown.document(), true).map(|c| (c, None)),
            _ => self.vertical(editor, markdown, what),
        };
        let Some((caret, preferred)) = target else {
            return false;
        };
        let before = (editor.caret, editor.anchor);
        let anchor = editor.anchor;
        if extend {
            editor.set_selection(markdown, anchor, caret);
        } else {
            editor.set_caret(markdown, caret);
        }
        let caret = editor.caret;
        editor.preferred = preferred.map(|preferred: Preferred| Preferred { caret, ..preferred });
        (editor.caret, editor.anchor) != before
    }

    /// The source caret `what` (a line or page move) leads to, and where to
    /// keep aiming horizontally; `None` for the second when the move ended
    /// at the edge of the document.
    fn vertical(
        &self,
        editor: &MarkdownEditor,
        markdown: &Markdown,
        what: Navigate,
    ) -> Option<(usize, Option<Preferred>)> {
        let document = markdown.document();
        let forward = matches!(what, Navigate::Down | Navigate::PageDown);
        let page = matches!(what, Navigate::PageUp | Navigate::PageDown);
        let remembered = editor
            .preferred
            .filter(|preferred| preferred.caret == editor.caret);

        let (spot, preferred, edge) =
            if let Some((block, at)) = edit::caret_text(document, editor.caret) {
                let start = Spot { block, at };
                let preferred =
                    remembered.unwrap_or_else(|| self.measure(markdown, start, editor.caret));
                let lines = if page {
                    self.page_lines(markdown, start)
                } else {
                    1
                };
                let mut spot = start;
                let mut edge = false;
                for _ in 0..lines {
                    if let Some(next) = self.step(markdown, spot, forward, &preferred) {
                        spot = next;
                    } else {
                        // No line beyond: go to the end of this one.
                        edge = true;
                        spot.at = if forward {
                            document.block(spot.block)?.content.text.len()
                        } else {
                            0
                        };
                        break;
                    }
                }
                (spot, preferred, edge)
            } else {
                // The caret is between blocks (on a blank line): the nearest
                // block in that direction is where it goes.
                let after = blocks_starting_at_or_before(document, editor.caret);
                let first = if forward {
                    after
                } else {
                    after.checked_sub(1)?
                };
                let block = editable_from(document, Some(first), forward)?;
                let preferred = Preferred {
                    x: None,
                    column: 0,
                    caret: editor.caret,
                };
                let spot = self.enter(markdown, block, forward, &preferred);
                (spot, preferred, false)
            };
        let caret = document
            .block(spot.block)
            .and_then(|block| edit::source_caret(&block, spot.at))?;
        Some((caret, (!edge).then_some(preferred)))
    }

    /// The layout of block `block` with its node, when it has both.
    fn view(&self, markdown: &Markdown, block: usize) -> Option<View<'_>> {
        let text = self.blocks.entity(markdown, block)?;
        let layout = self.blocks.texts.get(text).ok()?.buffer();
        let (node, transform) = self.nodes.get(text).ok()?;
        Some(View {
            layout,
            node,
            transform,
        })
    }

    /// Where `spot` is horizontally.
    fn measure(&self, markdown: &Markdown, spot: Spot, caret: usize) -> Preferred {
        let text = block_text(markdown.document(), spot.block);
        let mut preferred = Preferred {
            x: None,
            column: 0,
            caret,
        };
        if let Some(view) = self.view(markdown, spot.block) {
            let layout = view.layout;
            let line = line_of(layout, spot.at);
            let bounds = Cursor::from_byte_index(layout, spot.at, Affinity::Downstream)
                .geometry(layout, 0.0);
            let y = bounds.y0.midpoint(bounds.y1) as f32;
            preferred.x = Some(view.global_x(bounds.x0 as f32, y));
            let start = layout.get(line).map_or(0, |line| line.text_range().start);
            preferred.column = count_chars(text, start, spot.at);
        } else {
            let start = text.get(..spot.at).map_or(0, |before| {
                before.rfind('\n').map_or(0, |newline| newline + 1)
            });
            preferred.column = count_chars(text, start, spot.at);
        }
        preferred
    }

    /// The spot one line above or below `spot`, or `None` at the first or
    /// last line of the document.
    fn step(
        &self,
        markdown: &Markdown,
        spot: Spot,
        forward: bool,
        preferred: &Preferred,
    ) -> Option<Spot> {
        let document = markdown.document();
        let text = block_text(document, spot.block);
        if let Some(view) = self.view(markdown, spot.block) {
            let line = line_of(view.layout, spot.at);
            let target = if forward {
                Some(line + 1).filter(|target| *target < view.layout.len())
            } else {
                line.checked_sub(1)
            };
            if let Some(target) = target {
                let at = at_line(&view, text, target, preferred);
                return Some(Spot { at, ..spot });
            }
        } else if let Some(range) = adjacent_text_line(text, spot.at, forward) {
            let at = at_column(text, range, preferred.column);
            return Some(Spot { at, ..spot });
        }
        let next = if forward {
            spot.block.checked_add(1)
        } else {
            spot.block.checked_sub(1)
        };
        let block = editable_from(document, next, forward)?;
        Some(self.enter(markdown, block, forward, preferred))
    }

    /// The spot on the first line of block `block` (moving forward) or its
    /// last (moving back), at the horizontal position aimed at.
    fn enter(
        &self,
        markdown: &Markdown,
        block: usize,
        forward: bool,
        preferred: &Preferred,
    ) -> Spot {
        let text = block_text(markdown.document(), block);
        let at = if let Some(view) = self.view(markdown, block) {
            let line = if forward {
                0
            } else {
                view.layout.len().saturating_sub(1)
            };
            at_line(&view, text, line, preferred)
        } else {
            let range = if forward {
                first_text_line(text)
            } else {
                last_text_line(text)
            };
            at_column(text, range, preferred.column)
        };
        Spot { block, at }
    }

    /// How many lines a Page Up or Page Down moves from `spot`: the height
    /// of the viewport in lines of the caret's block, less one for context.
    fn page_lines(&self, markdown: &Markdown, spot: Spot) -> usize {
        let line_height = self
            .view(markdown, spot.block)
            .and_then(|view| {
                let line = view.layout.get(line_of(view.layout, spot.at))?;
                Some(line.metrics().line_height)
            })
            .filter(|height| *height > 0.0)
            .unwrap_or(FALLBACK_LINE_HEIGHT);
        let Some(viewport) = self.viewport_height(markdown) else {
            return FALLBACK_PAGE_LINES;
        };
        // A positive, finite count of lines, capped right after.
        #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
        let lines = (viewport / line_height).floor().max(0.0) as usize;
        lines.saturating_sub(1).clamp(1, MAX_PAGE_LINES)
    }

    /// The height, in physical pixels, of what shows the document: the
    /// closest ancestor of its blocks that clips vertically, or the window.
    fn viewport_height(&self, markdown: &Markdown) -> Option<f32> {
        let (_, mut current) = markdown.live_blocks().next()?;
        while let Ok(parent) = self.parents.get(current) {
            current = parent.parent();
            if let Ok((node, computed)) = self.clips.get(current)
                && node.overflow.y != OverflowAxis::Visible
                && computed.size().y > 0.0
            {
                return Some(computed.size().y);
            }
        }
        let window = self.windows.single().ok()?;
        // A window is never anywhere near 2^24 pixels tall.
        #[allow(clippy::cast_precision_loss)]
        Some(window.physical_height() as f32)
    }
}

/// The source caret a Left or Right at the edge of block `block` leads to:
/// the end of the previous block, or the start of the next one. `None` for
/// any other motion and at the edges of the document.
pub(crate) fn across_blocks(document: &Document, block: usize, motion: Motion) -> Option<usize> {
    match motion {
        Motion::Left => {
            let previous = editable_from(document, block.checked_sub(1), false)?;
            let block = document.block(previous)?;
            edit::source_caret(&block, block.content.text.len())
        }
        Motion::Right => {
            let next = editable_from(document, block.checked_add(1), true)?;
            edit::source_caret(&document.block(next)?, 0)
        }
        Motion::Home | Motion::End => None,
    }
}

/// The source caret at the very start or end of the document's text.
fn document_edge(document: &Document, end: bool) -> Option<usize> {
    if end {
        let block = editable_from(document, document.len().checked_sub(1), false)?;
        let block = document.block(block)?;
        edit::source_caret(&block, block.content.text.len())
    } else {
        let block = editable_from(document, Some(0), true)?;
        edit::source_caret(&document.block(block)?, 0)
    }
}

/// The first block from `from` on (backwards when `forward` is false) that
/// can hold the caret: one with text that maps to the source.
fn editable_from(document: &Document, from: Option<usize>, forward: bool) -> Option<usize> {
    let mut index = from?;
    loop {
        let block = document.block(index)?;
        if edit::source_caret(&block, 0).is_some() {
            return Some(index);
        }
        index = if forward {
            index.checked_add(1)?
        } else {
            index.checked_sub(1)?
        };
    }
}

/// How many blocks start at or before source byte `offset`.
fn blocks_starting_at_or_before(document: &Document, offset: usize) -> usize {
    let (mut low, mut high) = (0, document.len());
    while low < high {
        let middle = low + (high - low) / 2;
        if document
            .block(middle)
            .is_some_and(|block| block.span().start <= offset)
        {
            low = middle + 1;
        } else {
            high = middle;
        }
    }
    low
}

/// The rendered text of block `block`, empty for a block that is not there.
fn block_text(document: &Document, block: usize) -> &str {
    document
        .block(block)
        .map_or("", |block| block.block().content.text.as_str())
}

/// The index of the line of `layout` that has the text byte `at`. The caret
/// where a soft wrap happens is on the later line, as it is drawn.
fn line_of(layout: &Layout<TextBrush>, at: usize) -> usize {
    layout
        .lines()
        .position(|line| at < line.text_range().end)
        .unwrap_or(layout.len().saturating_sub(1))
}

/// How many characters of `text` lie between bytes `start` and `end`.
fn count_chars(text: &str, start: usize, end: usize) -> usize {
    text.get(start..end).map_or(0, |part| part.chars().count())
}

/// The text byte of line `line` of `view` at the horizontal position
/// `preferred` aims at: by pixels when both are known, otherwise by the
/// column in characters.
fn at_line(view: &View, text: &str, line: usize, preferred: &Preferred) -> usize {
    let layout = view.layout;
    let Some(row) = layout.get(line) else {
        return 0;
    };
    let range = row.text_range();
    let metrics = row.metrics();
    let y = metrics.block_min_coord.midpoint(metrics.block_max_coord);
    let at = match preferred.x.and_then(|global| view.local_x(global, y)) {
        Some(x) => Cursor::from_point(layout, x, y).index(),
        None => at_column(text, range.clone(), preferred.column),
    };
    let at = clamp_to_boundary(text, at.clamp(range.start, range.end));
    if at >= range.end && line + 1 < layout.len() {
        // The end of a wrapped line is the start of the next one: stay on
        // this line, before the space or the line break that ends it.
        text.get(..range.end)
            .and_then(|before| before.char_indices().next_back())
            .map_or(at, |(start, _)| start.max(range.start))
    } else {
        at
    }
}

/// The byte `column` characters into `range` of `text`, or its end.
fn at_column(text: &str, range: Range<usize>, column: usize) -> usize {
    let Some(line) = text.get(range.clone()) else {
        return range.start.min(text.len());
    };
    line.char_indices()
        .nth(column)
        .map_or(range.end, |(offset, _)| range.start + offset)
}

/// The `\n`-separated line of `text` that is first.
fn first_text_line(text: &str) -> Range<usize> {
    0..text.find('\n').unwrap_or(text.len())
}

/// The `\n`-separated line of `text` that is last.
fn last_text_line(text: &str) -> Range<usize> {
    text.rfind('\n').map_or(0, |newline| newline + 1)..text.len()
}

/// The `\n`-separated line of `text` before or after the one holding byte
/// `at`, when there is one.
fn adjacent_text_line(text: &str, at: usize, forward: bool) -> Option<Range<usize>> {
    let before = text.get(..at)?;
    let start = before.rfind('\n').map_or(0, |newline| newline + 1);
    if forward {
        let end = at + text[at..].find('\n')?;
        let next = end + 1;
        let next_end = text[next..].find('\n').map_or(text.len(), |n| next + n);
        Some(next..next_end)
    } else {
        let end = start.checked_sub(1)?;
        let previous = text[..end].rfind('\n').map_or(0, |newline| newline + 1);
        Some(previous..end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_lines_are_the_ones_between_line_breaks() {
        let text = "ab\ncde\n\nf";
        assert_eq!(first_text_line(text), 0..2);
        assert_eq!(last_text_line(text), 8..9);
        assert_eq!(adjacent_text_line(text, 1, true), Some(3..6));
        assert_eq!(adjacent_text_line(text, 4, true), Some(7..7));
        assert_eq!(adjacent_text_line(text, 7, true), Some(8..9));
        assert_eq!(adjacent_text_line(text, 8, true), None);
        assert_eq!(adjacent_text_line(text, 4, false), Some(0..2));
        assert_eq!(adjacent_text_line(text, 1, false), None);
        assert_eq!(adjacent_text_line("", 0, true), None);
    }

    #[test]
    fn a_column_is_counted_in_characters_and_stops_at_the_end_of_the_line() {
        let text = "aç日z\nabcdef";
        let line = 0..text.find('\n').unwrap();
        assert_eq!(at_column(text, line.clone(), 0), 0);
        assert_eq!(at_column(text, line.clone(), 2), 3);
        assert_eq!(at_column(text, line.clone(), 3), 6);
        assert_eq!(at_column(text, line.clone(), 99), line.end);
    }
}
