//! Which blocks an update replaced, as a list of disjoint splices.
//!
//! One edit can change blocks in places far apart: a link reference
//! definition at the end of a note decides how the paragraph that uses it, at
//! the top, is drawn. Reporting the stretch between them as replaced would
//! make every consumer redo work for blocks that did not change. The report is
//! instead a [`Splices`]: only the blocks that did change, in order, with the
//! unchanged ones between them left out.
//!
//! # Coordinates
//!
//! A [`Splices`] describes how one block list, **before**, became another,
//! **after**. Each [`Splice`] has two ranges:
//!
//! - `old` indexes the list before, and `new` the list after.
//! - Blocks that no splice names are *kept*: the same block (same content),
//!   moved by the sum of `new.len() - old.len()` of the splices before it.
//!   Everything that is not a splice is therefore implied, which is what lets
//!   a consumer's work depend on what changed and not on how far apart the
//!   changes are.
//! - The list is **ordered**: `old` ranges ascend and so do `new` ranges.
//!   Splices are **disjoint and separated**: at least one kept block lies
//!   between two of them (two that touch are one splice), and none is empty
//!   (a splice with neither an old nor a new block names no change).
//! - Because of the shift, `new.start == old.start + (blocks inserted minus
//!   blocks removed by the splices before)`; for the first splice the two
//!   starts are the same.
//!
//! [`Splices::locate_old`] finds the fate of any block of the list before in
//! `O(log k)` for `k` splices.
//!
//! # Composition
//!
//! Edits made one after the other, before a consumer looks, compose into one
//! [`Splices`] that goes straight from the list the consumer last saw to the
//! current one ([`Splices::compose`]): `then` takes the next edit expressed
//! against the list the earlier ones produced, merges the splices it touches
//! or overlaps with it (a block inserted by an earlier edit and replaced by
//! a later one is one splice, and an insertion that a later edit deletes
//! again vanishes), and shifts the `new` ranges of the splices after it.
//! The cost is `O(log k)` to find the place and `O(k)` to shift the tail
//! (`k` being the splices pending, not the blocks in the document). An edit
//! that changes the number of blocks needs nothing else: the blocks it did
//! not name move implicitly.
//!
//! A background parse brings the same thing: the result is a [`Splices`]
//! from the document that was shown to the one it parsed, which composes with
//! whatever was pending on the shown one, and the edits made while it ran are
//! applied afterwards and compose onto it in turn.

use std::ops::{Index, Range};

/// A stretch of blocks replaced: `old` in the list before became `new` in the
/// list after.
///
/// By itself a `Splice` is the report of an edit that changed one stretch, so
/// `old.start == new.start`; inside a [`Splices`] the starts differ by the
/// shift of the splices before it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Splice {
    /// Replaced indices in the block list before the update.
    pub old: Range<usize>,
    /// Their replacement in the block list after the update.
    pub new: Range<usize>,
}

impl Splice {
    /// Whether the update changed no block.
    pub fn is_empty(&self) -> bool {
        self.old.is_empty() && self.new.is_empty()
    }

    /// Blocks gained (positive) or lost.
    pub fn delta(&self) -> isize {
        self.new.len().cast_signed() - self.old.len().cast_signed()
    }
}

/// What became of a block of the list before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fate {
    /// The block is unchanged, now at this index of the list after.
    Kept(usize),
    /// The block was replaced: it was block `offset` of the `old` range of
    /// splice number `splice`.
    Replaced {
        /// Position of the splice in the list.
        splice: usize,
        /// Index of the block within the splice's `old` range.
        offset: usize,
    },
}

/// The blocks an update (or several composed) changed: ordered, disjoint
/// [`Splice`]s, with everything between them unchanged. See the module
/// documentation for the coordinates and for composition.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Splices {
    items: Vec<Splice>,
}

impl Splices {
    /// No change.
    pub fn new() -> Self {
        Self::default()
    }

    /// The change of one stretch (`old.start == new.start`); no change when
    /// the splice is empty.
    pub fn one(splice: Splice) -> Self {
        debug_assert_eq!(splice.old.start, splice.new.start);
        if splice.is_empty() {
            return Self::new();
        }
        Self {
            items: vec![splice],
        }
    }

    /// Everything replaced: a list of `old_len` blocks became one of
    /// `new_len`.
    pub fn everything(old_len: usize, new_len: usize) -> Self {
        Self::one(Splice {
            old: 0..old_len,
            new: 0..new_len,
        })
    }

    /// Whether nothing changed.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// How many splices there are.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// The splices, in order.
    pub fn as_slice(&self) -> &[Splice] {
        &self.items
    }

    /// The splices, in order.
    pub fn iter(&self) -> std::slice::Iter<'_, Splice> {
        self.items.iter()
    }

    /// The one splice that covers all of them and the unchanged blocks
    /// between them: what an API with a single range would have reported.
    /// For consumers that cannot do better, and for tests.
    pub fn covering(&self) -> Splice {
        match (self.items.first(), self.items.last()) {
            (Some(first), Some(last)) => Splice {
                old: first.old.start..last.old.end,
                new: first.new.start..last.new.end,
            },
            _ => Splice {
                old: 0..0,
                new: 0..0,
            },
        }
    }

    /// How many blocks of the list before were replaced, in all.
    pub fn removed(&self) -> usize {
        self.items.iter().map(|s| s.old.len()).sum()
    }

    /// How many blocks of the list after replaced them, in all.
    pub fn inserted(&self) -> usize {
        self.items.iter().map(|s| s.new.len()).sum()
    }

    /// The length of the list after, given the length of the one before.
    pub fn len_after(&self, before: usize) -> usize {
        before - self.removed() + self.inserted()
    }

    /// What became of block `index` of the list before, in `O(log k)`.
    pub fn locate_old(&self, index: usize) -> Fate {
        let at = self.items.partition_point(|s| s.old.end <= index);
        if let Some(splice) = self.items.get(at)
            && splice.old.start <= index
        {
            return Fate::Replaced {
                splice: at,
                offset: index - splice.old.start,
            };
        }
        Fate::Kept(index.wrapping_add_signed(self.shift_before(at)))
    }

    /// Where block `index` of the list before is now, if it was kept.
    pub fn map_old(&self, index: usize) -> Option<usize> {
        match self.locate_old(index) {
            Fate::Kept(now) => Some(now),
            Fate::Replaced { .. } => None,
        }
    }

    /// The shift of the blocks after the first `count` splices: how many
    /// more (or fewer) blocks the list has before them.
    fn shift_before(&self, count: usize) -> isize {
        count.checked_sub(1).map_or(0, |last| {
            self.items[last].new.end.cast_signed() - self.items[last].old.end.cast_signed()
        })
    }

    /// Fold in `next`, a single splice of the list this one produced (so
    /// `next.old` indexes the list after `self`, and `old.start ==
    /// new.start`): afterwards `self` goes from the list before to the one
    /// `next` produced. Splices that `next` touches or overlaps are merged
    /// with it; the ones after it have their `new` ranges moved.
    ///
    /// `O(log k)` to find the place, `O(k)` to move the splices after it.
    pub fn then(&mut self, next: &Splice) {
        debug_assert_eq!(next.old.start, next.new.start);
        if next.is_empty() {
            return;
        }
        // The splices whose `new` ranges touch `next.old`: from `lo` up to
        // (excluding) `hi`.
        let lo = self.items.partition_point(|s| s.new.end < next.old.start);
        let hi = self.items.partition_point(|s| s.new.start <= next.old.end);
        let hi = hi.max(lo);

        // The stretch of the list in between, in its coordinates.
        let (mut start, mut end) = (next.old.start, next.old.end);
        if lo < hi {
            start = start.min(self.items[lo].new.start);
            end = end.max(self.items[hi - 1].new.end);
        }
        let before = self.shift_before(lo);
        let after = self.shift_before(hi);
        let merged = Splice {
            old: start.wrapping_add_signed(-before)..end.wrapping_add_signed(-after),
            new: start..end.wrapping_add_signed(next.delta()),
        };
        // The splices past the merged one only move.
        let delta = next.delta();
        if delta != 0 {
            for s in &mut self.items[hi..] {
                s.new.start = s.new.start.wrapping_add_signed(delta);
                s.new.end = s.new.end.wrapping_add_signed(delta);
            }
        }
        let replacement = (!merged.is_empty()).then_some(merged);
        self.items.splice(lo..hi, replacement);
    }

    /// Fold in `next`, the splices of the list this one produced: afterwards
    /// `self` goes from the list before to the one `next` produced. Applying
    /// them in order, each in the list the ones before it left.
    pub fn compose(&mut self, next: &Splices) {
        if self.items.is_empty() {
            self.items.clone_from(&next.items);
            return;
        }
        let mut shift = 0isize;
        for s in &next.items {
            // `s.old` is in the list `next` starts from; the splices of
            // `next` before it have already been applied.
            let start = s.old.start.wrapping_add_signed(shift);
            self.then(&Splice {
                old: start..start + s.old.len(),
                new: start..start + s.new.len(),
            });
            shift += s.delta();
        }
    }

    /// Add a splice after the last one, for the code that builds a list in
    /// order: `old` and `new` are the ranges of the lists before and after.
    /// A splice that touches the last one extends it.
    pub(crate) fn push(&mut self, old: Range<usize>, new: Range<usize>) {
        if old.is_empty() && new.is_empty() {
            return;
        }
        if let Some(last) = self.items.last_mut()
            && last.old.end == old.start
            && last.new.end == new.start
        {
            last.old.end = old.end;
            last.new.end = new.end;
            return;
        }
        debug_assert!(self.items.last().is_none_or(|last| {
            last.old.end < old.start
                && last.new.end < new.start
                && old.start - last.old.end == new.start - last.new.end
        }));
        self.items.push(Splice { old, new });
    }

    /// Panic unless the list is valid for block lists of the given lengths:
    /// ordered, disjoint, separated by kept blocks, none empty, the shifts
    /// consistent, and the lengths adding up. For tests.
    #[doc(hidden)]
    pub fn assert_valid(&self, old_len: usize, new_len: usize) {
        let mut shift = 0isize;
        let mut old_end = 0;
        for (n, s) in self.items.iter().enumerate() {
            assert!(!s.is_empty(), "splice {n} is empty: {self:?}");
            assert!(
                s.old.start <= s.old.end && s.new.start <= s.new.end,
                "{self:?}"
            );
            assert!(
                n == 0 || s.old.start > old_end,
                "splice {n} touches the one before: {self:?}"
            );
            assert_eq!(
                s.new.start,
                s.old.start.wrapping_add_signed(shift),
                "splice {n} is not shifted by the ones before it: {self:?}"
            );
            shift += s.delta();
            old_end = s.old.end;
        }
        assert!(old_end <= old_len, "reaches past the old list: {self:?}");
        assert_eq!(
            old_len.wrapping_add_signed(shift),
            new_len,
            "the lengths do not add up: {self:?}"
        );
        if let Some(last) = self.items.last() {
            assert!(last.new.end <= new_len, "{self:?}");
        }
    }
}

impl Index<usize> for Splices {
    type Output = Splice;

    fn index(&self, index: usize) -> &Splice {
        &self.items[index]
    }
}

impl<'a> IntoIterator for &'a Splices {
    type Item = &'a Splice;
    type IntoIter = std::slice::Iter<'a, Splice>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply a splice to a model list: the blocks `old` become fresh ones
    /// numbered from `next_id`.
    fn replace(list: &mut Vec<u32>, old: Range<usize>, new_len: usize, next_id: &mut u32) {
        let fresh: Vec<u32> = (0..new_len)
            .map(|_| {
                *next_id += 1;
                *next_id
            })
            .collect();
        list.splice(old, fresh);
    }

    /// A deterministic generator, so a failure repeats.
    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
        }
    }

    /// `splices` must explain how `before` became `after`: every block they
    /// do not name is the same one (same id), at the place they say, and
    /// the replacements are in `after` where `new` says.
    fn explains(splices: &Splices, before: &[u32], after: &[u32]) {
        splices.assert_valid(before.len(), after.len());
        for (index, id) in before.iter().enumerate() {
            match splices.locate_old(index) {
                Fate::Kept(now) => {
                    assert_eq!(after[now], *id, "block {index} was kept: {splices:?}");
                }
                Fate::Replaced { splice, offset } => {
                    let s = &splices[splice];
                    assert!(s.old.contains(&index));
                    assert_eq!(offset, index - s.old.start);
                }
            }
        }
        // And nothing new hides outside the splices: the blocks of `after`
        // that are not in a `new` range are exactly the kept ones.
        let named: usize = splices.inserted();
        let kept = (0..before.len())
            .filter(|&i| splices.map_old(i).is_some())
            .count();
        assert_eq!(kept + named, after.len(), "{splices:?}");
        for s in splices {
            for id in &after[s.new.clone()] {
                assert!(!before.contains(id), "{splices:?}: {id} is not new");
            }
        }
    }

    /// One edit of a model list, returned as the splice it is.
    fn random_edit(rng: &mut Rng, list: &mut Vec<u32>, next_id: &mut u32) -> Splice {
        let len = list.len();
        let start = rng.below(len + 1);
        let removed = match rng.below(3) {
            0 => 0,
            1 => rng.below(3),
            _ => rng.below(8),
        }
        .min(len - start);
        let inserted = rng.below(4);
        replace(list, start..start + removed, inserted, next_id);
        Splice {
            old: start..start + removed,
            new: start..start + inserted,
        }
    }

    #[test]
    fn one_edit_after_another_composes_into_what_happened() {
        for seed in 1..=3000u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let mut next_id = 1000;
            let start: Vec<u32> = (0..rng.below(40) as u32).collect();
            let mut list = start.clone();
            let mut splices = Splices::new();
            for step in 0..=rng.below(12) {
                let edit = random_edit(&mut rng, &mut list, &mut next_id);
                splices.then(&edit);
                explains(&splices, &start, &list);
                let _ = step;
            }
        }
    }

    #[test]
    fn lists_of_splices_compose_in_order() {
        for seed in 1..=3000u64 {
            let mut rng = Rng(seed.wrapping_mul(0xD1B5_4A32_D192_ED03));
            let mut next_id = 1000;
            let start: Vec<u32> = (0..rng.below(60) as u32).collect();
            let mut list = start.clone();
            let mut pending = Splices::new();
            for _ in 0..=rng.below(6) {
                // A list of several splices made at once, in the coordinates
                // of `list` (ascending, separated).
                let before = list.clone();
                let mut made = Splices::new();
                let mut at = 0;
                let mut shift = 0isize;
                let mut after = Vec::new();
                while at <= before.len() && made.len() < 4 {
                    let start = at + rng.below(before.len() - at + 1);
                    let removed = rng.below(4).min(before.len() - start);
                    let inserted = rng.below(3);
                    if removed == 0 && inserted == 0 {
                        break;
                    }
                    let new_start = start.wrapping_add_signed(shift);
                    made.push(start..start + removed, new_start..new_start + inserted);
                    shift += inserted.cast_signed() - removed.cast_signed();
                    after.push((start, removed, inserted));
                    at = start + removed + 1;
                    if at > before.len() {
                        break;
                    }
                }
                // Apply them from the last, so the earlier ones keep their
                // coordinates.
                for &(start, removed, inserted) in after.iter().rev() {
                    replace(&mut list, start..start + removed, inserted, &mut next_id);
                }
                explains(&made, &before, &list);
                pending.compose(&made);
                explains(&pending, &start, &list);
            }
        }
    }

    #[test]
    fn touching_splices_merge_and_a_deleted_insertion_vanishes() {
        // Insert two blocks at 5, then delete them again.
        let mut s = Splices::new();
        s.then(&Splice {
            old: 5..5,
            new: 5..7,
        });
        s.then(&Splice {
            old: 5..7,
            new: 5..5,
        });
        assert!(s.is_empty(), "{s:?}");
        // Two edits that touch are one splice.
        let mut s = Splices::new();
        s.then(&Splice {
            old: 5..6,
            new: 5..6,
        });
        s.then(&Splice {
            old: 6..7,
            new: 6..7,
        });
        assert_eq!(
            s.as_slice(),
            [Splice {
                old: 5..7,
                new: 5..7
            }]
        );
        // Two with a kept block between stay two, the later one moved by the
        // earlier one's growth.
        let mut s = Splices::new();
        s.then(&Splice {
            old: 10..11,
            new: 10..11,
        });
        s.then(&Splice {
            old: 2..2,
            new: 2..5,
        });
        assert_eq!(
            s.as_slice(),
            [
                Splice {
                    old: 2..2,
                    new: 2..5
                },
                Splice {
                    old: 10..11,
                    new: 13..14
                },
            ]
        );
        assert_eq!(s.locate_old(2), Fate::Kept(5));
        assert_eq!(
            s.locate_old(10),
            Fate::Replaced {
                splice: 1,
                offset: 0
            }
        );
        assert_eq!(s.locate_old(11), Fate::Kept(14));
        assert_eq!(
            s.covering(),
            Splice {
                old: 2..11,
                new: 2..14
            }
        );
    }

    #[test]
    fn an_empty_list_composes_into_what_it_is_given() {
        let mut s = Splices::new();
        let mut next = Splices::new();
        next.push(3..4, 3..4);
        next.push(9..9, 9..12);
        s.compose(&next);
        assert_eq!(s, next);
        s.assert_valid(20, 23);
    }
}
