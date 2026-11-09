//! The height of every block, in a sum tree: block to pixel offset and pixel
//! offset to block in `O(log n)`, which is what virtualization and the scroll
//! anchor are built on.
//!
//! A height is an estimate until the block has been laid out once; then the
//! measured one takes its place.

use std::ops::{Add, Range};

use bevy_md_core::sum_tree::{Dimension, Item, SumTree, Summary};

/// The height of one block, margins included.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Height {
    pub px: f32,
    /// Whether `px` was measured from a layout rather than estimated.
    pub measured: bool,
}

impl Height {
    pub(crate) fn estimated(px: f32) -> Self {
        Self {
            px,
            measured: false,
        }
    }
}

/// The sum of heights, in `f64` so that 100 000 blocks add up without drift.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Total(f64);

impl Summary for Total {
    fn add(&mut self, other: &Self) {
        self.0 += other.0;
    }
}

impl Item for Height {
    type Summary = Total;

    fn summary(&self) -> Total {
        Total(f64::from(self.px))
    }
}

/// A pixel offset from the top of the first block.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub(crate) struct Y(pub f64);

impl Add for Y {
    type Output = Y;

    fn add(self, other: Y) -> Y {
        Y(self.0 + other.0)
    }
}

impl Dimension<Total> for Y {
    fn from_summary(summary: &Total) -> Self {
        Y(summary.0)
    }
}

/// Heights of all the blocks of a document.
#[derive(Clone, Debug, Default)]
pub(crate) struct Heights {
    tree: SumTree<Height>,
}

impl Heights {
    pub(crate) fn len(&self) -> usize {
        self.tree.len()
    }

    /// Height of everything.
    pub(crate) fn total(&self) -> f64 {
        Y::from_summary(self.tree.summary()).0
    }

    /// Offset of the top of block `index`; of the end when `index == len`.
    pub(crate) fn top(&self, index: usize) -> f64 {
        self.tree.prefix::<Y>(index).0
    }

    /// The offset of the bottom of blocks `range`: their summed height.
    pub(crate) fn span(&self, range: Range<usize>) -> f64 {
        self.top(range.end) - self.top(range.start)
    }

    pub(crate) fn get(&self, index: usize) -> Option<Height> {
        self.tree.get(index).copied()
    }

    /// The block holding offset `y`: the first one whose bottom is past it.
    /// Offsets before the first block give 0, past the end the last block.
    pub(crate) fn at(&self, y: f64) -> usize {
        if self.tree.is_empty() {
            return 0;
        }
        self.tree.seek(Y(y)).min(self.tree.len() - 1)
    }

    /// An index of estimated heights, one per item of `px`. Built where it is
    /// cheap to build (a worker thread) and handed over whole, to be hung
    /// into the live index by [`Heights::splice_built`].
    pub(crate) fn estimated(px: impl Iterator<Item = f32>) -> Self {
        Self {
            tree: SumTree::from_vec(px.map(Height::estimated).collect()),
        }
    }

    /// Replace the heights `range` with the ones of `built`, in
    /// `O(log n)`: no height of `built` is visited. The replaced heights
    /// come back as an index of their own, to be dropped by someone else.
    pub(crate) fn splice_built(&mut self, range: Range<usize>, built: Heights) -> Heights {
        Heights {
            tree: self.tree.splice_tree(range, built.tree),
        }
    }

    /// Replace the heights `range` with `new`.
    pub(crate) fn splice(&mut self, range: Range<usize>, new: Vec<Height>) {
        if self.tree.is_empty() {
            self.tree = SumTree::from_vec(new);
        } else {
            self.tree.splice(range, new);
        }
    }

    /// Set the height of block `index`. Returns whether it moved by more
    /// than a quarter pixel (or had not been measured yet).
    pub(crate) fn set(&mut self, index: usize, height: Height) -> bool {
        let Some(old) = self.get(index) else {
            return false;
        };
        if old.measured == height.measured && (old.px - height.px).abs() <= 0.25 {
            return false;
        }
        self.tree.update(index, |h| *h = height);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heights(px: &[f32]) -> Heights {
        let mut heights = Heights::default();
        heights.splice(0..0, px.iter().copied().map(Height::estimated).collect());
        heights
    }

    #[test]
    fn offsets_and_blocks_agree() {
        let h = heights(&[10.0, 20.0, 30.0]);
        assert_eq!(h.total(), 60.0);
        assert_eq!(h.top(0), 0.0);
        assert_eq!(h.top(2), 30.0);
        assert_eq!(h.top(3), 60.0);
        assert_eq!(h.at(-5.0), 0);
        assert_eq!(h.at(0.0), 0);
        assert_eq!(h.at(9.9), 0);
        assert_eq!(h.at(10.0), 1);
        assert_eq!(h.at(29.9), 1);
        assert_eq!(h.at(30.0), 2);
        assert_eq!(h.at(1_000.0), 2);
        assert_eq!(h.span(1..3), 50.0);
    }

    #[test]
    fn a_measure_replaces_an_estimate() {
        let mut h = heights(&[10.0, 20.0]);
        assert!(h.set(
            0,
            Height {
                px: 14.0,
                measured: true
            }
        ));
        assert_eq!(h.top(1), 14.0);
        // The same measure again changes nothing.
        assert!(!h.set(
            0,
            Height {
                px: 14.1,
                measured: true
            }
        ));
        assert_eq!(h.total(), 34.0);
    }

    #[test]
    fn splices_move_what_follows() {
        let mut h = heights(&[10.0, 20.0, 30.0]);
        h.splice(1..2, vec![Height::estimated(5.0), Height::estimated(6.0)]);
        assert_eq!(h.len(), 4);
        assert_eq!(h.top(3), 21.0);
        assert_eq!(h.total(), 51.0);
        assert_eq!(h.at(16.0), 2);
    }

    #[test]
    fn a_built_index_replaces_a_range_like_a_splice_does() {
        let mut by_hand = heights(&[10.0, 20.0, 30.0, 40.0]);
        by_hand.set(
            3,
            Height {
                px: 41.0,
                measured: true,
            },
        );
        let mut hung = by_hand.clone();
        let new = [5.0, 6.0, 7.0];
        by_hand.splice(1..3, new.iter().copied().map(Height::estimated).collect());
        let removed = hung.splice_built(1..3, Heights::estimated(new.iter().copied()));
        assert_eq!(removed.len(), 2);
        assert_eq!(removed.total(), 50.0);
        assert_eq!(hung.len(), by_hand.len());
        assert_eq!(hung.total(), by_hand.total());
        for ix in 0..hung.len() {
            assert_eq!(hung.get(ix), by_hand.get(ix), "height {ix}");
            assert_eq!(hung.top(ix), by_hand.top(ix), "top {ix}");
        }
        // The measure of a block that stayed is still a measure.
        assert_eq!(
            hung.get(4),
            Some(Height {
                px: 41.0,
                measured: true
            })
        );
        assert_eq!(hung.at(20.0), by_hand.at(20.0));
    }

    #[test]
    fn a_built_index_goes_into_an_empty_one_whole() {
        let mut live = Heights::default();
        let removed = live.splice_built(0..0, Heights::estimated([3.0, 4.0].into_iter()));
        assert_eq!(removed.len(), 0);
        assert_eq!(live.len(), 2);
        assert_eq!(live.top(2), 7.0);
    }

    #[test]
    fn a_hundred_thousand_blocks_stay_exact_and_fast_to_search() {
        let h = heights(&vec![25.0; 100_000]);
        assert_eq!(h.total(), 2_500_000.0);
        assert_eq!(h.at(1_250_000.0), 50_000);
        assert_eq!(h.top(77_777), 77_777.0 * 25.0);
    }
}
