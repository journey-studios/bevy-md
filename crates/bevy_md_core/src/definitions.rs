//! The index of link reference definitions (`[x]: url`) and footnote
//! definitions (`[^x]: text`).
//!
//! Both resolve across the whole document: a definition far below decides
//! whether `[text][x]` above it is a link, and where it goes. Before this
//! index every edit of a document holding one was a full parse. The index
//! keeps what a window reparse needs to stay a window reparse:
//!
//! - **`defs`**: the definitions as the parser sees them (the first of
//!   several with one label wins, the rest are invisible), at their source
//!   offsets, plus two maps by label (link destination; footnote count).
//!   A window reparse gets the definitions *outside* the window through
//!   pulldown-cmark's broken-link callback, so `[a][b]` in the window
//!   resolves against a definition elsewhere.
//! - **`uses`**: for each block that mentions a label (a reference link,
//!   resolved or not, or text shaped like a footnote reference), the labels.
//!   When an edit adds, removes or changes a definition, the blocks that use
//!   its label are refreshed; nothing else is.
//! - **`cands`**: every place that *looks* like a definition (`[label]:`), by
//!   label, definition or not, visible or hidden. pulldown-cmark hides the
//!   second definition of a label, so the parser alone cannot say whether
//!   removing the first one reveals another. A label that is also defined
//!   (or looks defined) somewhere else is the one case that falls back to a
//!   full parse; so does a label that cannot be keyed (a `[label]:` that
//!   spans lines), when a definition is in play.
//!
//! Positions are kept as distances ([`Positioned`]), so an edit moves none of
//! them and the work stays proportional to the window. Hashes of labels use
//! the same case folding as the parser ([`UniCase`]); a collision can only
//! cost an extra refresh, never a wrong result, except in the destination
//! map, which compares the labels themselves.

use std::hash::{Hash, Hasher};
use std::ops::Range;

use rustc_hash::{FxHashMap, FxHasher};
use unicase::UniCase;

use crate::positions::Positioned;

/// The key of a `[label]:` that cannot be keyed: it spans lines, and the
/// parser's label would drop the container prefixes in between.
pub(crate) const UNKEYED: u64 = 0;

const NOTE_SALT: u64 = 0x9E37_79B9_7F4A_7C15;

/// Case-folded hash of a label, as the parser compares labels.
pub(crate) fn label_key(label: &str) -> u64 {
    let mut hasher = FxHasher::default();
    UniCase::new(label).hash(&mut hasher);
    // 0 is reserved for `UNKEYED`.
    hasher.finish().max(1)
}

/// What a block depends on when it mentions `label` as a link reference.
pub(crate) fn link_use(label: &str) -> u64 {
    label_key(label)
}

/// What a block depends on when it mentions `[^label]`.
pub(crate) fn note_use(label: &str) -> u64 {
    label_key(label) ^ NOTE_SALT
}

/// What a definition is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum DefKind {
    /// `[label]: destination`
    Link(Box<str>),
    /// `[^label]: text`
    Note,
}

/// A definition the parser saw.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Def {
    pub label: Box<str>,
    pub kind: DefKind,
}

impl Def {
    /// What blocks mentioning this definition's label depend on.
    fn use_key(&self) -> u64 {
        match self.kind {
            DefKind::Link(_) => link_use(&self.label),
            DefKind::Note => note_use(&self.label),
        }
    }

    /// The key of the text `[label]:` that spells this definition.
    fn candidate_key(&self) -> u64 {
        match self.kind {
            DefKind::Link(_) => label_key(&self.label),
            DefKind::Note => label_key(&format!("^{}", self.label)),
        }
    }
}

/// Everything a parse of a slice found about definitions: what the index
/// needs to take it in.
#[derive(Debug, Default)]
pub(crate) struct Found {
    /// Definitions, in source order.
    pub defs: Vec<(usize, Def)>,
    /// Definition-shaped text, in source order, keyed.
    pub cands: Vec<(usize, u64)>,
    /// Blocks that mention labels: the block's start and the labels.
    pub uses: Vec<(usize, Box<[u64]>)>,
}

/// Labels of the definitions in a window of the document, as the index has
/// them.
#[derive(Debug, Default)]
pub(crate) struct WindowDefs {
    pub defs: Vec<(usize, Def)>,
    pub cands: Vec<(usize, u64)>,
}

/// What to do about a window whose definitions changed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    /// A definition's label is also defined, or looks defined, elsewhere,
    /// which a window cannot decide: parse everything.
    Full,
    /// Take the window in; refresh the blocks mentioning these use keys
    /// first (none when no definition changed).
    Apply { changed: Vec<u64> },
}

/// Counts and destinations by label: labels hashed, then compared as text.
#[derive(Clone, Debug, Default)]
struct ByLabel<T: Clone + std::fmt::Debug>(FxHashMap<u64, Vec<(Box<str>, T)>>);

impl<T: Clone + std::fmt::Debug> ByLabel<T> {
    fn get(&self, label: &str) -> Option<&T> {
        self.0
            .get(&label_key(label))?
            .iter()
            .find(|(known, _)| UniCase::new(&**known) == UniCase::new(label))
            .map(|(_, value)| value)
    }

    fn entry(&mut self, label: &str, default: T) -> &mut T {
        let bucket = self.0.entry(label_key(label)).or_default();
        let at = bucket
            .iter()
            .position(|(known, _)| UniCase::new(&**known) == UniCase::new(label))
            .unwrap_or_else(|| {
                bucket.push((label.into(), default));
                bucket.len() - 1
            });
        &mut bucket[at].1
    }

    fn remove(&mut self, label: &str) {
        let key = label_key(label);
        if let Some(bucket) = self.0.get_mut(&key) {
            bucket.retain(|(known, _)| UniCase::new(&**known) != UniCase::new(label));
            if bucket.is_empty() {
                self.0.remove(&key);
            }
        }
    }

    fn len(&self) -> usize {
        self.0.values().map(Vec::len).sum()
    }
}

/// The definitions of a document, where they are, and who mentions them.
#[derive(Clone, Debug, Default)]
pub(crate) struct DefIndex {
    defs: Positioned<Def>,
    cands: Positioned<u64>,
    uses: Positioned<Box<[u64]>>,
    /// Destination of the winning definition of each link label.
    links: ByLabel<Box<str>>,
    /// How many definitions each footnote label has.
    notes: ByLabel<u32>,
    cand_counts: FxHashMap<u64, u32>,
}

/// What `defs` say about the label of `def` (keyed `key`): the definition's
/// kind, if there is one.
fn state<'d>(defs: &'d [(usize, Def)], def: &Def, key: u64) -> Option<&'d DefKind> {
    defs.iter()
        .find(|(_, other)| {
            other.use_key() == key && UniCase::new(&*other.label) == UniCase::new(&*def.label)
        })
        .map(|(_, other)| &other.kind)
}

impl DefIndex {
    /// Start over from what a parse of the whole document found.
    pub fn rebuild(&mut self, found: Found) {
        *self = Self::default();
        for (_, def) in &found.defs {
            self.add(def);
        }
        for &(_, key) in &found.cands {
            *self.cand_counts.entry(key).or_default() += 1;
        }
        self.defs = Positioned::from_items(found.defs);
        self.cands = Positioned::from_items(found.cands);
        self.uses = Positioned::from_items(found.uses);
    }

    fn add(&mut self, def: &Def) {
        match &def.kind {
            DefKind::Link(dest) => {
                *self.links.entry(&def.label, dest.clone()) = dest.clone();
            }
            DefKind::Note => *self.notes.entry(&def.label, 0) += 1,
        }
    }

    fn remove(&mut self, def: &Def) {
        match def.kind {
            DefKind::Link(_) => self.links.remove(&def.label),
            DefKind::Note => {
                let count = self.notes.entry(&def.label, 0);
                *count = count.saturating_sub(1);
                if *count == 0 {
                    self.notes.remove(&def.label);
                }
            }
        }
    }

    /// The definitions and definition-shaped text starting in `range`.
    pub fn window(&self, range: &Range<usize>) -> WindowDefs {
        WindowDefs {
            defs: self
                .defs
                .in_range(range)
                .into_iter()
                .map(|(pos, def)| (pos, def.clone()))
                .collect(),
            cands: self
                .cands
                .in_range(range)
                .into_iter()
                .map(|(pos, key)| (pos, *key))
                .collect(),
        }
    }

    /// The destination of the definition of link `label`, unless it is one
    /// of `except` (labels of definitions in the window being reparsed,
    /// which the parser reads itself).
    pub fn resolve_outside(&self, label: &str, except: &WindowDefs) -> Option<Box<str>> {
        let dest = self.links.get(label)?;
        let inside = except.defs.iter().any(|(_, def)| {
            matches!(def.kind, DefKind::Link(_)) && UniCase::new(&*def.label) == UniCase::new(label)
        });
        (!inside).then(|| dest.clone())
    }

    /// Whether footnote `label` is defined outside the window `except`.
    pub fn note_outside(&self, label: &str, except: &WindowDefs) -> bool {
        let inside = except
            .defs
            .iter()
            .filter(|(_, def)| {
                matches!(def.kind, DefKind::Note)
                    && UniCase::new(&*def.label) == UniCase::new(label)
            })
            .count();
        self.notes
            .get(label)
            .is_some_and(|&count| count as usize > inside)
    }

    /// Whether a window with `old` definitions now has `new` ones can be
    /// taken in on its own, and which labels' dependents it changes.
    pub fn plan(&self, old: &WindowDefs, new: &Found) -> Plan {
        let touched: Vec<&Def> = old
            .defs
            .iter()
            .chain(&new.defs)
            .map(|(_, def)| def)
            .collect();
        if touched.is_empty() {
            return Plan::Apply {
                changed: Vec::new(),
            };
        }
        // A `[label]:` that cannot be keyed might be any label.
        let unkeyed_inside = old
            .cands
            .iter()
            .chain(&new.cands)
            .any(|&(_, key)| key == UNKEYED);
        let unkeyed_old_inside = old.cands.iter().filter(|&&(_, key)| key == UNKEYED).count();
        let unkeyed_outside = self
            .cand_counts
            .get(&UNKEYED)
            .copied()
            .unwrap_or(0)
            .saturating_sub(u32::try_from(unkeyed_old_inside).unwrap_or(u32::MAX));
        if unkeyed_inside || unkeyed_outside > 0 {
            return Plan::Full;
        }
        for def in &touched {
            let key = def.candidate_key();
            let known = self.cand_counts.get(&key).copied().unwrap_or(0);
            let inside = old.cands.iter().filter(|&&(_, k)| k == key).count();
            if usize::try_from(known).unwrap_or(usize::MAX) > inside {
                return Plan::Full;
            }
        }
        let mut changed = Vec::new();
        for (_, def) in old.defs.iter().chain(&new.defs) {
            let key = def.use_key();
            if changed.contains(&key) {
                continue;
            }
            if state(&old.defs, def, key) != state(&new.defs, def, key) {
                changed.push(key);
            }
        }
        Plan::Apply { changed }
    }

    /// Take in what a parse of the window `old` (as it was) found now that it
    /// holds `new`, the text in it having grown by `delta` bytes.
    pub fn replace(&mut self, old: &Range<usize>, delta: isize, previous: &WindowDefs, new: Found) {
        for (_, def) in &previous.defs {
            self.remove(def);
        }
        for &(_, key) in &previous.cands {
            if let Some(count) = self.cand_counts.get_mut(&key) {
                *count -= 1;
                if *count == 0 {
                    self.cand_counts.remove(&key);
                }
            }
        }
        for (_, def) in &new.defs {
            self.add(def);
        }
        for &(_, key) in &new.cands {
            *self.cand_counts.entry(key).or_default() += 1;
        }
        self.defs.replace(old, delta, new.defs);
        self.cands.replace(old, delta, new.cands);
        self.uses.replace(old, delta, new.uses);
    }

    /// Where the blocks that mention any of `keys` start, in order.
    pub fn users(&self, keys: &[u64]) -> Vec<usize> {
        self.uses
            .iter()
            .filter(|(_, labels)| labels.iter().any(|label| keys.contains(label)))
            .map(|(pos, _)| pos)
            .collect()
    }

    /// The destination of link `label`.
    pub fn link(&self, label: &str) -> Option<&str> {
        self.links.get(label).map(|dest| &**dest)
    }

    /// Whether footnote `label` has a definition.
    pub fn has_note(&self, label: &str) -> bool {
        self.notes.get(label).is_some_and(|&count| count > 0)
    }

    /// The definitions in order, with their offsets.
    pub fn definitions(&self) -> impl Iterator<Item = (usize, &Def)> {
        self.defs.iter()
    }

    /// The use entries in order (for tests).
    pub fn uses(&self) -> impl Iterator<Item = (usize, &[u64])> {
        self.uses.iter().map(|(pos, labels)| (pos, &**labels))
    }

    /// The definition-shaped text, in order (for tests).
    pub fn candidates(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        self.cands.iter().map(|(pos, key)| (pos, *key))
    }

    /// How many labels the maps hold (for tests).
    pub fn label_counts(&self) -> (usize, usize) {
        (self.links.len(), self.notes.len())
    }
}

/// ASCII punctuation, which a backslash can escape.
fn is_punct(byte: u8) -> bool {
    byte.is_ascii_punctuation()
}

/// The label as the parser keys it: runs of whitespace become one space, and
/// the ends are trimmed.
pub(crate) fn normalize_label(raw: &str) -> String {
    let mut label = String::with_capacity(raw.len());
    let mut in_space = false;
    for ch in raw.chars() {
        if matches!(ch, '\t'..='\r' | ' ') {
            in_space = true;
        } else {
            if in_space && !label.is_empty() {
                label.push(' ');
            }
            in_space = false;
            label.push(ch);
        }
    }
    label
}

/// Text shaped like a definition: `[label]:`, as far as one line shows. A
/// superset of what the parser reads as one (code blocks and paragraph text
/// included); `base` is the offset of `text` in the document.
///
/// A `[` whose line ends before any `]` might open a label that goes on the
/// next line, which the parser allows and a line cannot key: it counts as
/// [`UNKEYED`]. Looking one line at a time is what makes a scan of a
/// window (which starts and ends at line boundaries) find what a scan of the
/// whole document finds in the same lines.
pub(crate) fn scan_candidates(text: &str, base: usize) -> Vec<(usize, u64)> {
    let bytes = text.as_bytes();
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = bytes[from..]
        .iter()
        .position(|&b| b == b'[')
        .map(|p| p + from)
    {
        from = at + 1;
        let mut close = None;
        let mut open_ended = false;
        let mut j = at + 1;
        loop {
            match bytes.get(j) {
                None | Some(b'\n' | b'\r') => {
                    open_ended = true;
                    break;
                }
                Some(_) if j - at > 1100 => break,
                Some(b'[') => break,
                Some(b']') => {
                    close = Some(j);
                    break;
                }
                Some(b'\\') if bytes.get(j + 1).is_some_and(|&b| is_punct(b)) => j += 2,
                Some(_) => j += 1,
            }
        }
        if open_ended {
            found.push((base + at, UNKEYED));
            continue;
        }
        let Some(close) = close else { continue };
        if bytes.get(close + 1) != Some(&b':') {
            continue;
        }
        found.push((base + at, label_key(&normalize_label(&text[at + 1..close]))));
    }
    found
}

/// The labels of text shaped like a footnote reference (`[^label]`) in
/// `text`, normalized, each once. A superset of what the parser reads as
/// one.
pub(crate) fn scan_note_labels(text: &str) -> Vec<String> {
    let mut labels = Vec::new();
    for_each_note_label(text, |label| {
        if !labels.iter().any(|known| known == label) {
            labels.push(label.to_owned());
        }
    });
    labels
}

/// The use keys of the footnote-shaped text in `text`.
pub(crate) fn scan_note_uses(text: &str, out: &mut Vec<u64>) {
    for_each_note_label(text, |label| out.push(note_use(label)));
}

fn for_each_note_label(text: &str, mut each: impl FnMut(&str)) {
    let mut rest = text;
    while let Some(at) = rest.find("[^") {
        rest = &rest[at + 2..];
        let bytes = rest.as_bytes();
        let mut j = 0;
        let mut close = None;
        while j < bytes.len() && j <= 1100 {
            match bytes[j] {
                b'[' => break,
                b']' => {
                    close = Some(j);
                    break;
                }
                b'\\' if bytes.get(j + 1).is_some_and(|&b| is_punct(b)) => j += 2,
                _ => j += 1,
            }
        }
        let Some(close) = close else { continue };
        let raw = &rest[..close];
        let label = normalize_label(raw);
        if label.is_empty() {
            continue;
        }
        each(&label);
        // In a table the parser reads `\|` as `|`.
        if raw.contains("\\|") {
            each(&normalize_label(&raw.replace("\\|", "|")));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_normalize_like_the_parser() {
        assert_eq!(normalize_label("  a \t b\n c  "), "a b c");
        assert_eq!(label_key("Foo"), label_key("fOO"));
        assert_eq!(label_key("ẞ"), label_key("ss"));
        assert_ne!(label_key("a"), label_key("b"));
        assert_ne!(label_key("a"), UNKEYED);
    }

    #[test]
    fn candidates_are_bracketed_labels_followed_by_a_colon() {
        let text = "x\n[a]: /u\n[B c]:\nplain [d] and [e]: no\n[f\ng]: multi\n[^n]: note";
        let found = scan_candidates(text, 100);
        let keys: Vec<u64> = found.iter().map(|&(_, key)| key).collect();
        assert_eq!(found[0].0, 102);
        assert_eq!(keys[0], label_key("a"));
        assert_eq!(keys[1], label_key("b c"));
        assert_eq!(keys[2], label_key("e"));
        // `[f` may open a label that goes on to the next line.
        assert_eq!(keys[3], UNKEYED);
        assert_eq!(keys[4], label_key("^n"));
        assert_eq!(keys.len(), 5);
    }

    #[test]
    fn note_uses_find_every_footnote_shaped_label() {
        let mut out = Vec::new();
        scan_note_uses("a[^1] b[^ Two ]c [^x\\]y] [^unclosed [^3]", &mut out);
        assert!(out.contains(&note_use("1")));
        assert!(out.contains(&note_use("two")));
        assert!(out.contains(&note_use("x\\]y")));
        assert!(out.contains(&note_use("3")));
        assert!(!out.contains(&note_use("unclosed")));
    }

    #[test]
    fn replace_keeps_offsets_after_the_window_moving_with_it() {
        let mut index = DefIndex::default();
        let def = |label: &str, dest: &str| Def {
            label: label.into(),
            kind: DefKind::Link(dest.into()),
        };
        index.rebuild(Found {
            defs: vec![(10, def("a", "/a")), (50, def("b", "/b"))],
            cands: vec![(10, label_key("a")), (50, label_key("b"))],
            uses: vec![(30, vec![link_use("a")].into())],
        });
        let old = 0..20;
        let previous = index.window(&old);
        assert_eq!(previous.defs.len(), 1);
        // The window grew by 5 bytes and `a` moved to 12.
        index.replace(
            &old,
            5,
            &previous,
            Found {
                defs: vec![(12, def("a", "/a2"))],
                cands: vec![(12, label_key("a"))],
                uses: Vec::new(),
            },
        );
        let at: Vec<_> = index
            .definitions()
            .map(|(pos, d)| (pos, &*d.label))
            .collect();
        assert_eq!(at, [(12, "a"), (55, "b")]);
        assert_eq!(index.link("A"), Some("/a2"));
        assert_eq!(index.users(&[link_use("a")]), vec![35]);
    }
}
