//! Things that sit at source offsets, kept as distances so an edit moves none
//! of them: a [`SumTree`] of items that each carry the gap from the previous
//! one. The block list of a [`Document`](crate::Document) has the same shape;
//! this is the version for the sparse things next to it (definitions).

use std::ops::{Add, Range};

use crate::sum_tree::{Dimension, Item, SumTree, Summary};

/// The sum of the gaps of a run of items: where the last one starts.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Gaps(pub isize);

impl Summary for Gaps {
    fn add(&mut self, other: &Self) {
        self.0 += other.0;
    }
}

/// The running sum of the gaps: a source offset.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub(crate) struct Start(pub isize);

impl Add for Start {
    type Output = Start;

    fn add(self, other: Start) -> Start {
        Start(self.0 + other.0)
    }
}

impl Dimension<Gaps> for Start {
    fn from_summary(summary: &Gaps) -> Self {
        Start(summary.0)
    }
}

#[derive(Clone, Debug)]
struct Placed<V: Clone + std::fmt::Debug> {
    /// Bytes from the previous item's start (from 0 for the first).
    gap: isize,
    value: V,
}

impl<V: Clone + std::fmt::Debug> Item for Placed<V> {
    type Summary = Gaps;

    fn summary(&self) -> Gaps {
        Gaps(self.gap)
    }
}

/// Values at source offsets, ordered by offset.
#[derive(Clone, Debug)]
pub(crate) struct Positioned<V: Clone + std::fmt::Debug> {
    tree: SumTree<Placed<V>>,
}

impl<V: Clone + std::fmt::Debug> Default for Positioned<V> {
    fn default() -> Self {
        Self {
            tree: SumTree::new(),
        }
    }
}

impl<V: Clone + std::fmt::Debug> Positioned<V> {
    /// A tree of `items`, which must be in order of offset.
    pub fn from_items(items: Vec<(usize, V)>) -> Self {
        let mut previous = 0isize;
        let entries = items
            .into_iter()
            .map(|(pos, value)| {
                let start = pos.cast_signed();
                debug_assert!(start >= previous, "items in order");
                let gap = start - previous;
                previous = start;
                Placed { gap, value }
            })
            .collect();
        Self {
            tree: SumTree::from_vec(entries),
        }
    }

    /// Indices of the items starting in `range`.
    fn indices(&self, range: &Range<usize>) -> Range<usize> {
        if self.tree.is_empty() || range.is_empty() {
            return 0..0;
        }
        let lo = if range.start == 0 {
            0
        } else {
            self.tree.seek(Start(range.start.cast_signed() - 1))
        };
        let hi = self.tree.seek(Start(range.end.cast_signed() - 1));
        lo..hi.max(lo)
    }

    /// The items starting in `range`, with their offsets.
    pub fn in_range(&self, range: &Range<usize>) -> Vec<(usize, &V)> {
        let indices = self.indices(range);
        if indices.is_empty() {
            return Vec::new();
        }
        let mut at = self.tree.prefix::<Start>(indices.start).0;
        self.tree
            .iter_from(indices.start)
            .take(indices.len())
            .map(|entry| {
                at += entry.gap;
                (at.cast_unsigned(), &entry.value)
            })
            .collect()
    }

    /// All the items with their offsets, in order.
    pub fn iter(&self) -> impl Iterator<Item = (usize, &V)> {
        let mut at = 0isize;
        self.tree.iter().map(move |entry| {
            at += entry.gap;
            (at.cast_unsigned(), &entry.value)
        })
    }

    /// Replace the items starting in `old` with `new` (in order, inside the
    /// new extent of the range), and move everything after the range by
    /// `delta` bytes, which is how much the text in `old` grew.
    pub fn replace(&mut self, old: &Range<usize>, delta: isize, new: Vec<(usize, V)>) {
        if self.tree.is_empty() {
            if !new.is_empty() {
                *self = Self::from_items(new);
            }
            return;
        }
        let indices = self.indices(old);
        let (a, b) = (indices.start, indices.end);
        let before = if a == 0 {
            0
        } else {
            self.tree.prefix::<Start>(a).0
        };
        let next_start = (b < self.tree.len()).then(|| self.tree.prefix::<Start>(b + 1).0);
        let mut previous = before;
        let inserted = new.len();
        let entries: Vec<Placed<V>> = new
            .into_iter()
            .map(|(pos, value)| {
                let start = pos.cast_signed();
                debug_assert!(start >= previous, "items in order");
                let gap = start - previous;
                previous = start;
                Placed { gap, value }
            })
            .collect();
        self.tree.splice(a..b, entries);
        if let Some(next) = next_start {
            let gap = next + delta - previous;
            debug_assert!(gap >= 0, "items in order");
            self.tree.update(a + inserted, |entry| entry.gap = gap);
        }
    }
}
