//! A sum tree: a sequence with summaries, searchable by running sums.
//!
//! Every node keeps the item count and the summary of its subtree. That gives
//! `O(log n)` for what a long document needs from one structure:
//!
//! - index to running sum ([`SumTree::prefix`]), such as the source offset or
//!   the pixel height before block `i`;
//! - running sum to index ([`SumTree::seek`]), such as the block at a source
//!   offset or at a scroll position;
//! - changing one item ([`SumTree::update`]) and replacing a run of items
//!   ([`SumTree::splice`], `O(log n + k)`).
//!
//! # Why a B-tree of chunks
//!
//! Positions are never stored absolutely: an item carries the distance from
//! the previous one, so an edit changes one item and the sums along one path,
//! instead of shifting every item after it. Flat alternatives were weighed:
//! a Fenwick tree has no insertion or removal, and a vector of chunks with an
//! offset per chunk makes every edit touch `n / chunk` offsets. This tree has
//! the fan-out of a B-tree (leaves of up to 32 items, inner nodes of up to
//! 16 children, so 100 000 items are four levels deep) and stays flat in
//! practice. It is the same idea as the `SumTree` of the gpui editor that
//! stayed fast with thousands of blocks.
//!
//! Nodes are not merged after deletions; the tree is rebuilt in bulk when
//! it has become sparse, which amortizes to nothing.

use std::ops::{Add, Range};

/// What a subtree is summarized as: a monoid.
pub trait Summary: Clone + Default + std::fmt::Debug {
    /// Fold `other`, the summary of what comes after, into `self`.
    fn add(&mut self, other: &Self);
}

/// An element of a [`SumTree`].
pub trait Item: Clone + std::fmt::Debug {
    /// The summary of one item, and of a run of them.
    type Summary: Summary;

    /// The summary of this item alone.
    fn summary(&self) -> Self::Summary;
}

/// One running sum a tree can be searched by: a value derived from a
/// summary that adds up and never decreases along the sequence.
pub trait Dimension<S>: Copy + Default + PartialOrd + Add<Output = Self> {
    /// The value of one summary.
    fn from_summary(summary: &S) -> Self;
}

const LEAF_MAX: usize = 32;
const LEAF_FILL: usize = 24;
const INNER_MAX: usize = 16;
const INNER_FILL: usize = 12;
/// A sparse tree is compacted once per this many splices.
const COMPACT_CHECK: u32 = 2048;

#[derive(Clone, Debug)]
struct Child<T: Item> {
    count: usize,
    summary: T::Summary,
    node: Node<T>,
}

#[derive(Clone, Debug)]
enum Node<T: Item> {
    Leaf(Vec<T>),
    Inner(Vec<Child<T>>),
}

impl<T: Item> Node<T> {
    fn count(&self) -> usize {
        match self {
            Node::Leaf(items) => items.len(),
            Node::Inner(children) => children.iter().map(|child| child.count).sum(),
        }
    }

    fn summarize(&self) -> T::Summary {
        let mut total = T::Summary::default();
        match self {
            Node::Leaf(items) => {
                for item in items {
                    total.add(&item.summary());
                }
            }
            Node::Inner(children) => {
                for child in children {
                    total.add(&child.summary);
                }
            }
        }
        total
    }

    fn into_child(self) -> Child<T> {
        Child {
            count: self.count(),
            summary: self.summarize(),
            node: self,
        }
    }

    fn leaves(&self) -> usize {
        match self {
            Node::Leaf(_) => 1,
            Node::Inner(children) => children.iter().map(|child| child.node.leaves()).sum(),
        }
    }
}

impl<T: Item> Child<T> {
    fn refresh(&mut self) {
        self.count = self.node.count();
        self.summary = self.node.summarize();
    }
}

/// A sequence of items with `O(log n)` index and running-sum queries.
#[derive(Clone, Debug)]
pub struct SumTree<T: Item> {
    root: Node<T>,
    len: usize,
    summary: T::Summary,
    splices: u32,
}

impl<T: Item> Default for SumTree<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Item> SumTree<T> {
    /// An empty tree.
    pub fn new() -> Self {
        Self {
            root: Node::Leaf(Vec::new()),
            len: 0,
            summary: T::Summary::default(),
            splices: 0,
        }
    }

    /// A tree of `items`, built in `O(n)`.
    pub fn from_vec(items: Vec<T>) -> Self {
        let len = items.len();
        let mut level: Vec<Child<T>> = Vec::with_capacity(len / LEAF_FILL + 1);
        let mut items = items.into_iter();
        loop {
            let chunk: Vec<T> = items.by_ref().take(LEAF_FILL).collect();
            if chunk.is_empty() {
                break;
            }
            level.push(Node::Leaf(chunk).into_child());
        }
        while level.len() > INNER_FILL {
            level = group(level);
        }
        let root = match level.pop() {
            None => Node::Leaf(Vec::new()),
            Some(only) if level.is_empty() => only.node,
            Some(last) => {
                level.push(last);
                Node::Inner(level)
            }
        };
        let summary = root.summarize();
        Self {
            root,
            len,
            summary,
            splices: 0,
        }
    }

    /// How many items the tree holds.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether the tree holds no items.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The summary of every item.
    pub fn summary(&self) -> &T::Summary {
        &self.summary
    }

    /// The item at `index`.
    pub fn get(&self, mut index: usize) -> Option<&T> {
        if index >= self.len {
            return None;
        }
        let mut node = &self.root;
        loop {
            match node {
                Node::Leaf(items) => return items.get(index),
                Node::Inner(children) => {
                    let mut next = None;
                    for child in children {
                        if index < child.count {
                            next = Some(&child.node);
                            break;
                        }
                        index -= child.count;
                    }
                    node = next?;
                }
            }
        }
    }

    /// The item at `index` together with the sum of dimension `D` before it,
    /// in one descent.
    pub fn get_with_prefix<D: Dimension<T::Summary>>(&self, mut index: usize) -> Option<(&T, D)> {
        if index >= self.len {
            return None;
        }
        let mut acc = D::default();
        let mut node = &self.root;
        loop {
            match node {
                Node::Leaf(items) => {
                    for item in &items[..index] {
                        acc = acc + D::from_summary(&item.summary());
                    }
                    return Some((&items[index], acc));
                }
                Node::Inner(children) => {
                    let mut next = None;
                    for child in children {
                        if index < child.count {
                            next = Some(&child.node);
                            break;
                        }
                        acc = acc + D::from_summary(&child.summary);
                        index -= child.count;
                    }
                    node = next?;
                }
            }
        }
    }

    /// Change the item at `index` with `change`, then repair the summaries
    /// on the path to it. The count of items cannot change.
    ///
    /// # Panics
    ///
    /// When `index` is out of bounds.
    pub fn update(&mut self, index: usize, change: impl FnOnce(&mut T)) {
        assert!(
            index < self.len,
            "index {index} out of bounds ({})",
            self.len
        );
        update_node(&mut self.root, index, change);
        self.summary = self.root.summarize();
    }

    /// The sum of dimension `D` over the items before `index`
    /// (`index == len` is the sum of all of them).
    pub fn prefix<D: Dimension<T::Summary>>(&self, mut index: usize) -> D {
        let mut acc = D::default();
        let mut node = &self.root;
        loop {
            match node {
                Node::Leaf(items) => {
                    for item in &items[..index.min(items.len())] {
                        acc = acc + D::from_summary(&item.summary());
                    }
                    return acc;
                }
                Node::Inner(children) => {
                    let mut next = None;
                    for child in children {
                        if index >= child.count {
                            acc = acc + D::from_summary(&child.summary);
                            index -= child.count;
                        } else {
                            next = Some(&child.node);
                            break;
                        }
                    }
                    match next {
                        Some(next) if index > 0 => node = next,
                        _ => return acc,
                    }
                }
            }
        }
    }

    /// How many leading items have a running sum of dimension `D`,
    /// themselves included, of at most `target`. The index of the last
    /// item starting at or before `target` is one less, when it is not 0.
    pub fn seek<D: Dimension<T::Summary>>(&self, target: D) -> usize {
        let mut acc = D::default();
        let mut count = 0;
        let mut node = &self.root;
        loop {
            match node {
                Node::Leaf(items) => {
                    for item in items {
                        let next = acc + D::from_summary(&item.summary());
                        if next <= target {
                            acc = next;
                            count += 1;
                        } else {
                            break;
                        }
                    }
                    return count;
                }
                Node::Inner(children) => {
                    let mut descend = None;
                    for child in children {
                        let next = acc + D::from_summary(&child.summary);
                        if next <= target {
                            acc = next;
                            count += child.count;
                        } else {
                            descend = Some(&child.node);
                            break;
                        }
                    }
                    match descend {
                        Some(next) => node = next,
                        None => return count,
                    }
                }
            }
        }
    }

    /// Replace the items `range` with `items`, in `O(log n + k)` for `k`
    /// items touched.
    ///
    /// # Panics
    ///
    /// When `range` is out of bounds.
    pub fn splice(&mut self, range: Range<usize>, items: Vec<T>) {
        assert!(
            range.start <= range.end && range.end <= self.len,
            "range {range:?} out of bounds ({})",
            self.len
        );
        let extras = splice_node(&mut self.root, range.start, range.end, items);
        if !extras.is_empty() {
            let old = std::mem::replace(&mut self.root, Node::Leaf(Vec::new()));
            let mut kids = vec![old.into_child()];
            kids.extend(extras);
            while kids.len() > INNER_MAX {
                kids = group(kids);
            }
            self.root = Node::Inner(kids);
        }
        loop {
            match &mut self.root {
                Node::Inner(children) if children.len() == 1 => {
                    let only = children.pop().expect("one child");
                    self.root = only.node;
                }
                Node::Inner(children) if children.is_empty() => {
                    self.root = Node::Leaf(Vec::new());
                }
                _ => break,
            }
        }
        self.len = self.root.count();
        self.summary = self.root.summarize();
        self.splices += 1;
        if self.splices >= COMPACT_CHECK {
            self.splices = 0;
            if self.root.leaves() * LEAF_MAX > self.len * 4 + LEAF_MAX {
                *self = Self::from_vec(self.iter().cloned().collect());
            }
        }
    }

    /// Cut the tree at `at`: it keeps the items before `at` and the items
    /// from `at` on are returned as a tree of their own. `O(log n)`: only
    /// the nodes on the path to `at` are split, whatever the item count.
    ///
    /// # Panics
    ///
    /// When `at` is past the end.
    #[must_use]
    pub fn split_off(&mut self, at: usize) -> SumTree<T> {
        assert!(at <= self.len, "split at {at} out of bounds ({})", self.len);
        if at == self.len {
            return Self::new();
        }
        if at == 0 {
            return std::mem::take(self);
        }
        let root = std::mem::replace(&mut self.root, Node::Leaf(Vec::new()));
        let (left, right) = split_node(root, at);
        let tail_len = self.len - at;
        self.root = normalized(left);
        self.len = at;
        self.summary = self.root.summarize();
        let root = normalized(right);
        SumTree {
            summary: root.summarize(),
            root,
            len: tail_len,
            splices: 0,
        }
    }

    /// Put the items of `other` after the ones of this tree, in
    /// `O(log n)`: `other` is hung from the right edge of this tree (or
    /// this one from the left edge of `other`) at the level its height
    /// calls for, and none of its items is visited.
    pub fn append(&mut self, other: SumTree<T>) {
        if other.is_empty() {
            return;
        }
        if self.is_empty() {
            *self = other;
            return;
        }
        let left = std::mem::replace(&mut self.root, Node::Leaf(Vec::new()));
        self.root = join(left, other.root);
        self.len += other.len;
        self.summary = self.root.summarize();
        self.splices = self.splices.max(other.splices);
    }

    /// Replace the items `range` with all the items of `other`, without
    /// visiting any of them: `O(log n)` whatever the size of `range` and of
    /// `other`, where [`SumTree::splice`] is `O(log n + k)`. Returns the
    /// items taken out, as a tree, so that freeing them can be left to
    /// another thread.
    ///
    /// # Panics
    ///
    /// When `range` is out of bounds.
    #[must_use]
    pub fn splice_tree(&mut self, range: Range<usize>, other: SumTree<T>) -> SumTree<T> {
        assert!(
            range.start <= range.end && range.end <= self.len,
            "range {range:?} out of bounds ({})",
            self.len
        );
        let tail = self.split_off(range.end);
        let removed = self.split_off(range.start);
        self.append(other);
        self.append(tail);
        self.splices += 1;
        if self.splices >= COMPACT_CHECK {
            self.splices = 0;
            if self.root.leaves() * LEAF_MAX > self.len * 4 + LEAF_MAX {
                *self = Self::from_vec(self.iter().cloned().collect());
            }
        }
        removed
    }

    /// The items in order.
    pub fn iter(&self) -> Iter<'_, T> {
        self.iter_from(0)
    }

    /// The items from `index` on, in order.
    pub fn iter_from(&self, mut index: usize) -> Iter<'_, T> {
        let mut iter = Iter {
            stack: [(&[], 0); MAX_DEPTH],
            depth: 0,
            leaf: None,
            remaining: self.len.saturating_sub(index),
        };
        if index >= self.len {
            return iter;
        }
        let mut node = &self.root;
        loop {
            match node {
                Node::Leaf(items) => {
                    iter.leaf = Some((items, index));
                    return iter;
                }
                Node::Inner(children) => {
                    let mut at = 0;
                    while index >= children[at].count {
                        index -= children[at].count;
                        at += 1;
                    }
                    iter.push(children, at + 1);
                    node = &children[at].node;
                }
            }
        }
    }
}

impl<'a, T: Item> IntoIterator for &'a SumTree<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

/// Deeper than a tree can grow in practice (depth is 5 at 100k items;
/// a deeper tree is a bug and panics).
const MAX_DEPTH: usize = 24;

/// The items of a [`SumTree`], in order.
pub struct Iter<'a, T: Item> {
    /// Inner nodes on the way down, each with the next child to visit.
    stack: [(&'a [Child<T>], usize); MAX_DEPTH],
    depth: usize,
    leaf: Option<(&'a [T], usize)>,
    remaining: usize,
}

impl<'a, T: Item> Iter<'a, T> {
    fn push(&mut self, children: &'a [Child<T>], next: usize) {
        self.stack[self.depth] = (children, next);
        self.depth += 1;
    }
}

impl<'a, T: Item> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<&'a T> {
        loop {
            if let Some((items, at)) = &mut self.leaf {
                if let Some(item) = items.get(*at) {
                    *at += 1;
                    self.remaining -= 1;
                    return Some(item);
                }
                self.leaf = None;
            }
            let top = self.depth.checked_sub(1)?;
            let (children, next) = &mut self.stack[top];
            let Some(child) = children.get(*next) else {
                self.depth -= 1;
                continue;
            };
            *next += 1;
            match &child.node {
                Node::Leaf(items) => self.leaf = Some((items, 0)),
                Node::Inner(children) => self.push(children, 0),
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<T: Item> ExactSizeIterator for Iter<'_, T> {}

/// Chunk `children` under new inner nodes of up to `INNER_FILL` each.
fn group<T: Item>(children: Vec<Child<T>>) -> Vec<Child<T>> {
    let mut parents = Vec::with_capacity(children.len() / INNER_FILL + 1);
    let mut children = children.into_iter();
    loop {
        let chunk: Vec<Child<T>> = children.by_ref().take(INNER_FILL).collect();
        if chunk.is_empty() {
            return parents;
        }
        parents.push(Node::Inner(chunk).into_child());
    }
}

fn update_node<T: Item>(node: &mut Node<T>, mut index: usize, change: impl FnOnce(&mut T)) {
    match node {
        Node::Leaf(items) => change(&mut items[index]),
        Node::Inner(children) => {
            for child in children {
                if index < child.count {
                    update_node(&mut child.node, index, change);
                    child.summary = child.node.summarize();
                    return;
                }
                index -= child.count;
            }
            unreachable!("index checked against the count");
        }
    }
}

/// Replace items `a..b` (local to `node`) with `items`. Returns the siblings
/// that follow `node` when it grew past its capacity and had to split.
fn splice_node<T: Item>(node: &mut Node<T>, a: usize, b: usize, items: Vec<T>) -> Vec<Child<T>> {
    match node {
        Node::Leaf(leaf) => {
            leaf.splice(a..b, items);
            if leaf.len() <= LEAF_MAX {
                return Vec::new();
            }
            let mut tail = leaf.split_off(LEAF_FILL).into_iter();
            let mut extras = Vec::new();
            loop {
                let chunk: Vec<T> = tail.by_ref().take(LEAF_FILL).collect();
                if chunk.is_empty() {
                    return extras;
                }
                extras.push(Node::Leaf(chunk).into_child());
            }
        }
        Node::Inner(children) => {
            // The child the splice starts in (a boundary goes to the child
            // that follows it, unless there is none).
            let (mut first, mut offset) = (0, 0);
            while first + 1 < children.len() && a >= offset + children[first].count {
                offset += children[first].count;
                first += 1;
            }
            // The last child it reaches.
            let mut last = first;
            let mut end = offset + children[first].count;
            if b <= end {
                // The common case: the splice stays inside one child.
                let child = &mut children[first];
                let extras = splice_node(&mut child.node, a - offset, b - offset, items);
                child.refresh();
                let count = child.count;
                let at = first + usize::from(count > 0);
                if count == 0 {
                    children.remove(first);
                }
                children.splice(at..at, extras.into_iter().filter(|extra| extra.count > 0));
                return if children.len() <= INNER_MAX {
                    Vec::new()
                } else {
                    group(children.split_off(INNER_FILL))
                };
            }
            while b > end && last + 1 < children.len() {
                last += 1;
                end += children[last].count;
            }
            let affected: Vec<Child<T>> = children.drain(first..=last).collect();
            let mut replacement = Vec::with_capacity(affected.len() + 1);
            let mut items = Some(items);
            let mut pos = offset;
            for (k, mut child) in affected.into_iter().enumerate() {
                let count = child.count;
                let local_a = if k == 0 { a - pos } else { 0 };
                let local_b = (b - pos).min(count);
                pos += count;
                if k > 0 && local_a == 0 && local_b == count {
                    continue; // swallowed whole
                }
                let extras = splice_node(
                    &mut child.node,
                    local_a,
                    local_b,
                    items.take().unwrap_or_default(),
                );
                child.refresh();
                if child.count > 0 {
                    replacement.push(child);
                }
                replacement.extend(extras.into_iter().filter(|extra| extra.count > 0));
            }
            children.splice(first..first, replacement);
            if children.len() <= INNER_MAX {
                return Vec::new();
            }
            let tail = children.split_off(INNER_FILL);
            group(tail)
        }
    }
}

/// Levels below `node`'s root: 0 for a leaf. All the leaves of a tree are at
/// the same depth, so the first path tells.
fn height<T: Item>(node: &Node<T>) -> usize {
    let mut levels = 0;
    let mut node = node;
    while let Node::Inner(children) = node {
        levels += 1;
        match children.first() {
            Some(first) => node = &first.node,
            None => break,
        }
    }
    levels
}

/// A node holding `children`, or the empty leaf when there are none.
fn inner_or_empty<T: Item>(children: Vec<Child<T>>) -> Node<T> {
    if children.is_empty() {
        Node::Leaf(Vec::new())
    } else {
        Node::Inner(children)
    }
}

/// `node` with the one-child roots stripped, which a cut leaves behind.
fn normalized<T: Item>(mut node: Node<T>) -> Node<T> {
    loop {
        match node {
            Node::Inner(mut children) if children.len() == 1 => {
                node = children.pop().expect("one child").node;
            }
            Node::Inner(children) if children.is_empty() => return Node::Leaf(Vec::new()),
            other => return other,
        }
    }
}

/// Cut `node` after `at` items (`0 < at < count`) into the items before and
/// the items after. Both halves keep the height of `node` (a half that
/// would be empty is the empty leaf), and only the nodes on the way down
/// are rebuilt.
fn split_node<T: Item>(node: Node<T>, at: usize) -> (Node<T>, Node<T>) {
    match node {
        Node::Leaf(mut items) => {
            let right = items.split_off(at);
            (Node::Leaf(items), Node::Leaf(right))
        }
        Node::Inner(mut children) => {
            let mut offset = 0;
            let mut index = 0;
            while index < children.len() && at >= offset + children[index].count {
                offset += children[index].count;
                index += 1;
            }
            if index == children.len() {
                return (Node::Inner(children), Node::Leaf(Vec::new()));
            }
            let mut right = children.split_off(index + 1);
            let cut = children.pop().expect("the child holding `at`");
            let local = at - offset;
            if local == 0 {
                right.insert(0, cut);
            } else {
                let (before, after) = split_node(cut.node, local);
                children.push(before.into_child());
                if after.count() > 0 {
                    right.insert(0, after.into_child());
                }
            }
            (inner_or_empty(children), inner_or_empty(right))
        }
    }
}

/// The sequence of `left` followed by that of `right`, as a node (both
/// non empty). A tree is hung from the edge of the taller one, at the level
/// where the heights match, so the result keeps every leaf at one depth.
fn join<T: Item>(left: Node<T>, right: Node<T>) -> Node<T> {
    let (left_height, right_height) = (height(&left), height(&right));
    let mut root = match left_height.cmp(&right_height) {
        std::cmp::Ordering::Equal => return join_equal(left, right),
        std::cmp::Ordering::Greater => {
            let mut root = left;
            let extras = hang_right(&mut root, right, left_height - right_height);
            (root, extras)
        }
        std::cmp::Ordering::Less => {
            let mut root = right;
            let extras = hang_left(&mut root, left, right_height - left_height);
            (root, extras)
        }
    };
    if !root.1.is_empty() {
        let mut kids = vec![root.0.into_child()];
        kids.extend(root.1);
        while kids.len() > INNER_MAX {
            kids = group(kids);
        }
        root.0 = Node::Inner(kids);
    }
    root.0
}

/// Two nodes of the same height: merged when they fit in one, else siblings
/// under a new root.
fn join_equal<T: Item>(left: Node<T>, right: Node<T>) -> Node<T> {
    match (left, right) {
        (Node::Leaf(mut a), Node::Leaf(b)) if a.len() + b.len() <= LEAF_MAX => {
            a.extend(b);
            Node::Leaf(a)
        }
        (Node::Inner(mut a), Node::Inner(b)) if a.len() + b.len() <= INNER_MAX => {
            a.extend(b);
            Node::Inner(a)
        }
        (a, b) => Node::Inner(vec![a.into_child(), b.into_child()]),
    }
}

/// Hang `tree` (`gap` levels shorter than `node`) from the right edge of
/// `node`. Returns the siblings that follow `node` when it overflowed.
fn hang_right<T: Item>(node: &mut Node<T>, tree: Node<T>, gap: usize) -> Vec<Child<T>> {
    let Node::Inner(children) = node else {
        unreachable!("a node taller than the tree is an inner node");
    };
    if gap == 1 {
        children.push(tree.into_child());
    } else {
        let last = children.last_mut().expect("an inner node has children");
        let extras = hang_right(&mut last.node, tree, gap - 1);
        last.refresh();
        children.extend(extras);
    }
    overflow(children)
}

/// Hang `tree` (`gap` levels shorter than `node`) from the left edge of
/// `node`. Returns the siblings that follow `node` when it overflowed.
fn hang_left<T: Item>(node: &mut Node<T>, tree: Node<T>, gap: usize) -> Vec<Child<T>> {
    let Node::Inner(children) = node else {
        unreachable!("a node taller than the tree is an inner node");
    };
    if gap == 1 {
        children.insert(0, tree.into_child());
    } else {
        let first = children.first_mut().expect("an inner node has children");
        let extras = hang_left(&mut first.node, tree, gap - 1);
        first.refresh();
        children.splice(1..1, extras);
    }
    overflow(children)
}

/// Keep the first `INNER_FILL` children and hand the rest back, grouped, as
/// the siblings that follow, when there are more than `INNER_MAX`.
fn overflow<T: Item>(children: &mut Vec<Child<T>>) -> Vec<Child<T>> {
    if children.len() <= INNER_MAX {
        Vec::new()
    } else {
        group(children.split_off(INNER_FILL))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq)]
    struct Num(i64);

    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    struct Total(i64);

    impl Summary for Total {
        fn add(&mut self, other: &Self) {
            self.0 += other.0;
        }
    }

    impl Item for Num {
        type Summary = Total;

        fn summary(&self) -> Total {
            Total(self.0)
        }
    }

    #[derive(Clone, Copy, Default, PartialEq, PartialOrd)]
    struct Sum(i64);

    impl Add for Sum {
        type Output = Sum;

        fn add(self, other: Sum) -> Sum {
            Sum(self.0 + other.0)
        }
    }

    impl Dimension<Total> for Sum {
        fn from_summary(summary: &Total) -> Self {
            Sum(summary.0)
        }
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
        }

        /// A small item value.
        fn value(&mut self) -> i64 {
            i64::try_from(self.below(9)).expect("small")
        }
    }

    /// Everything the tree answers, against a plain vector.
    fn check(tree: &SumTree<Num>, model: &[i64]) {
        assert_eq!(tree.len(), model.len());
        assert_eq!(tree.summary().0, model.iter().sum::<i64>());
        let items: Vec<i64> = tree.iter().map(|n| n.0).collect();
        assert_eq!(items, model);
        for (ix, value) in model.iter().enumerate().step_by(7) {
            assert_eq!(tree.get(ix).map(|n| n.0), Some(*value));
            assert_eq!(tree.prefix::<Sum>(ix).0, model[..ix].iter().sum::<i64>());
            let from: Vec<i64> = tree.iter_from(ix).map(|n| n.0).collect();
            assert_eq!(from, model[ix..]);
        }
        assert_eq!(tree.prefix::<Sum>(model.len()).0, model.iter().sum::<i64>());
        assert!(tree.get(model.len()).is_none());
        // The values are non-negative, so running sums never decrease.
        let mut running = 0;
        let cumulative: Vec<i64> = model
            .iter()
            .map(|v| {
                running += v;
                running
            })
            .collect();
        for target in (0..=running).step_by(5) {
            let expected = cumulative.partition_point(|&c| c <= target);
            assert_eq!(tree.seek(Sum(target)), expected, "seek {target}");
        }
    }

    #[test]
    fn matches_a_vector_through_random_edits() {
        for seed in 1..=60u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let start: Vec<i64> = (0..rng.below(400)).map(|_| rng.value()).collect();
            let mut model = start.clone();
            let mut tree = SumTree::from_vec(start.into_iter().map(Num).collect());
            check(&tree, &model);
            for _ in 0..120 {
                match rng.below(5) {
                    0 if !model.is_empty() => {
                        let ix = rng.below(model.len());
                        let value = rng.value();
                        model[ix] = value;
                        tree.update(ix, |n| n.0 = value);
                    }
                    _ => {
                        let len = model.len();
                        let a = rng.below(len + 1);
                        let wide = if rng.below(8) == 0 { 300 } else { 6 };
                        let b = (a + rng.below(wide)).min(len);
                        let wide = if rng.below(8) == 0 { 200 } else { 5 };
                        let new: Vec<i64> = (0..rng.below(wide)).map(|_| rng.value()).collect();
                        model.splice(a..b, new.iter().copied());
                        tree.splice(a..b, new.into_iter().map(Num).collect());
                    }
                }
                check(&tree, &model);
            }
        }
    }

    #[test]
    fn many_small_inserts_in_one_place_split_leaves() {
        let mut tree = SumTree::new();
        let mut model = Vec::new();
        for ix in 0..5_000 {
            tree.splice(0..0, vec![Num(ix % 7)]);
            model.insert(0, ix % 7);
        }
        check(&tree, &model);
        for _ in 0..4_990 {
            tree.splice(10..11, Vec::new());
            model.remove(10);
        }
        check(&tree, &model);
    }

    /// Panic unless the tree is well formed: every leaf at one depth, every
    /// node's count and summary equal to what is below it, no empty node
    /// below the root, and the root agreeing with `len` and `summary`.
    fn validate(tree: &SumTree<Num>) {
        fn walk(node: &Node<Num>, is_root: bool) -> (usize, i64, usize) {
            match node {
                Node::Leaf(items) => {
                    assert!(is_root || !items.is_empty(), "empty leaf below the root");
                    (items.len(), items.iter().map(|n| n.0).sum(), 0)
                }
                Node::Inner(children) => {
                    assert!(!children.is_empty(), "inner node without children");
                    assert!(children.len() <= INNER_MAX, "{} children", children.len());
                    let mut depth = None;
                    let (mut count, mut sum) = (0, 0);
                    for child in children {
                        let (c, s, d) = walk(&child.node, false);
                        assert_eq!(child.count, c, "a child's count");
                        assert_eq!(child.summary.0, s, "a child's summary");
                        assert!(c > 0, "empty child");
                        assert!(depth.is_none_or(|known| known == d), "leaves at two depths");
                        depth = Some(d);
                        count += c;
                        sum += s;
                    }
                    (count, sum, depth.expect("children") + 1)
                }
            }
        }
        let (count, sum, _) = walk(&tree.root, true);
        assert_eq!(count, tree.len, "the tree's length");
        assert_eq!(sum, tree.summary.0, "the tree's summary");
        if let Node::Leaf(items) = &tree.root {
            assert!(items.len() <= LEAF_MAX);
        }
    }

    /// `check` for big trees: the same answers, but only at a few hundred
    /// positions, since `check` is quadratic in the length.
    fn check_sampled(tree: &SumTree<Num>, model: &[i64]) {
        assert_eq!(tree.len(), model.len());
        assert_eq!(tree.summary().0, model.iter().sum::<i64>());
        assert!(tree.iter().map(|n| n.0).eq(model.iter().copied()));
        let mut running = 0;
        let cumulative: Vec<i64> = model
            .iter()
            .map(|v| {
                running += v;
                running
            })
            .collect();
        let step = (model.len() / 300).max(1);
        for ix in (0..model.len()).step_by(step) {
            let before = if ix == 0 { 0 } else { cumulative[ix - 1] };
            assert_eq!(tree.get(ix).map(|n| n.0), Some(model[ix]));
            assert_eq!(tree.prefix::<Sum>(ix).0, before);
            assert!(
                tree.iter_from(ix)
                    .take(5)
                    .map(|n| n.0)
                    .eq(model[ix..].iter().copied().take(5))
            );
            assert_eq!(
                tree.seek(Sum(before)),
                cumulative.partition_point(|&c| c <= before)
            );
        }
    }

    fn tree_of(model: &[i64]) -> SumTree<Num> {
        SumTree::from_vec(model.iter().copied().map(Num).collect())
    }

    #[test]
    fn split_and_append_round_trip_at_every_size() {
        for len in [
            0usize, 1, 2, 23, 24, 25, 31, 32, 33, 100, 383, 384, 385, 1_000, 6_000,
        ] {
            let model: Vec<i64> = (0..len)
                .map(|i| i64::try_from(i % 9).expect("small"))
                .collect();
            for at in [0, 1, len / 3, len / 2, len.saturating_sub(1), len] {
                if at > len {
                    continue;
                }
                let mut left = tree_of(&model);
                let right = left.split_off(at);
                validate(&left);
                validate(&right);
                check(&left, &model[..at]);
                check(&right, &model[at..]);
                left.append(right);
                validate(&left);
                check(&left, &model);
            }
        }
    }

    #[test]
    fn splice_tree_matches_a_vector_through_random_edits() {
        for seed in 1..=40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            // Sizes that are a few leaves, a few levels and in between, so
            // that the trees joined have heights that differ by 0, 1 and 2.
            let sizes = [0usize, 3, 40, 500, 4_000];
            let start: Vec<i64> = (0..sizes[rng.below(sizes.len())])
                .map(|_| rng.value())
                .collect();
            let mut model = start.clone();
            let mut tree = tree_of(&start);
            for _ in 0..25 {
                let len = model.len();
                let a = rng.below(len + 1);
                let b = if rng.below(3) == 0 {
                    len
                } else {
                    let wide = if rng.below(4) == 0 { 2_000 } else { 30 };
                    (a + rng.below(wide)).min(len)
                };
                let new: Vec<i64> = (0..sizes[rng.below(sizes.len())])
                    .map(|_| rng.value())
                    .collect();
                let removed: Vec<i64> = model.splice(a..b, new.iter().copied()).collect();
                let taken = tree.splice_tree(a..b, tree_of(&new));
                validate(&tree);
                validate(&taken);
                check(&taken, &removed);
                check(&tree, &model);
                // A plain splice and a small edit still work on what the
                // joins left behind.
                if rng.below(2) == 0 && !model.is_empty() {
                    let ix = rng.below(model.len());
                    let value = rng.value();
                    model[ix] = value;
                    tree.update(ix, |n| n.0 = value);
                    let at = rng.below(model.len() + 1);
                    model.splice(at..at, [value, value]);
                    tree.splice(at..at, vec![Num(value), Num(value)]);
                    validate(&tree);
                    check(&tree, &model);
                }
            }
        }
    }

    #[test]
    fn replacing_everything_or_nothing_with_a_tree() {
        let model: Vec<i64> = (0..1_000).map(|i| i % 5).collect();
        let mut tree = tree_of(&model);
        let removed = tree.splice_tree(0..0, tree_of(&[]));
        assert!(removed.is_empty());
        check(&tree, &model);
        let removed = tree.splice_tree(0..1_000, tree_of(&[7, 7]));
        check(&removed, &model);
        check(&tree, &[7, 7]);
        let removed = tree.splice_tree(0..2, tree_of(&model));
        check(&removed, &[7, 7]);
        check(&tree, &model);
        // Into an empty tree.
        let mut empty = SumTree::<Num>::new();
        let _ = empty.splice_tree(0..0, tree_of(&model));
        validate(&empty);
        check(&empty, &model);
    }

    #[test]
    fn joins_stay_shallow_and_cheap_to_query() {
        // A hundred thousand items pasted into the middle of a larger tree:
        // depth stays that of a B-tree and a query still works.
        let big: Vec<i64> = (0..200_000).map(|i| i % 3).collect();
        let pasted: Vec<i64> = (0..143_000).map(|i| i % 4).collect();
        let mut model = big.clone();
        let mut tree = tree_of(&big);
        for round in 0..20 {
            let at = (round * 7_919) % model.len();
            model.splice(at..at, pasted.iter().copied());
            let _ = tree.splice_tree(at..at, tree_of(&pasted));
            if round % 5 == 4 {
                validate(&tree);
                assert!(height(&tree.root) <= 6, "height {}", height(&tree.root));
            }
        }
        check_sampled(&tree, &model);
    }

    #[test]
    fn empty_tree() {
        let tree = SumTree::<Num>::new();
        assert!(tree.is_empty());
        assert_eq!(tree.get(0), None);
        assert_eq!(tree.seek(Sum(10)), 0);
        assert_eq!(tree.prefix::<Sum>(0).0, 0);
        assert_eq!(tree.iter().count(), 0);
        assert_eq!(tree.iter_from(3).count(), 0);
    }

    #[test]
    fn deep_trees_stay_shallow() {
        let items: Vec<Num> = (0..100_000).map(|_| Num(1)).collect();
        let tree = SumTree::from_vec(items);
        let mut depth = 1;
        let mut node = &tree.root;
        while let Node::Inner(children) = node {
            depth += 1;
            node = &children[0].node;
        }
        assert!(depth <= 5, "depth {depth}");
        assert_eq!(tree.seek(Sum(49_999)), 49_999);
        assert_eq!(tree.prefix::<Sum>(77_777).0, 77_777);
    }
}
