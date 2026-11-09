//! Rich text editing for [`bevy_md`]: a spike of decision D4.
//!
//! Bevy's `EditableText` wraps Parley's `PlainEditor`, which holds one
//! plain string with one style. Markdown renders as several styled spans, so
//! this crate keeps its **own caret** over the layout Bevy already computed
//! for the block's [`Text`]:
//!
//! - **Hit test and movement** use Parley's `Cursor` and `Selection` on the
//!   layout from [`ComputedTextBlock::buffer`], the same calls
//!   `EditableText` makes on its private layout.
//! - **Drawing** is Bevy's own: the caret is written to
//!   [`TextLayoutInfo::cursor`], which `bevy_ui_render` draws for any text
//!   entity with a [`TextCursorStyle`].
//! - **Editing** never touches the layout. The caret is a byte offset into
//!   the markdown source (decision D3), a keystroke is a
//!   [`Markdown::edit`], and the source map of `bevy_md_core` converts
//!   between rendered-text offsets and source offsets in both directions.
//!   `bevy_md` then re-renders only the block that changed.
//!
//! Only the block with the caret has the editor's components
//! ([`TextCursorStyle`], [`TextReadWriteMode::Editable`]) and a [`Pinned`]
//! mark, so every other block is plain text, and the caret's block stays
//! alive when scrolled out of the window.
//!
//! Every edit is recorded as an inverted splice ([`History`]); `Cmd/Ctrl+Z`
//! undoes and `Cmd/Ctrl+Shift+Z` redoes, grouped by time and by word.
//!
//! # Input is never dropped
//!
//! Keys, committed IME text and the paste shortcut are turned into actions
//! the moment they are read, and the actions wait in order in the editor
//! until they can be applied to a document that is what it says it is:
//!
//! - **While a large change is parsing** ([`Markdown::is_parsing`]) the
//!   document shown is behind the source, so nothing is applied: the actions
//!   queue (the shortcuts keep the meaning they had when pressed, because
//!   the modifiers are read with the event) and are applied, in order, in
//!   the first frame after the parse has been installed. The caret does not
//!   move meanwhile, and is clamped to the new source if a replacement made
//!   it shorter. Clicks are the exception: a click points at pixels of the
//!   document being replaced, so it is discarded and counted in
//!   [`EditMetrics::clicks_ignored`].
//! - **Right after an edit** the layout of the edited block is the text from
//!   before it ([`Markdown::is_rendered`]), so an action that reads the layout
//!   (a move, Backspace, Delete) waits for the next frame rather than act on
//!   the old lines. Text, paste, undo and redo do not need the layout and go
//!   on in the same frame.
//! - **History** is forgotten when something other than the editor makes a
//!   large change ([`Markdown::parse_revision`] moved), and a step that does
//!   not fit the source it is applied to is dropped instead of applied, so
//!   undo never edits bytes it did not write.
//!
//! The order between a keyboard event and an IME commit that arrive in the
//! same frame is not known (Bevy keeps no order across message types): keys
//! come first. Across frames the order is exact.
//!
//! # Clipboard
//!
//! `Cmd/Ctrl+V` (and the `Paste` key) inserts the text of Bevy's
//! [`Clipboard`] at the caret, or in place of the selection, as one step of
//! the history. `Cmd/Ctrl+C` (`Copy`) puts the selected **markdown source**
//! on the clipboard, delimiters included, and changes nothing else;
//! `Cmd/Ctrl+X` (`Cut`) does that and deletes the selection, as one step
//! whose undo gives back the text and the selection. Copy and cut do nothing
//! without a selection, and a cut that cannot write to the clipboard deletes
//! nothing ([`EditMetrics::copy_failures`]). Bevy's own `EditableText`
//! handles the clipboard for itself only; this crate reads the resource
//! directly. By default the resource is an in-process buffer; the
//! `system-clipboard` feature reads the operating system's. A paste reads the
//! clipboard when the key is pressed; copy and cut read the selection when
//! they are applied, like every queued action.
//!
//! # Selection
//!
//! The selection is the byte range between [`MarkdownEditor::anchor`] and
//! [`MarkdownEditor::caret`]. Shift with Left, Right, Home and End extends
//! it, `Cmd/Ctrl+A` selects everything, Esc drops it, and typing, IME text,
//! Backspace and Delete replace or remove just the selection, as one step of
//! the history. See the `selection` module for how it is edited and drawn;
//! only the block with the caret draws it.
//!
//! # Navigation
//!
//! Up and Down move by visual lines and keep their column, Left and Right
//! go on into the neighbouring block, Page Up and Page Down move a viewport
//! and Cmd/Ctrl+Home and End go to the ends of the document; with Shift each
//! grows the selection. See the `navigation` module's source for how a block
//! with no layout is handled.
//!
//! Put a [`MarkdownEditor`] on the entity of a [`Markdown`] and add
//! [`MarkdownEditorPlugin`]. Not yet: splitting and joining blocks, and IME
//! composition (committed text is inserted).

mod clipboard;
mod edit;
mod history;
mod navigation;
mod pointer;
mod selection;

pub use history::{Change, GROUP_WINDOW, History, MAX_STEPS, Step};
pub use pointer::{ClickSettings, PointerInput};

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bevy::clipboard::Clipboard;
use bevy::ecs::system::SystemParam;
use bevy::input::ButtonState;
use bevy::input::keyboard::{Key, KeyboardInput};
use bevy::prelude::*;
use bevy::text::{
    ComputedTextBlock, TextBrush, TextCursorStyle, TextLayoutInfo, TextReadWriteMode,
};
use bevy::time::Real;
use bevy::ui::widget::text_system;
use bevy::ui::{ComputedNode, UiGlobalTransform, UiSystems};
use bevy::window::{Ime, PrimaryWindow};
use bevy_md::{BlockText, Markdown, MarkdownPlugin, MarkdownSystems, Pinned};
use bevy_md_core::Document;
use navigation::{Navigate, Navigator};
use parley::{Affinity, Cursor, Layout, Selection};

/// Adds caret editing to every [`MarkdownEditor`].
pub struct MarkdownEditorPlugin;

impl Plugin for MarkdownEditorPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<MarkdownPlugin>() {
            app.add_plugins(MarkdownPlugin);
        }
        app.add_plugins(pointer::PointerPlugin);
        app.init_resource::<CaretStyle>()
            .init_resource::<EditMetrics>()
            .add_message::<PlaceCaret>()
            .add_systems(Update, (place_caret, handle_input).chain())
            .add_systems(PostUpdate, mark_sync_start.before(MarkdownSystems::Sync))
            .add_systems(
                PostUpdate,
                focus_caret_block
                    .after(MarkdownSystems::Sync)
                    .before(UiSystems::Prepare),
            )
            .add_systems(
                PostUpdate,
                (count_laid_out, show_caret).chain().after(text_system),
            );
    }
}

/// `at` limited to `source` and moved back to a character boundary.
fn clamp_to_boundary(source: &str, at: usize) -> usize {
    let mut at = at.min(source.len());
    while !source.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The caret of an editable [`Markdown`]. Add it to the same entity.
#[derive(Component, Debug)]
pub struct MarkdownEditor {
    /// Byte offset into the markdown source. This is the moving end (the
    /// focus) of the selection.
    caret: usize,
    /// The fixed end of the selection, a byte offset into the markdown
    /// source. Equal to `caret` when nothing is selected.
    anchor: usize,
    /// Real time of the last move or edit; the blink restarts from it.
    active_since: Duration,
    /// The text entity the cursor was last written to.
    shown_on: Option<Entity>,
    /// The block with the editor components and the pin: the caret's.
    focus: Option<Focus>,
    /// What can be undone and redone.
    history: History,
    /// Actions read but not applied yet, oldest first (see the crate docs).
    pending: VecDeque<Action>,
    /// [`Markdown::parse_revision`] as of the last time the editor looked or
    /// made a change itself: when it differs, someone else made a large
    /// change and the history no longer describes the source.
    revision: u64,
    /// Which modifier keys are down, followed from the key events in order
    /// so that a chord that goes down and up inside one frame still counts.
    modifiers: Modifiers,
    /// Where a run of Up and Down moves aims horizontally.
    preferred: Option<navigation::Preferred>,
}

/// Modifier keys that change what a key means.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Modifiers {
    super_key: bool,
    control: bool,
    shift: bool,
}

impl Modifiers {
    /// What the platform calls the command key: Cmd on macOS, Ctrl elsewhere.
    /// Either one counts.
    fn command(self) -> bool {
        self.super_key || self.control
    }

    fn from_held(held: &ButtonInput<Key>) -> Self {
        Self {
            super_key: held.pressed(Key::Super),
            control: held.pressed(Key::Control),
            shift: held.pressed(Key::Shift),
        }
    }
}

/// The entities of the block the caret is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Focus {
    /// The root of the block's subtree, which is `Pinned`.
    root: Entity,
    /// Its text, which is the only one with the editor's components.
    text: Entity,
}

impl MarkdownEditor {
    /// An editor with the caret at the start of the source.
    pub fn new() -> Self {
        Self {
            caret: 0,
            anchor: 0,
            active_since: Duration::ZERO,
            shown_on: None,
            focus: None,
            history: History::new(),
            pending: VecDeque::new(),
            revision: 0,
            modifiers: Modifiers::default(),
            preferred: None,
        }
    }

    /// How many actions (keys, IME commits, pastes) are waiting to be
    /// applied: a large change is parsing, or the layout they need is a
    /// frame away.
    pub fn pending_actions(&self) -> usize {
        self.pending.len()
    }

    /// Keep the caret on a character boundary inside the source, which a
    /// replacement made by someone else may have shortened.
    fn clamp_caret(&mut self, markdown: &Markdown) {
        let source = markdown.source();
        let valid = |at: usize| at <= source.len() && source.is_char_boundary(at);
        if !valid(self.caret) || !valid(self.anchor) {
            self.set_selection(markdown, self.anchor, self.caret);
        }
    }

    /// Forget the history when the source was changed by someone else.
    fn reconcile(&mut self, markdown: &Markdown) {
        let revision = markdown.parse_revision();
        if self.revision != revision {
            self.history.clear();
            self.revision = revision;
        }
    }

    /// The caret as a byte offset into the markdown source.
    pub fn caret(&self) -> usize {
        self.caret
    }

    /// Move the caret to a byte offset of `markdown`'s source (the text after
    /// every change, [`Markdown::source`], even one still parsing), clamped
    /// to the source and to a character boundary.
    /// Any selection is dropped: the anchor goes to the caret.
    pub fn set_caret(&mut self, markdown: &Markdown, caret: usize) {
        let caret = clamp_to_boundary(markdown.source(), caret);
        self.caret = caret;
        self.anchor = caret;
    }

    /// The fixed end of the selection, a byte offset into the source.
    pub fn anchor(&self) -> usize {
        self.anchor
    }

    /// Select from `anchor` (fixed) to `caret` (moving), both clamped to the
    /// source and to a character boundary. `anchor > caret` is a selection
    /// made backwards.
    pub fn set_selection(&mut self, markdown: &Markdown, anchor: usize, caret: usize) {
        let source = markdown.source();
        self.anchor = clamp_to_boundary(source, anchor);
        self.caret = clamp_to_boundary(source, caret);
    }

    /// The selected bytes of the source, in order, or `None` when nothing is
    /// selected.
    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        (self.anchor != self.caret)
            .then(|| self.anchor.min(self.caret)..self.anchor.max(self.caret))
    }

    /// The undo and redo stacks.
    pub fn history(&self) -> &History {
        &self.history
    }

    /// Take back the last step of the history and put the caret (and the
    /// selection, if there was one) where it was before it. Returns whether anything was undone.
    pub fn undo(&mut self, markdown: &mut Markdown) -> bool {
        match self.history.undo_selecting(markdown) {
            Some((anchor, caret)) => {
                self.set_selection(markdown, anchor, caret);
                true
            }
            None => false,
        }
    }

    /// Do again the last step that was undone. Returns whether anything was.
    pub fn redo(&mut self, markdown: &mut Markdown) -> bool {
        match self.history.redo(markdown) {
            Some(caret) => {
                self.set_caret(markdown, caret);
                true
            }
            None => false,
        }
    }
}

impl Default for MarkdownEditor {
    fn default() -> Self {
        Self::new()
    }
}

/// How the caret looks.
#[derive(Resource, Clone, Copy, Debug)]
pub struct CaretStyle {
    /// Color of the caret.
    pub color: Color,
    /// Background of the selected text.
    pub selection_color: Color,
    /// Width in logical pixels.
    pub width: f32,
    /// A full on-off cycle; the caret restarts visible on every edit.
    pub blink_period: Duration,
}

impl Default for CaretStyle {
    fn default() -> Self {
        Self {
            color: Color::srgb(0.95, 0.95, 1.0),
            selection_color: Color::srgba(0.25, 0.45, 0.85, 0.55),
            width: 1.5,
            blink_period: Duration::from_millis(1060),
        }
    }
}

/// Timing of the last edit, for budgets and the spike's report.
#[derive(Resource, Default, Debug)]
pub struct EditMetrics {
    /// From a key event being handled to the edited block being laid out
    /// and the caret placed on it, all in one frame. Excludes the OS
    /// delivering the event and the GPU drawing the frame.
    pub key_to_layout: Option<Duration>,
    /// Of that, the time `Document::edit` took (reparse of the window).
    pub document_edit: Option<Duration>,
    /// Of that, from `bevy_md` starting to respawn the changed block to
    /// the block being laid out: the part the editor design controls.
    pub sync_to_layout: Option<Duration>,
    /// How many blocks had their text laid out again in that frame.
    pub blocks_laid_out: usize,
    /// How many blocks the document has.
    pub blocks: usize,
    /// Edits so far.
    pub edits: u64,
    /// Clicks discarded because a large change was parsing: they pointed at
    /// the document being replaced.
    pub clicks_ignored: u64,
    /// Paste shortcuts that found nothing to paste because the clipboard
    /// could not be read as text (not counting an empty clipboard).
    pub paste_failures: u64,
    /// Copy and cut shortcuts that could not write to the clipboard (a cut
    /// that fails deletes nothing).
    pub copy_failures: u64,
    /// The slowest `key_to_layout` so far.
    pub worst: Duration,
    started: Option<Instant>,
    sync_started: Option<Instant>,
    laid_out_this_frame: usize,
}

/// Put the caret where a single click landed, with no click counting, drag
/// or Shift (those are [`PointerInput`], which the mouse sends). `position`
/// is in physical window pixels; a test or an app can send it.
#[derive(Message, Clone, Copy, Debug)]
pub struct PlaceCaret {
    /// Physical pixels from the window's top-left corner.
    pub position: Vec2,
}

/// What a key asks for.
#[derive(Clone, Debug)]
enum Action {
    Move(Motion),
    /// A motion with Shift: the caret moves, the anchor stays.
    Extend(Motion),
    /// Cmd/Ctrl+A.
    SelectAll,
    /// Esc: drop the selection, keep the caret.
    Collapse,
    /// A move by lines or pages, or to an end of the document; `true` when
    /// it extends the selection (Shift was down).
    Navigate(Navigate, bool),
    Backspace,
    Delete,
    Insert(String),
    /// Text from the clipboard: an [`Action::Insert`] that is a step of its
    /// own in the history.
    Paste(String),
    /// A paste that reads the clipboard when it is applied, because a copy or
    /// a cut is ahead of it in the queue.
    PasteClipboard,
    /// The selected source to the clipboard.
    Copy,
    /// Copy, then delete the selection, as one step of the history.
    Cut,
    Undo,
    Redo,
}

impl Action {
    /// Whether the action reads the glyph layout of the caret's block.
    fn needs_layout(&self) -> bool {
        matches!(
            self,
            Action::Move(_)
                | Action::Extend(_)
                | Action::Navigate(..)
                | Action::Backspace
                | Action::Delete
        )
    }

    /// Whether the action can change the source, and counts as an edit in
    /// [`EditMetrics`] when it does.
    fn edits_source(&self) -> bool {
        matches!(
            self,
            Action::Insert(_)
                | Action::Paste(_)
                | Action::PasteClipboard
                | Action::Cut
                | Action::Backspace
                | Action::Delete
                | Action::Undo
                | Action::Redo
        )
    }
}

#[derive(Clone, Copy, Debug)]
enum Motion {
    Left,
    Right,
    Home,
    End,
}

/// Finds the text entity of a block, and its layout.
#[derive(SystemParam)]
struct BlockTexts<'w, 's> {
    children: Query<'w, 's, &'static Children>,
    texts: Query<'w, 's, &'static ComputedTextBlock, With<BlockText>>,
}

impl BlockTexts<'_, '_> {
    /// The text entity of block `block`.
    fn entity(&self, markdown: &Markdown, block: usize) -> Option<Entity> {
        let root = markdown.block_entity(block)?;
        // Quote bars wrap the block's node, so the text can be a few levels down.
        self.children
            .iter_descendants(root)
            .find(|descendant| self.texts.contains(*descendant))
    }

    /// The layout Bevy computed for block `block`.
    fn layout(&self, markdown: &Markdown, block: usize) -> Option<&Layout<TextBrush>> {
        let text = self.entity(markdown, block)?;
        self.texts.get(text).ok().map(ComputedTextBlock::buffer)
    }
}

fn place_caret(
    mut clicks: MessageReader<PlaceCaret>,
    ui_scale: Res<UiScale>,
    time: Res<Time<Real>>,
    mut metrics: ResMut<EditMetrics>,
    editor: Single<(&mut MarkdownEditor, &Markdown)>,
    blocks: BlockTexts,
    texts: Query<(&ComputedTextBlock, &ComputedNode, &UiGlobalTransform), With<BlockText>>,
) {
    let (mut editor, markdown) = editor.into_inner();
    for click in clicks.read() {
        if markdown.is_parsing() {
            // The pixels belong to the document being replaced, and the
            // caret is a byte of the source that is replacing it.
            metrics.clicks_ignored += 1;
            continue;
        }
        for (block_ix, _) in markdown.live_blocks() {
            let Some(entity) = blocks.entity(markdown, block_ix) else {
                continue;
            };
            let Ok((computed, node, transform)) = texts.get(entity) else {
                continue;
            };
            let Some(inverse) = transform.try_inverse() else {
                continue;
            };
            // The same mapping `bevy_ui_widgets` uses for `TextInput`:
            // window pixels to the node's own space, centered on the node.
            let local = inverse.transform_point2(click.position / ui_scale.0);
            let content = node.content_box();
            if !content.contains(local) {
                continue;
            }
            let point = local - content.min;
            let cursor = Cursor::from_point(computed.buffer(), point.x, point.y);
            let Some(block) = markdown.document().block(block_ix) else {
                continue;
            };
            if let Some(caret) = edit::source_caret(&block, cursor.index()) {
                editor.set_caret(markdown, caret);
                editor.active_since = time.elapsed();
            }
            break;
        }
    }
}

/// The action a key press asks for, with the modifiers it was pressed with.
/// `None` for a key that asks for nothing.
fn key_action(
    event: &KeyboardInput,
    modifiers: Modifiers,
    clipboard: Option<&mut Clipboard>,
    metrics: &mut EditMetrics,
    // A copy or a cut is waiting ahead: a paste reads the clipboard late.
    clipboard_ahead: bool,
) -> Option<Action> {
    let (command, shift) = (modifiers.command(), modifiers.shift);
    match &event.logical_key {
        Key::Character(c) if command && c.eq_ignore_ascii_case("z") => {
            Some(if shift { Action::Redo } else { Action::Undo })
        }
        Key::Character(c) if command && c.eq_ignore_ascii_case("c") => Some(Action::Copy),
        Key::Character(c) if command && c.eq_ignore_ascii_case("x") => Some(Action::Cut),
        Key::Character(c) if command && c.eq_ignore_ascii_case("v") => {
            clipboard::paste_shortcut(clipboard, metrics, clipboard_ahead)
        }
        Key::Copy => Some(Action::Copy),
        Key::Cut => Some(Action::Cut),
        Key::Paste => clipboard::paste_shortcut(clipboard, metrics, clipboard_ahead),
        Key::Character(c) if command && c.eq_ignore_ascii_case("a") => Some(Action::SelectAll),
        Key::Escape => Some(Action::Collapse),
        Key::ArrowUp | Key::Home if command => {
            Some(Action::Navigate(Navigate::DocumentStart, shift))
        }
        Key::ArrowDown | Key::End if command => {
            Some(Action::Navigate(Navigate::DocumentEnd, shift))
        }
        Key::ArrowUp => Some(Action::Navigate(Navigate::Up, shift)),
        Key::ArrowDown => Some(Action::Navigate(Navigate::Down, shift)),
        Key::PageUp => Some(Action::Navigate(Navigate::PageUp, shift)),
        Key::PageDown => Some(Action::Navigate(Navigate::PageDown, shift)),
        Key::ArrowLeft => Some(motion_action(Motion::Left, shift)),
        Key::ArrowRight => Some(motion_action(Motion::Right, shift)),
        Key::Home => Some(motion_action(Motion::Home, shift)),
        Key::End => Some(motion_action(Motion::End, shift)),
        Key::Backspace => Some(Action::Backspace),
        Key::Delete => Some(Action::Delete),
        _ if command => None,
        _ => event
            .text
            .as_ref()
            .filter(|text| !text.is_empty() && !text.chars().any(char::is_control))
            .map(|text| Action::Insert(text.to_string())),
    }
}

/// A motion key: it extends the selection with Shift, else it moves.
fn motion_action(motion: Motion, shift: bool) -> Action {
    if shift {
        Action::Extend(motion)
    } else {
        Action::Move(motion)
    }
}

/// The text on the clipboard, read now (what the user copied when they
/// pressed the shortcut, not what is there when the action is applied).
fn paste_action(clipboard: Option<&mut Clipboard>, metrics: &mut EditMetrics) -> Option<Action> {
    let read = clipboard.map(|clipboard| clipboard.fetch_text().poll_result());
    if let Some(Some(Ok(text))) = read {
        let text = clean_paste(&text);
        (!text.is_empty()).then_some(Action::Paste(text))
    } else {
        // No clipboard, no text on it, or a read still in flight (the web,
        // where reads are asynchronous and not supported here).
        metrics.paste_failures += 1;
        None
    }
}

/// Pasted text with line breaks as `\n` and no control characters but tabs.
fn clean_paste(text: &str) -> String {
    let mut clean = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                clean.push('\n');
            }
            '\n' | '\t' => clean.push(c),
            c if c.is_control() => {}
            c => clean.push(c),
        }
    }
    clean
}

// A Bevy system: each parameter is a distinct piece of the world it reads.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn handle_input(
    mut keys: MessageReader<KeyboardInput>,
    mut commits: MessageReader<Ime>,
    held: Res<ButtonInput<Key>>,
    time: Res<Time<Real>>,
    mut metrics: ResMut<EditMetrics>,
    mut clipboard: Option<ResMut<Clipboard>>,
    editor: Single<(&mut MarkdownEditor, &mut Markdown)>,
    blocks: BlockTexts,
    navigator: Navigator,
) {
    let (mut editor, mut markdown) = editor.into_inner();

    // 1. Read everything there is, in order, whatever state the document is
    //    in: a message not read now is gone for good in two frames.
    //    The modifiers are followed through the events themselves: the state
    //    of `ButtonInput` is the one at the end of the frame, after a chord
    //    that went down and up inside it.
    let mut modifiers = editor.modifiers;
    let mut modifier_events = false;
    let mut read = Vec::new();
    for event in keys.read() {
        let down = event.state == ButtonState::Pressed;
        match event.logical_key {
            Key::Super => modifiers.super_key = down,
            Key::Control => modifiers.control = down,
            Key::Shift => modifiers.shift = down,
            _ => {
                if down {
                    let clipboard_ahead =
                        (editor.pending.iter().chain(read.iter())).any(Action::writes_clipboard);
                    read.extend(key_action(
                        event,
                        modifiers,
                        clipboard.as_deref_mut(),
                        &mut metrics,
                        clipboard_ahead,
                    ));
                }
                continue;
            }
        }
        modifier_events = true;
    }
    if !modifier_events {
        // Nothing moved a modifier in this frame: what is held is the truth
        // (and corrects a key released while the window had no focus).
        modifiers = Modifiers::from_held(&held);
    }
    if editor.modifiers != modifiers {
        editor.modifiers = modifiers;
    }
    for ime in commits.read() {
        if let Ime::Commit { value, .. } = ime
            && !value.is_empty()
        {
            read.push(Action::Insert(value.clone()));
        }
    }
    if !read.is_empty() {
        editor.pending.extend(read);
    }
    if editor.pending.is_empty() {
        return;
    }

    // 2. Something else made a large change: the history is about another
    //    source.
    if editor.revision != markdown.parse_revision() {
        editor.reconcile(&markdown);
    }

    // 3. Apply what can be applied to the document as it is now. A large
    //    change still parsing means the document shown is behind the source:
    //    everything waits for the frame after it lands.
    let now = time.elapsed();
    let mut applied = false;
    while let Some(next) = editor.pending.front() {
        if markdown.is_parsing() {
            break;
        }
        // The layout of a block just edited is the text from before the
        // edit; the next frame has the new one.
        if next.needs_layout() && !markdown.is_rendered() {
            break;
        }
        let needs_layout = next.needs_layout();
        let Some(action) = editor.pending.pop_front() else {
            break;
        };
        if !applied {
            applied = true;
            metrics.started = Some(Instant::now());
            metrics.document_edit = None;
            metrics.sync_started = None;
        }
        editor.clamp_caret(&markdown);
        let layout = needs_layout
            .then(|| {
                markdown
                    .document()
                    .block_at(editor.caret)
                    .and_then(|block| blocks.layout(&markdown, block))
            })
            .flatten();
        let edited = action.edits_source();
        let before = Instant::now();
        let changed = if let Action::Navigate(navigate, extend) = action {
            navigator.apply(&mut editor, &markdown, navigate, extend)
        } else {
            editor.preferred = None; // Anything else ends a run of line moves.
            clipboard::dispatch(
                &mut editor,
                &mut markdown,
                layout,
                &action,
                clipboard.as_deref_mut(),
                &mut metrics,
                now,
            )
        };
        if changed && edited {
            metrics.document_edit = Some(before.elapsed());
            metrics.edits += 1;
        }
        // A large paste or redo starts a parse: that is the editor's own
        // change, and the history it recorded stands.
        editor.revision = markdown.parse_revision();
    }
    if applied {
        editor.active_since = time.elapsed();
    }
}

/// Apply one action to the source. Returns whether it did anything.
fn apply(
    editor: &mut MarkdownEditor,
    markdown: &mut Markdown,
    layout: Option<&Layout<TextBrush>>,
    action: &Action,
    now: Duration,
) -> bool {
    let (block, at) = edit::caret_text(markdown.document(), editor.caret).unzip();
    match action {
        Action::Insert(text) => {
            // Typing over a selection replaces it.
            if let Some(changed) = selection::replace(editor, markdown, text, now) {
                return changed;
            }
            let change = edit::insert(markdown, &mut editor.caret, text);
            editor.anchor = editor.caret;
            editor.history.record(change, now);
            true
        }
        Action::Paste(text) => {
            clipboard::paste(editor, markdown, text, now);
            true
        }
        // Applied by `clipboard::apply`, which has the clipboard, and by
        // `navigation::Navigator`, which needs more than one block's layout.
        Action::PasteClipboard | Action::Copy | Action::Cut | Action::Navigate(..) => false,
        Action::Undo => editor.undo(markdown),
        Action::Redo => editor.redo(markdown),
        Action::SelectAll => selection::select_all(editor, markdown),
        Action::Collapse => selection::collapse(editor),
        Action::Extend(motion) => selection::extend(editor, markdown, layout, *motion),
        Action::Move(motion) => {
            // A selection collapses to its edge first (Left, Right), or
            // the motion starts from the moving end (Home, End).
            if selection::collapse_to_edge(editor, markdown, *motion) {
                return true;
            }
            let had_selection = selection::collapse(editor);
            let (Some(block), Some(at)) = (block, at) else {
                return had_selection;
            };
            let to = motion_to(markdown.document(), layout, block, at, *motion);
            let caret = markdown
                .document()
                .block(block)
                .and_then(|block| edit::source_caret(&block, to));
            // At the edge of a block, Left and Right go on into the next one.
            let caret = if to == at {
                navigation::across_blocks(markdown.document(), block, *motion)
            } else {
                caret
            };
            let Some(caret) = caret else {
                return had_selection;
            };
            editor.set_caret(markdown, caret);
            true
        }
        Action::Backspace | Action::Delete => {
            // With a selection, only the selection goes.
            if let Some(changed) = selection::replace(editor, markdown, "", now) {
                return changed;
            }
            let (Some(block), Some(at)) = (block, at) else {
                return false;
            };
            let motion = if matches!(action, Action::Backspace) {
                Motion::Left
            } else {
                Motion::Right
            };
            let to = motion_to(markdown.document(), layout, block, at, motion);
            if to == at {
                return false;
            }
            let range = at.min(to)..at.max(to);
            match edit::delete_text(markdown, &mut editor.caret, block, range) {
                Some(change) => {
                    editor.anchor = editor.caret;
                    editor.history.record(change, now);
                    true
                }
                None => false,
            }
        }
    }
}

/// Where `motion` takes the caret of block `block`, as a rendered-text
/// offset: by the block's layout when it has one. A block without one (it is
/// scrolled out of the window and has no node) cannot say where its lines
/// break or what a character cluster is, so Left, Right, Backspace and Delete
/// step one `char` and Home and End stay: the text is still edited, never
/// dropped.
fn motion_to(
    document: &Document,
    layout: Option<&Layout<TextBrush>>,
    block: usize,
    at: usize,
    motion: Motion,
) -> usize {
    if let Some(layout) = layout {
        return move_text(layout, at, motion);
    }
    let Some(block) = document.block(block) else {
        return at;
    };
    let text = block.content.text.as_str();
    match motion {
        Motion::Left => text
            .get(..at)
            .and_then(|before| before.char_indices().next_back())
            .map_or(at, |(start, _)| start),
        Motion::Right => text
            .get(at..)
            .and_then(|after| after.chars().next())
            .map_or(at, |c| at + c.len_utf8()),
        Motion::Home | Motion::End => at,
    }
}

/// Where a motion takes the caret, as a rendered-text offset.
fn move_text(layout: &Layout<TextBrush>, at: usize, motion: Motion) -> usize {
    let selection = Selection::from_byte_index(layout, at, Affinity::Downstream);
    let moved = match motion {
        Motion::Left => selection.previous_visual(layout, false),
        Motion::Right => selection.next_visual(layout, false),
        Motion::Home => selection.line_start(layout, false),
        Motion::End => selection.line_end(layout, false),
    };
    moved.focus().index()
}

/// Notes when `bevy_md` is about to respawn the edited block.
fn mark_sync_start(mut metrics: ResMut<EditMetrics>) {
    if metrics.started.is_some() {
        metrics.sync_started = Some(Instant::now());
    }
}

/// Gives the block with the caret, and only it, the look of an editable
/// text and a [`Pinned`] mark, and takes both off the block the caret left.
/// Every other block is plain text with no editor state, so a document costs
/// the same with or without an editor on it, and the caret's block stays
/// alive when scrolled out of the window.
fn focus_caret_block(
    mut commands: Commands,
    style: Res<CaretStyle>,
    mut editor: Single<(&mut MarkdownEditor, &Markdown)>,
    blocks: BlockTexts,
) {
    let (editor, markdown) = &mut *editor;
    if markdown.is_parsing() {
        // The caret is a byte of the source that is parsing; the blocks
        // shown are the old document's. Focus follows when it lands.
        return;
    }
    let now = edit::caret_text(markdown.document(), editor.caret).and_then(|(block, _)| {
        Some(Focus {
            root: markdown.block_entity(block)?,
            text: blocks.entity(markdown, block)?,
        })
    });
    if editor.focus == now {
        return;
    }
    if let Some(old) = editor.focus.take() {
        if let Ok(mut text) = commands.get_entity(old.text) {
            text.remove::<(TextCursorStyle, TextReadWriteMode)>();
        }
        if let Ok(mut root) = commands.get_entity(old.root) {
            root.remove::<Pinned>();
        }
    }
    if let Some(now) = now {
        commands.entity(now.text).insert((
            TextCursorStyle {
                color: style.color,
                // The caret's block is the only one that shows a selection,
                // and it is always the focused one.
                selection_color: style.selection_color,
                unfocused_selection_color: style.selection_color,
                ..default()
            },
            TextReadWriteMode::Editable,
        ));
        commands.entity(now.root).insert(Pinned);
    }
    editor.focus = now;
}

/// How many blocks Bevy laid out this frame: a block that is not laid out
/// again keeps its [`ComputedTextBlock`] untouched.
fn count_laid_out(
    mut metrics: ResMut<EditMetrics>,
    laid_out: Query<Ref<ComputedTextBlock>, With<BlockText>>,
) {
    metrics.laid_out_this_frame = laid_out.iter().filter(Ref::is_changed).count();
}

/// Draws the caret and reports how long the edit took. Runs after Bevy has
/// laid the text out, so the caret is on the layout of this very frame.
fn show_caret(
    time: Res<Time<Real>>,
    style: Res<CaretStyle>,
    mut metrics: ResMut<EditMetrics>,
    mut window: Single<&mut Window, With<PrimaryWindow>>,
    mut editor: Single<(&mut MarkdownEditor, &Markdown)>,
    blocks: BlockTexts,
    mut texts: Query<
        (
            &ComputedTextBlock,
            &ComputedNode,
            &UiGlobalTransform,
            &mut TextLayoutInfo,
        ),
        With<BlockText>,
    >,
) {
    let (editor, markdown) = &mut *editor;
    if markdown.is_parsing() {
        // Same as `focus_caret_block`: nothing to place the caret on until
        // the parse is installed. An edit that started it is not timed.
        metrics.started = None;
        metrics.sync_started = None;
        return;
    }
    let placed = edit::caret_text(markdown.document(), editor.caret)
        .and_then(|(block, at)| Some((blocks.entity(markdown, block)?, at)));

    // Take the cursor off the block it left.
    if editor.shown_on != placed.map(|(entity, _)| entity)
        && let Some(old) = editor.shown_on.take()
        && let Ok((.., mut info)) = texts.get_mut(old)
        && (info.cursor.is_some() || !info.selection_rects.is_empty())
    {
        info.cursor = None;
        info.selection_rects.clear();
    }
    if let Some((entity, at)) = placed
        && let Ok((computed, node, transform, mut info)) = texts.get_mut(entity)
    {
        let layout = computed.buffer();
        let width = style.width * layout.scale();
        let bounds =
            Cursor::from_byte_index(layout, at, Affinity::Downstream).geometry(layout, width);
        let rect = Rect::new(
            bounds.x0 as f32,
            bounds.y0 as f32,
            bounds.x1 as f32,
            bounds.y1 as f32,
        );
        let period = style.blink_period.as_secs_f32().max(f32::EPSILON);
        let phase = time
            .elapsed()
            .saturating_sub(editor.active_since)
            .as_secs_f32()
            % period;
        let cursor = Some((phase < period / 2.0, rect));
        if info.cursor != cursor {
            info.cursor = cursor;
        }
        editor.shown_on = Some(entity);

        // The selected part of this block (the only one that shows it).
        let rects = editor
            .selection()
            .zip(markdown.document().block_at(editor.caret))
            .map(|(selection, block)| {
                selection::rects(markdown.document(), &selection, block, layout)
            })
            .unwrap_or_default();
        if info.selection_rects != rects {
            info.selection_rects = rects;
        }

        // Where the OS should put the IME's candidate window: under the
        // caret, in logical window pixels.
        let below =
            transform.transform_point2(node.content_box().min + Vec2::new(rect.min.x, rect.max.y));
        let ime_position = below * node.inverse_scale_factor();
        if !window.ime_enabled {
            window.ime_enabled = true;
        }
        if window.ime_position != ime_position {
            window.ime_position = ime_position;
        }
    }

    metrics.blocks = markdown.document().len();
    if let Some(started) = metrics.started.take() {
        let elapsed = started.elapsed();
        metrics.key_to_layout = Some(elapsed);
        metrics.sync_to_layout = metrics.sync_started.take().map(|start| start.elapsed());
        metrics.worst = metrics.worst.max(elapsed);
        metrics.blocks_laid_out = metrics.laid_out_this_frame;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pasted_text_gets_unix_line_breaks_and_loses_control_characters() {
        assert_eq!(clean_paste("a\r\nb\rc\nd"), "a\nb\nc\nd");
        assert_eq!(clean_paste("tab\there"), "tab\there");
        assert_eq!(
            clean_paste("bell\x07 and nul\0 gone\x1b"),
            "bell and nul gone"
        );
        assert_eq!(clean_paste("日本 ç 😀"), "日本 ç 😀");
        assert_eq!(clean_paste("\r\n\r\n"), "\n\n");
        assert_eq!(clean_paste(""), "");
    }

    #[test]
    fn a_clipboard_that_cannot_be_read_is_counted_and_gives_no_action() {
        let mut metrics = EditMetrics::default();
        assert!(paste_action(None, &mut metrics).is_none());
        assert_eq!(metrics.paste_failures, 1);

        // An empty one is readable: nothing to paste, nothing failed.
        let mut clipboard = Clipboard::default();
        clipboard.set_text("").unwrap();
        assert!(paste_action(Some(&mut clipboard), &mut metrics).is_none());
        assert_eq!(metrics.paste_failures, 1);

        clipboard.set_text("a\r\nb").unwrap();
        let action = paste_action(Some(&mut clipboard), &mut metrics);
        assert!(matches!(action, Some(Action::Paste(text)) if text == "a\nb"));
    }

    #[test]
    fn without_a_layout_motions_step_one_character_and_never_split_one() {
        let markdown = Markdown::new("aç日😀z");
        let document = markdown.document();
        let text = &document.block(0).unwrap().content.text;
        assert_eq!(text, "aç日😀z");
        let step = |at, motion| motion_to(document, None, 0, at, motion);
        // Right: a|ç|日|😀|z|, by the bytes each takes.
        let mut at = 0;
        let mut stops = vec![at];
        for _ in 0..5 {
            at = step(at, Motion::Right);
            stops.push(at);
        }
        assert_eq!(stops, [0, 1, 3, 6, 10, 11]);
        assert_eq!(step(11, Motion::Right), 11, "stays at the end");
        // Left walks the same stops back.
        for stop in stops.iter().rev().skip(1) {
            at = step(at, Motion::Left);
            assert_eq!(at, *stop);
        }
        assert_eq!(step(0, Motion::Left), 0, "stays at the start");
        // Lines are unknown without a layout.
        assert_eq!(step(6, Motion::Home), 6);
        assert_eq!(step(6, Motion::End), 6);
    }
}
