//! Checks shared by the property tests: that the [`Splices`] an update
//! reports explain how the block list changed, and name only what did.

#![allow(dead_code, missing_docs)]

use bevy_md_core::{Block, Document, Fate, Splices};

/// Whether two blocks are the same, wherever they sit: everything but the
/// extent of the root, which an edit elsewhere in the root changes.
pub fn same_block(a: &Block, b: &Block) -> bool {
    a.kind == b.kind
        && a.content == b.content
        && a.quote_depth == b.quote_depth
        && a.list_depth == b.list_depth
        && a.span_len() == b.span_len()
}

/// An owned copy of a document's blocks and where they start, taken before
/// an update to compare against after it.
pub struct Snapshot {
    pub blocks: Vec<(usize, Block)>,
}

pub fn snapshot(doc: &Document) -> Snapshot {
    Snapshot {
        blocks: doc
            .blocks()
            .map(|block| (block.span().start, block.block().clone()))
            .collect(),
    }
}

/// Panic unless `splices` is a valid description of how `before` became
/// `doc` (the lengths add up, the list is ordered, disjoint and separated),
/// and every block it leaves out is the same block, at the index it says.
/// With `growth` (how many bytes the text grew by, for one edit) a block
/// left out also starts where it did, or `growth` bytes later.
#[track_caller]
pub fn assert_explains(
    before: &Snapshot,
    doc: &Document,
    splices: &Splices,
    growth: Option<isize>,
    context: &str,
) {
    splices.assert_valid(before.blocks.len(), doc.len());
    let mut kept = 0;
    for (ix, (start, block)) in before.blocks.iter().enumerate() {
        match splices.locate_old(ix) {
            Fate::Kept(now) => {
                kept += 1;
                let new = doc.block(now).unwrap_or_else(|| {
                    panic!("{context}: block {ix} kept as {now}, which does not exist")
                });
                assert!(
                    same_block(block, new.block()),
                    "{context}: block {ix} is reported as kept (now {now}) but differs\nbefore: {block:?}\nafter:  {:?}\nsplices: {splices:?}",
                    new.block()
                );
                if let Some(growth) = growth {
                    let moved = start.wrapping_add_signed(growth);
                    assert!(
                        new.span().start == *start || new.span().start == moved,
                        "{context}: kept block {ix} moved from {start} to {} (growth {growth})",
                        new.span().start
                    );
                }
            }
            Fate::Replaced { splice, offset } => {
                let s = &splices[splice];
                assert_eq!(ix, s.old.start + offset, "{context}");
            }
        }
    }
    assert_eq!(kept + splices.inserted(), doc.len(), "{context}");
}
