//! Undo and redo as inverted splices (G10).
//!
//! Every edit of the editor is a replacement in the markdown source: the
//! bytes `start..start + old.len()` became `new`. A step keeps exactly that,
//! plus where the caret was before and after, so its inverse is the same
//! replacement the other way round and costs the size of the change, not the
//! size of the document. Undo and redo apply it with [`Markdown::edit`], so
//! only the blocks the change reaches are rendered again and the entities of
//! every other block stay.
//!
//! Typing is grouped so that one undo takes back what feels like one action:
//! edits less than half a second apart, one right after the other, and
//! inside the same word (or the same run of spaces, or of punctuation)
//! share a step. The history keeps the last 200 steps.

use std::collections::VecDeque;
use std::time::Duration;

use bevy_md::Markdown;

/// Steps kept: past this the oldest is forgotten.
pub const MAX_STEPS: usize = 200;

/// Edits further apart than this never share a step.
pub const GROUP_WINDOW: Duration = Duration::from_millis(500);

/// One replacement of the source and where the caret was around it, as an
/// edit hands it to [`History::record`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Where the replaced bytes started.
    pub start: usize,
    /// The bytes that were replaced.
    pub old: String,
    /// The bytes that took their place.
    pub new: String,
    /// The caret before the edit.
    pub caret_before: usize,
    /// The caret after it.
    pub caret_after: usize,
}

/// One entry of the history: a change, possibly several merged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    change: Change,
    /// When the last of the merged edits happened.
    at: Duration,
    /// The fixed end of the selection the change replaced, when there was
    /// one (see [`History::record_selection`]).
    anchor_before: Option<usize>,
}

impl Step {
    /// Where the replaced bytes started.
    pub fn start(&self) -> usize {
        self.change.start
    }

    /// The bytes before the change.
    pub fn old(&self) -> &str {
        &self.change.old
    }

    /// The bytes after the change.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(&self) -> &str {
        &self.change.new
    }

    /// The caret before the change.
    pub fn caret_before(&self) -> usize {
        self.change.caret_before
    }

    /// The caret after the change.
    pub fn caret_after(&self) -> usize {
        self.change.caret_after
    }

    /// The fixed end of the selection before the change: where undo puts the
    /// anchor. Equal to [`Step::caret_before`] when nothing was selected.
    pub fn anchor_before(&self) -> usize {
        self.anchor_before.unwrap_or(self.change.caret_before)
    }

    /// The source range the change covered before it happened.
    pub fn old_range(&self) -> std::ops::Range<usize> {
        self.change.start..self.change.start + self.change.old.len()
    }

    /// The source range the change covers now.
    pub fn new_range(&self) -> std::ops::Range<usize> {
        self.change.start..self.change.start + self.change.new.len()
    }
}

/// What a character is for the purpose of grouping typing into words.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Word,
    Space,
    Newline,
    Other,
}

fn class(c: char) -> Class {
    if c == '\n' || c == '\r' {
        Class::Newline
    } else if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Other
    }
}

/// The class of `text` when all of it is one class that can be grouped
/// (not a line break), else `None`.
fn uniform(text: &str) -> Option<Class> {
    let mut chars = text.chars();
    let first = class(chars.next()?);
    (first != Class::Newline && chars.all(|c| class(c) == first)).then_some(first)
}

/// The undo and redo stacks of one editor.
#[derive(Clone, Debug, Default)]
pub struct History {
    undo: VecDeque<Step>,
    redo: Vec<Step>,
    /// The next change starts a step of its own: the caret was just moved by
    /// an undo or a redo.
    sealed: bool,
}

impl History {
    /// An empty history.
    pub fn new() -> Self {
        Self::default()
    }

    /// Steps that can be undone.
    pub fn undo_len(&self) -> usize {
        self.undo.len()
    }

    /// Steps that can be redone.
    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Heap held by the history: the steps themselves and the text they keep.
    /// It grows with the text changed, never with the size of the document.
    pub fn bytes(&self) -> usize {
        self.undo
            .iter()
            .chain(self.redo.iter())
            .map(|step| {
                std::mem::size_of::<Step>()
                    + step.change.old.capacity()
                    + step.change.new.capacity()
            })
            .sum()
    }

    /// The steps that can be undone, oldest first.
    pub fn steps(&self) -> impl Iterator<Item = &Step> {
        self.undo.iter()
    }

    /// Forget everything.
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
        self.sealed = false;
    }

    /// Record an edit made at real time `now`. It joins the last step when
    /// it continues it (see the module docs), else starts a new one. Either
    /// way what could be redone is gone.
    pub fn record(&mut self, change: Change, now: Duration) {
        self.redo.clear();
        let sealed = std::mem::take(&mut self.sealed);
        if !sealed
            && let Some(top) = self.undo.back_mut()
            && merge(top, &change, now)
        {
            return;
        }
        self.undo.push_back(Step {
            change,
            at: now,
            anchor_before: None,
        });
        if self.undo.len() > MAX_STEPS {
            self.undo.pop_front();
        }
    }

    /// Record an edit that replaced the selection `anchor_before` to
    /// `change.caret_before`. It is a step of its own, never merged with the
    /// typing around it, and undo puts the whole selection back.
    pub fn record_selection(&mut self, change: Change, anchor_before: usize, now: Duration) {
        self.seal();
        self.record(change, now);
        if let Some(top) = self.undo.back_mut() {
            top.anchor_before = Some(anchor_before);
        }
        self.seal();
    }

    /// The next change starts a step of its own, whatever it is: a paste is
    /// one thing to undo, not the end of the word typed before it.
    pub fn seal(&mut self) {
        self.sealed = true;
    }

    /// Take back the last step on `markdown`. Returns where the caret goes,
    /// or `None` when there is nothing to undo, the markdown is still
    /// parsing a large change (its source is not settled), or the step no
    /// longer fits the source: something other than the editor changed the
    /// text it describes, so the history is forgotten rather than applied
    /// to the wrong bytes.
    pub fn undo(&mut self, markdown: &mut Markdown) -> Option<usize> {
        self.undo_selecting(markdown).map(|(_, caret)| caret)
    }

    /// [`History::undo`] that also says where the fixed end of the selection
    /// goes: `(anchor, caret)`.
    pub(crate) fn undo_selecting(&mut self, markdown: &mut Markdown) -> Option<(usize, usize)> {
        if markdown.is_parsing() {
            return None;
        }
        let step = self.undo.pop_back()?;
        if markdown.source().get(step.new_range()) != Some(step.new()) {
            self.clear();
            return None;
        }
        markdown.edit(step.new_range(), step.old());
        let selection = (step.anchor_before(), step.caret_before());
        self.redo.push(step);
        self.sealed = true;
        Some(selection)
    }

    /// Do again the last step that was undone. Returns where the caret goes,
    /// or `None` for the same reasons as [`History::undo`].
    pub fn redo(&mut self, markdown: &mut Markdown) -> Option<usize> {
        if markdown.is_parsing() {
            return None;
        }
        let step = self.redo.pop()?;
        if markdown.source().get(step.old_range()) != Some(step.old()) {
            self.clear();
            return None;
        }
        markdown.edit(step.old_range(), step.new());
        let caret = step.caret_after();
        self.undo.push_back(step);
        self.sealed = true;
        Some(caret)
    }
}

/// Fold `change` into `top` when it continues it. Returns whether it did.
fn merge(top: &mut Step, change: &Change, now: Duration) -> bool {
    if now.saturating_sub(top.at) > GROUP_WINDOW || top.change.caret_after != change.caret_before {
        return false;
    }
    let step = &mut top.change;
    let merged = if step.old.is_empty() && change.old.is_empty() {
        // Typing on: the new text starts where the last one ended, in the
        // same kind of characters.
        let same = change.start == step.start + step.new.len()
            && uniform(&step.new[step.new.len() - last_len(&step.new)..])
                .is_some_and(|last| uniform(&change.new) == Some(last));
        if same {
            step.new.push_str(&change.new);
        }
        same
    } else if step.new.is_empty() && change.new.is_empty() {
        let end = change.start + change.old.len();
        if end == step.start
            && matches!(
                (change.old.chars().next_back(), step.old.chars().next()),
                (Some(a), Some(b)) if class(a) == class(b) && class(a) != Class::Newline
            )
        {
            // Backspacing: what goes now is just before what went.
            step.start = change.start;
            step.old.insert_str(0, &change.old);
            true
        } else if change.start == step.start
            && matches!(
                (step.old.chars().next_back(), change.old.chars().next()),
                (Some(a), Some(b)) if class(a) == class(b) && class(a) != Class::Newline
            )
        {
            // Deleting forward: what goes now was just after what went.
            step.old.push_str(&change.old);
            true
        } else {
            false
        }
    } else {
        false
    };
    if merged {
        step.caret_after = change.caret_after;
        top.at = now;
    }
    merged
}

/// Byte length of the last character of `text`.
fn last_len(text: &str) -> usize {
    text.chars().next_back().map_or(0, char::len_utf8)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn typed(at: usize, text: &str) -> Change {
        Change {
            start: at,
            old: String::new(),
            new: text.to_owned(),
            caret_before: at,
            caret_after: at + text.len(),
        }
    }

    fn backspaced(at: usize, text: &str) -> Change {
        Change {
            start: at,
            old: text.to_owned(),
            new: String::new(),
            caret_before: at + text.len(),
            caret_after: at,
        }
    }

    fn deleted_forward(at: usize, text: &str) -> Change {
        Change {
            start: at,
            old: text.to_owned(),
            new: String::new(),
            caret_before: at,
            caret_after: at,
        }
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Type `text` one character at a time from `at`, 50 ms apart.
    fn type_in(history: &mut History, mut at: usize, text: &str, start: u64) -> u64 {
        let mut now = start;
        for c in text.chars() {
            let s = c.to_string();
            history.record(typed(at, &s), ms(now));
            at += s.len();
            now += 50;
        }
        now
    }

    #[test]
    fn typing_a_word_is_one_step() {
        let mut history = History::new();
        type_in(&mut history, 0, "hello", 0);
        assert_eq!(history.undo_len(), 1);
        let step = history.steps().next().unwrap();
        assert_eq!((step.start(), step.old(), step.new()), (0, "", "hello"));
        assert_eq!((step.caret_before(), step.caret_after()), (0, 5));
    }

    #[test]
    fn an_edit_of_a_selection_is_a_step_of_its_own_and_remembers_the_anchor() {
        let mut history = History::new();
        history.record(typed(0, "ab"), ms(0));
        // "cd" selected backwards (anchor 4, caret 2) and replaced by "e".
        let replaced = Change {
            start: 2,
            old: "cd".to_owned(),
            new: "e".to_owned(),
            caret_before: 2,
            caret_after: 3,
        };
        history.record_selection(replaced, 4, ms(10));
        history.record(typed(3, "f"), ms(20));
        assert_eq!(history.undo_len(), 3, "grouped with neither side");
        let steps: Vec<&Step> = history.steps().collect();
        assert_eq!(steps[0].anchor_before(), steps[0].caret_before());
        assert_eq!(steps[1].anchor_before(), 4);
        assert_eq!(steps[1].caret_before(), 2);
    }

    #[test]
    fn a_new_word_and_the_space_before_it_are_steps_of_their_own() {
        let mut history = History::new();
        type_in(&mut history, 0, "hello world", 0);
        let steps: Vec<&str> = history.steps().map(Step::new).collect();
        assert_eq!(steps, ["hello", " ", "world"]);
    }

    #[test]
    fn a_pause_longer_than_half_a_second_starts_a_new_step() {
        let mut history = History::new();
        history.record(typed(0, "a"), ms(0));
        history.record(typed(1, "b"), ms(500));
        assert_eq!(history.undo_len(), 1, "500 ms apart is still grouped");
        history.record(typed(2, "c"), ms(1_001));
        assert_eq!(history.undo_len(), 2, "501 ms after the last is not");
    }

    #[test]
    fn moving_the_caret_between_edits_breaks_the_group() {
        let mut history = History::new();
        history.record(typed(0, "a"), ms(0));
        // The caret went somewhere else before the next key.
        history.record(typed(10, "b"), ms(10));
        assert_eq!(history.undo_len(), 2);
    }

    #[test]
    fn backspacing_and_deleting_forward_group_within_a_word() {
        let mut history = History::new();
        // "hello" with the caret after it: backspace three times.
        history.record(backspaced(4, "o"), ms(0));
        history.record(backspaced(3, "l"), ms(40));
        history.record(backspaced(2, "l"), ms(80));
        assert_eq!(history.undo_len(), 1);
        let step = history.steps().next().unwrap();
        assert_eq!((step.start(), step.old(), step.new()), (2, "llo", ""));
        assert_eq!((step.caret_before(), step.caret_after()), (5, 2));

        // Forward deletes at the same place.
        let mut history = History::new();
        history.record(deleted_forward(2, "l"), ms(0));
        history.record(deleted_forward(2, "l"), ms(40));
        history.record(deleted_forward(2, "o"), ms(80));
        assert_eq!(history.undo_len(), 1);
        assert_eq!(history.steps().next().unwrap().old(), "llo");
    }

    #[test]
    fn typing_after_deleting_is_a_new_step() {
        let mut history = History::new();
        history.record(backspaced(0, "a"), ms(0));
        history.record(typed(0, "b"), ms(10));
        assert_eq!(history.undo_len(), 2);
    }

    #[test]
    fn the_history_keeps_the_last_two_hundred_steps() {
        let mut history = History::new();
        for k in 0..250u64 {
            // Far apart in time, so each is a step.
            history.record(typed(0, "a"), ms(k * 1_000));
        }
        assert_eq!(history.undo_len(), MAX_STEPS);
    }

    #[test]
    fn undo_and_redo_apply_the_inverse_and_move_the_caret() {
        let mut markdown = Markdown::new("one two");
        let mut history = History::new();
        // Typed "X" at 3, then deleted "two".
        markdown.edit(3..3, "X");
        history.record(typed(3, "X"), ms(0));
        markdown.edit(5..8, "");
        history.record(
            Change {
                start: 5,
                old: "two".into(),
                new: String::new(),
                caret_before: 8,
                caret_after: 5,
            },
            ms(5_000),
        );
        assert_eq!(markdown.document().source(), "oneX ");

        assert_eq!(history.undo(&mut markdown), Some(8));
        assert_eq!(markdown.document().source(), "oneX two");
        assert_eq!(history.undo(&mut markdown), Some(3));
        assert_eq!(markdown.document().source(), "one two");
        assert_eq!(history.undo(&mut markdown), None);

        assert_eq!(history.redo(&mut markdown), Some(4));
        assert_eq!(markdown.document().source(), "oneX two");
        assert_eq!(history.redo(&mut markdown), Some(5));
        assert_eq!(markdown.document().source(), "oneX ");
        assert_eq!(history.redo(&mut markdown), None);
    }

    #[test]
    fn a_new_edit_after_an_undo_drops_the_redo_stack() {
        let mut markdown = Markdown::new("a");
        let mut history = History::new();
        markdown.edit(1..1, "b");
        history.record(typed(1, "b"), ms(0));
        history.undo(&mut markdown);
        assert_eq!(history.redo_len(), 1);
        markdown.edit(1..1, "c");
        history.record(typed(1, "c"), ms(10));
        assert_eq!(history.redo_len(), 0);
        // And it did not merge into the step that was undone.
        assert_eq!(history.undo_len(), 1);
        assert_eq!(history.steps().next().unwrap().new(), "c");
    }

    #[test]
    fn the_first_edit_after_an_undo_is_a_step_of_its_own() {
        let mut history = History::new();
        let mut markdown = Markdown::new("");
        type_in(&mut history, 0, "ab", 0);
        markdown.edit(0..0, "ab");
        history.undo(&mut markdown);
        history.record(typed(0, "c"), ms(100));
        history.record(typed(1, "d"), ms(120));
        let steps: Vec<&str> = history.steps().map(Step::new).collect();
        assert_eq!(steps, ["cd"]);
    }
}
