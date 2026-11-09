//! Parsing a large change off the main thread (G12).
//!
//! A change that adds more than [`SYNC_LIMIT`] bytes (pasting a book, a model's
//! whole answer at once) is parsed on the [`AsyncComputeTaskPool`] while the
//! frames keep running on the document as it was. Anything small stays on the
//! calling thread, so a keystroke or a streamed token is on screen in the
//! frame that made it.
//!
//! The pieces:
//!
//! - **Revisions.** Every large change starts a new revision and drops the
//!   parse of the one before it, so a result is only ever applied if it is
//!   for the latest large change. A parse that was already running finishes
//!   and its result is thrown away ([`Markdown::discarded_parses`]).
//! - **Fusion.** Small changes that arrive while a parse runs (the next
//!   streamed chunks, keystrokes) are queued in order, consecutive appends
//!   merged into one, and applied together in the frame after the result.
//! - **The logical source.** [`Markdown::source`] is the text after every call
//!   so far, including the ones still waiting, so callers can keep
//!   computing byte ranges against it. It is one `Arc<String>` that the main
//!   thread reads, the parse task builds its [`Document`] on (nothing is
//!   copied to hand it over) and the installed document keeps: the text is
//!   built once and is never copied again unless a change arrives while the
//!   task still holds it, and then once.
//! - **Nothing heavy on the main thread.** The task parses the new source,
//!   works out which blocks differ from the document being shown, and
//!   estimates the height of the new ones; the main thread installs the
//!   result and the old document is dropped on the pool too.
//!
//! [`Markdown::source`]: crate::Markdown::source
//! [`Markdown::discarded_parses`]: crate::Markdown::discarded_parses

use std::ops::Range;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bevy::tasks::futures_lite::future::poll_once;
use bevy::tasks::{AsyncComputeTaskPool, Task, TaskPool, block_on};
use bevy_md_core::estimate::{Metrics, estimate_height};
use bevy_md_core::{Document, Splices};

use crate::heights::Heights;

/// Changes adding up to this many bytes are parsed on the calling thread.
pub const SYNC_LIMIT: usize = 4 * 1024;

/// Where the time of the last large change went, stage by stage, in wall
/// time. Diagnostics only (see [`Markdown::parse_timings`]); nothing reads it
/// to decide anything.
///
/// [`Markdown::parse_timings`]: crate::Markdown::parse_timings
#[derive(Clone, Copy, Debug, Default)]
pub struct ParseTimings {
    /// Main thread, in the call that made the change: building the new
    /// source.
    pub source_copy: Duration,
    /// Main thread, in the sync that starts the parse: handing the source to
    /// the task.
    pub start_clone: Duration,
    /// Worker thread: parsing the source.
    pub parse: Duration,
    /// Worker thread: diffing the new blocks against the shown ones.
    pub splice_from: Duration,
    /// Worker thread: estimating the heights of the new blocks.
    pub estimate: Duration,
    /// Main thread, in the frame that installs the result: turning the
    /// estimates into the height index and splicing it in.
    pub install_heights: Duration,
    /// Main thread: the whole `advance` of the frame that installed the
    /// result (everything before the window is rebuilt).
    pub install_advance: Duration,
    /// Bytes of the source parsed.
    pub bytes: usize,
    /// Blocks the result replaced the shown ones' with.
    pub new_blocks: usize,
}

/// What a finished parse hands back.
pub(crate) struct Parsed {
    pub document: Document,
    /// The blocks that differ from the document that was being shown, as
    /// disjoint splices from it to `document`.
    pub splices: Splices,
    /// The estimated heights of the blocks of each splice's `new` range, as
    /// an index each, ready to be hung into the live one.
    pub heights: Vec<Heights>,
    pub parse: Duration,
    pub splice_from: Duration,
    pub estimate: Duration,
}

/// A change that arrived while a parse was running.
pub(crate) enum Op {
    Edit { range: Range<usize>, text: String },
    Append(String),
}

impl Op {
    fn is_large(&self) -> bool {
        match self {
            Op::Edit { text, .. } | Op::Append(text) => text.len() > SYNC_LIMIT,
        }
    }
}

struct Job {
    revision: u64,
    task: Task<Parsed>,
    /// The result, when `wait` already took it from the task.
    finished: Option<Parsed>,
}

/// The state of the background parse of one [`Markdown`](crate::Markdown).
#[derive(Default)]
pub(crate) struct Background {
    /// Bumped by every large change: the revision a result must carry.
    revision: u64,
    /// The source after every change so far, kept only while a parse is
    /// running or queued changes wait to be applied. Shared with the parse
    /// task, which builds the document on this very allocation; a change
    /// made while the task holds it copies it first (`Arc::make_mut`).
    logical: Option<Arc<String>>,
    /// A parse of `logical` has to be started.
    wanted: bool,
    job: Option<Job>,
    /// Small changes made after the parse started, in order.
    queue: Vec<Op>,
    /// Results thrown away because a newer change came first.
    discarded: u64,
    pub timings: ParseTimings,
}

impl Background {
    /// Whether the document being shown is behind the source.
    pub fn busy(&self) -> bool {
        self.logical.is_some()
    }

    pub fn logical(&self) -> Option<&str> {
        self.logical.as_deref().map(String::as_str)
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn discarded(&self) -> u64 {
        self.discarded
    }

    pub fn is_waiting_to_start(&self) -> bool {
        self.wanted
    }

    /// Whether queued changes are all that is left to apply.
    pub fn only_queue_left(&self) -> bool {
        self.logical.is_some() && self.job.is_none() && !self.wanted
    }

    /// Parse `source`, replacing the whole text, in the background.
    pub fn begin(&mut self, source: Arc<String>) {
        self.supersede();
        self.logical = Some(source);
    }

    /// A newer large change makes the running parse and the queue moot.
    fn supersede(&mut self) {
        if self.job.take().is_some() {
            self.discarded += 1;
        }
        self.queue.clear();
        self.wanted = true;
        self.revision += 1;
    }

    /// Forget the background work: the text is being replaced outright.
    pub fn cancel(&mut self) {
        if !self.busy() {
            return;
        }
        if self.job.take().is_some() {
            self.discarded += 1;
        }
        self.queue.clear();
        self.logical = None;
        self.wanted = false;
        self.revision += 1;
    }

    /// A streamed chunk while busy: merged with the append before it.
    pub fn append(&mut self, text: &str) {
        let Some(logical) = &mut self.logical else {
            return;
        };
        writable(logical).push_str(text);
        match self.queue.last_mut() {
            Some(Op::Append(merged)) => merged.push_str(text),
            _ => self.queue.push(Op::Append(text.to_owned())),
        }
    }

    /// An edit while busy. A large one starts a new parse of everything.
    pub fn edit(&mut self, range: Range<usize>, text: &str) {
        let Some(logical) = &mut self.logical else {
            return;
        };
        writable(logical).replace_range(range.clone(), text);
        if text.len() > SYNC_LIMIT {
            self.supersede();
        } else {
            self.queue.push(Op::Edit {
                range,
                text: text.to_owned(),
            });
        }
    }

    /// Start the parse of the logical source, if one is wanted.
    pub fn start(&mut self, shown: &Arc<Document>, metrics: Metrics) {
        if !self.wanted {
            return;
        }
        let clone_started = Instant::now();
        // The task shares the text: a handle, not a copy.
        let Some(source) = self.logical.as_ref().map(Arc::clone) else {
            self.wanted = false;
            return;
        };
        self.timings.start_clone = clone_started.elapsed();
        self.timings.bytes = source.len();
        self.wanted = false;
        // The source being parsed already has every change made so far: what
        // is queued is part of it, and only what comes after is applied on
        // the result.
        self.queue.clear();
        let old = Arc::clone(shown);
        let task = AsyncComputeTaskPool::get_or_init(TaskPool::default).spawn(async move {
            let started = Instant::now();
            let document = Document::parse_shared(source);
            let parse = started.elapsed();
            let started = Instant::now();
            let splices = document.splice_from(&old);
            let splice_from = started.elapsed();
            let started = Instant::now();
            let heights = splices
                .iter()
                .map(|splice| {
                    Heights::estimated(
                        document
                            .blocks_from(splice.new.start)
                            .take(splice.new.len())
                            .map(|block| estimate_height(block.block(), &metrics)),
                    )
                })
                .collect();
            let estimate = started.elapsed();
            Parsed {
                document,
                splices,
                heights,
                parse,
                splice_from,
                estimate,
            }
        });
        self.job = Some(Job {
            revision: self.revision,
            task,
            finished: None,
        });
    }

    /// Wait for the running parse to finish (starting it first if it is
    /// still to be started); `poll` then returns its result. See
    /// [`Markdown::wait_for_parse`](crate::Markdown::wait_for_parse).
    pub fn wait(&mut self, shown: &Arc<Document>, metrics: Metrics) {
        self.start(shown, metrics);
        if let Some(job) = &mut self.job
            && job.finished.is_none()
        {
            job.finished = Some(block_on(&mut job.task));
        }
    }

    /// The result of the parse, once it is done, if it is still wanted.
    pub fn poll(&mut self) -> Option<Parsed> {
        let job = self.job.as_mut()?;
        let parsed = match job.finished.take() {
            Some(parsed) => parsed,
            None => block_on(poll_once(&mut job.task))?,
        };
        let revision = job.revision;
        self.job = None;
        if revision != self.revision {
            self.discarded += 1;
            return None;
        }
        Some(parsed)
    }

    /// The queued changes to apply on the document the result installed, or
    /// none when one of them is large: then a parse of the whole is wanted
    /// and the queue is part of it.
    pub fn take_queue(&mut self) -> Vec<Op> {
        let ops = std::mem::take(&mut self.queue);
        if ops.iter().any(Op::is_large) {
            self.wanted = true;
            self.revision += 1;
            return Vec::new();
        }
        ops
    }

    /// The changes in the queue are about to be applied to the document the
    /// result installed, which holds the same text as `logical` minus them:
    /// let go of this handle first, so that the first of them edits the text
    /// in place instead of copying it because two owners hold it.
    pub fn release_source(&mut self) {
        debug_assert!(!self.wanted && self.job.is_none());
        self.logical = None;
    }

    /// Everything the source asked for is on the document.
    pub fn finish(&mut self) {
        if !self.wanted && self.job.is_none() && self.queue.is_empty() {
            self.logical = None;
        }
    }
}

/// The text of `logical` to write to. When the parse task still holds the
/// allocation the text is copied first, with room to grow so that the next
/// few changes do not move it again: the task keeps the text it was given,
/// and this side carries on with its own.
fn writable(logical: &mut Arc<String>) -> &mut String {
    if Arc::get_mut(logical).is_none() {
        let mut copy = String::with_capacity(logical.len() + logical.len() / 8 + 4096);
        copy.push_str(logical);
        *logical = Arc::new(copy);
    }
    Arc::get_mut(logical).expect("the only handle: the shared one was replaced")
}

/// Hand a value to the pool to drop: freeing a document of a hundred thousand
/// blocks is as long as it sounds.
pub(crate) fn drop_in_background<T: Send + 'static>(value: T) {
    AsyncComputeTaskPool::get_or_init(TaskPool::default)
        .spawn(async move { drop(value) })
        .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn busy(source: &str) -> Background {
        let mut background = Background::default();
        background.begin(Arc::new(source.to_owned()));
        background
    }

    #[test]
    fn starting_a_parse_takes_the_queue_into_it() {
        let mut background = busy("abc");
        background.append("d");
        background.edit(0..1, "X");
        let document = Arc::new(Document::new());
        background.start(&document, Metrics::default());
        assert!(
            background.queue.is_empty(),
            "they are in the source being parsed"
        );
        assert_eq!(background.logical(), Some("Xbcd"));
        background.append("e");
        assert_eq!(
            background.queue.len(),
            1,
            "what comes after is applied after"
        );
    }

    #[test]
    fn a_large_change_starts_a_revision_and_wants_a_parse() {
        let background = busy("abc");
        assert!(background.busy());
        assert!(background.is_waiting_to_start());
        assert_eq!(background.revision(), 1);
        assert_eq!(background.logical(), Some("abc"));
    }

    #[test]
    fn appends_while_busy_are_merged_into_one() {
        let mut background = busy("abc");
        background.append("d");
        background.append("e");
        background.append("f");
        assert_eq!(background.logical(), Some("abcdef"));
        assert_eq!(background.queue.len(), 1);
        let ops = background.take_queue();
        assert!(matches!(&ops[..], [Op::Append(text)] if text == "def"));
    }

    #[test]
    fn an_edit_between_appends_keeps_the_order() {
        let mut background = busy("abc");
        background.append("d");
        background.edit(0..1, "X");
        background.append("e");
        background.append("f");
        assert_eq!(background.logical(), Some("Xbcdef"));
        assert_eq!(background.queue.len(), 3);
    }

    #[test]
    fn a_large_edit_supersedes_what_was_queued() {
        let mut background = busy("abc");
        background.append("d");
        let before = background.revision();
        background.edit(1..1, &"x".repeat(SYNC_LIMIT + 1));
        assert_eq!(background.revision(), before + 1);
        assert!(background.queue.is_empty());
        assert!(background.is_waiting_to_start());
        assert_eq!(background.logical().map(str::len), Some(4 + SYNC_LIMIT + 1));
    }

    #[test]
    fn a_large_queue_is_folded_into_a_new_parse() {
        let mut background = busy("abc");
        background.append(&"x".repeat(SYNC_LIMIT / 2 + 1));
        background.append(&"y".repeat(SYNC_LIMIT / 2 + 1));
        // Merged, the chunks are over the limit: they are not applied one by
        // one on the main thread.
        background.wanted = false;
        assert!(background.take_queue().is_empty());
        assert!(background.is_waiting_to_start());
        assert!(background.busy());
    }

    #[test]
    fn cancelling_forgets_everything_and_is_a_revision() {
        let mut background = busy("abc");
        background.append("d");
        background.cancel();
        assert!(!background.busy());
        assert!(background.queue.is_empty());
        assert!(!background.is_waiting_to_start());
        assert_eq!(background.revision(), 2);
    }

    #[test]
    fn finishing_waits_for_the_queue() {
        let mut background = busy("abc");
        background.wanted = false;
        background.append("d");
        background.finish();
        assert!(background.busy(), "a queued append is still to be applied");
        let _ = background.take_queue();
        background.finish();
        assert!(!background.busy());
    }
}
