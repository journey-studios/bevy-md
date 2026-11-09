//! Undo is the inverse of every edit: after any sequence of edits, undoing
//! everything gives the original source byte for byte, and redoing
//! everything gives the final one.

use std::time::Duration;

use bevy_md::Markdown;
use bevy_md_editor::{Change, History, MAX_STEPS};

const SAMPLE: &str = include_str!("../../bevy_md_core/tests/fixtures/sample.md");

/// Tokens that change markdown structure the most when typed or deleted.
const TOKENS: &[&str] = &[
    "a", "b", " ", "\n", "\n\n", "# ", "- ", "1. ", "> ", "```", "*", "**", "`", "|", "---",
    "[x] ", "é", "日本", "word", "_",
];

/// xorshift64*, so the test needs no dependency and stays reproducible.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: usize) -> usize {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) % n as u64) as usize
    }
}

/// An editing session over a [`Markdown`], recording what the editor would.
struct Session {
    markdown: Markdown,
    history: History,
    caret: usize,
    now: Duration,
    rng: Rng,
}

impl Session {
    fn new(seed: u64, source: &str) -> Self {
        Self {
            markdown: Markdown::new(source),
            history: History::new(),
            caret: 0,
            now: Duration::ZERO,
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
        }
    }

    fn source(&self) -> &str {
        self.markdown.document().source()
    }

    fn boundary(&self, mut at: usize) -> usize {
        let source = self.source();
        at = at.min(source.len());
        while !source.is_char_boundary(at) {
            at -= 1;
        }
        at
    }

    /// One random edit, `gap` after the last one.
    fn edit(&mut self, gap: Duration) {
        self.now += gap;
        self.caret = self.boundary(self.caret);
        if self.rng.below(6) == 0 {
            let at = self.rng.below(self.source().len() + 1);
            self.caret = self.boundary(at);
        }
        let caret = self.caret;
        let change = match self.rng.below(4) {
            0 | 1 => {
                let text = TOKENS[self.rng.below(TOKENS.len())];
                self.markdown.edit(caret..caret, text);
                self.caret = caret + text.len();
                Change {
                    start: caret,
                    old: String::new(),
                    new: text.to_owned(),
                    caret_before: caret,
                    caret_after: self.caret,
                }
            }
            2 => {
                if caret == 0 {
                    return;
                }
                let start = self.boundary(caret - 1);
                let old = self.source()[start..caret].to_owned();
                self.markdown.edit(start..caret, "");
                self.caret = start;
                Change {
                    start,
                    old,
                    new: String::new(),
                    caret_before: caret,
                    caret_after: start,
                }
            }
            _ => {
                let source = self.source();
                if caret >= source.len() {
                    return;
                }
                let end = (caret + 1..=source.len())
                    .find(|&e| source.is_char_boundary(e))
                    .unwrap();
                let old = source[caret..end].to_owned();
                self.markdown.edit(caret..end, "");
                Change {
                    start: caret,
                    old,
                    new: String::new(),
                    caret_before: caret,
                    caret_after: caret,
                }
            }
        };
        self.history.record(change, self.now);
    }

    fn undo_all(&mut self) -> usize {
        let mut steps = 0;
        while self.history.undo(&mut self.markdown).is_some() {
            steps += 1;
        }
        steps
    }

    fn redo_all(&mut self) -> usize {
        let mut steps = 0;
        while self.history.redo(&mut self.markdown).is_some() {
            steps += 1;
        }
        steps
    }
}

fn seeds() -> u64 {
    std::env::var("BEVY_MD_SEEDS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(150)
}

#[test]
fn undoing_every_edit_gives_the_original_source_byte_for_byte() {
    for seed in 1..=seeds() {
        let mut session = Session::new(seed, SAMPLE);
        let original = SAMPLE.to_owned();
        let edits = 20 + session.rng.below(120);
        for _ in 0..edits {
            // Fast typing, normal typing and pauses: all kinds of grouping.
            let gap = [10, 40, 120, 700][session.rng.below(4)];
            session.edit(Duration::from_millis(gap));
        }
        let finished = session.source().to_owned();
        let steps = session.history.undo_len();
        assert!(
            steps <= MAX_STEPS,
            "seed {seed}: the test outgrew the limit"
        );

        assert_eq!(session.undo_all(), steps, "seed {seed}");
        assert_eq!(session.source(), original, "seed {seed}: undo all");
        session.markdown.document().assert_consistent();
        assert_eq!(session.history.redo_len(), steps);

        assert_eq!(session.redo_all(), steps, "seed {seed}");
        assert_eq!(session.source(), finished, "seed {seed}: redo all");
        session.markdown.document().assert_consistent();
    }
}

#[test]
fn every_edit_is_a_step_when_they_are_far_apart_and_each_state_comes_back() {
    for seed in 1..=seeds().min(60) {
        let mut session = Session::new(seed, "# Title\n\nSome text here.\n");
        let mut states = vec![session.source().to_owned()];
        // Edits, undos and redos, checked against a stack of every state.
        let mut undone: Vec<String> = Vec::new();
        for _ in 0..80 {
            match session.rng.below(5) {
                0..=2 => {
                    let before = session.history.undo_len();
                    session.edit(Duration::from_millis(2_000));
                    if session.history.undo_len() > before {
                        states.push(session.source().to_owned());
                        undone.clear();
                    }
                }
                3 => {
                    if session.history.undo(&mut session.markdown).is_some() {
                        undone.push(states.pop().expect("a state"));
                    }
                }
                _ => {
                    if session.history.redo(&mut session.markdown).is_some() {
                        states.push(undone.pop().expect("a state to redo"));
                    }
                }
            }
            assert_eq!(
                session.source(),
                states.last().unwrap(),
                "seed {seed}: after {} states",
                states.len()
            );
            assert_eq!(session.history.undo_len(), states.len() - 1);
            assert_eq!(session.history.redo_len(), undone.len());
        }
    }
}

#[test]
fn only_the_last_two_hundred_steps_can_be_undone() {
    let mut session = Session::new(7, "start\n");
    let mut states = vec![session.source().to_owned()];
    while states.len() < 421 {
        let before = session.history.undo_len();
        session.edit(Duration::from_millis(1_000));
        // Every accepted edit is a step of its own, a second apart; the
        // history never holds more than its limit.
        if session.source() != states.last().unwrap() {
            states.push(session.source().to_owned());
        }
        assert!(session.history.undo_len() >= before.min(MAX_STEPS - 1));
        assert!(session.history.undo_len() <= MAX_STEPS);
    }
    assert_eq!(session.history.undo_len(), MAX_STEPS);
    assert_eq!(session.undo_all(), MAX_STEPS);
    // What is left is the state 200 edits before the last one.
    assert_eq!(session.source(), states[states.len() - 1 - MAX_STEPS]);
}

#[test]
#[allow(clippy::explicit_counter_loop)]
fn undo_and_redo_of_a_group_come_back_whole() {
    let mut session = Session::new(1, "");
    let mut caret = 0;
    // Type "hello world" quickly.
    for (k, c) in "hello world".chars().enumerate() {
        let text = c.to_string();
        session.markdown.edit(caret..caret, &text);
        session.history.record(
            Change {
                start: caret,
                old: String::new(),
                new: text.clone(),
                caret_before: caret,
                caret_after: caret + 1,
            },
            Duration::from_millis(k as u64 * 60),
        );
        caret += 1;
    }
    assert_eq!(session.source(), "hello world");
    assert_eq!(session.history.undo_len(), 3);
    assert_eq!(session.history.undo(&mut session.markdown), Some(6));
    assert_eq!(session.source(), "hello ");
    assert_eq!(session.history.undo(&mut session.markdown), Some(5));
    assert_eq!(session.source(), "hello");
    assert_eq!(session.history.undo(&mut session.markdown), Some(0));
    assert_eq!(session.source(), "");
    assert_eq!(session.history.redo(&mut session.markdown), Some(5));
    assert_eq!(session.source(), "hello");
}
