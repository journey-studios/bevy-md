//! One block to one UI subtree. A block of prose is a single text entity
//! with one span per styled run, so Bevy lays it out as one paragraph.

use bevy::ecs::system::EntityCommands;
use bevy::prelude::*;
use bevy::text::{FontSize, FontSource, FontWeight, Strikethrough, TextBackgroundColor, Underline};
use bevy_md_core::{Align, Block, BlockKind, Inline, ListMarker, Style};

use crate::BlockText;
use crate::theme::MarkdownTheme;

/// Height of a task checkbox, in logical pixels.
const CHECKBOX: f32 = 14.0;

/// A text entity of a block and its spans, in the order [`rebind`] visits
/// them. Plain text (code, a list number) has no spans.
#[derive(Clone, Debug)]
pub(crate) struct Slot {
    pub text: Entity,
    pub spans: Vec<Entity>,
}

/// What distinguishes the subtrees two blocks can share: every component
/// but the strings is the same for blocks with the same signature.
pub(crate) type Signature = Box<[u32]>;

/// The subtree of one block.
#[derive(Clone, Debug)]
pub(crate) struct Built {
    pub root: Entity,
    pub slots: Vec<Slot>,
    pub signature: Signature,
    /// The block it showed when it was released, if the document has not
    /// changed there since: a hint to give it back to the same block.
    pub last: Option<usize>,
}

pub(crate) fn spawn_block(commands: &mut Commands, block: &Block, theme: &MarkdownTheme) -> Built {
    let mut slots = Vec::new();
    let content = spawn_content(commands, block, theme, &mut slots);
    // One bar per quote level, each wrapping the one inside it.
    let root = (0..block.quote_depth).fold(content, |inner, _| {
        commands
            .spawn((
                Node {
                    flex_direction: FlexDirection::Column,
                    flex_shrink: 0.0,
                    border: UiRect::left(Val::Px(3.0)),
                    padding: UiRect::left(Val::Px(theme.indent * 0.6)),
                    ..default()
                },
                BorderColor::all(theme.border),
            ))
            .add_child(inner)
            .id()
    });
    Built {
        root,
        slots,
        signature: signature(block),
        last: None,
    }
}

/// Everything about how `block` is built except its strings.
pub(crate) fn signature(block: &Block) -> Signature {
    fn inline(sig: &mut Vec<u32>, inline: &Inline) {
        sig.push(u32::try_from(inline.runs.len()).unwrap_or(u32::MAX));
        for run in &inline.runs {
            sig.push(u32::from(run.style.bits()) | (u32::from(run.link.is_some()) << 8));
        }
    }
    let mut sig = vec![u32::from(block.quote_depth), u32::from(block.list_depth)];
    match &block.kind {
        BlockKind::Paragraph => {
            sig.push(0);
            inline(&mut sig, &block.content);
        }
        BlockKind::Heading(level) => {
            sig.extend([1, u32::from(*level)]);
            inline(&mut sig, &block.content);
        }
        BlockKind::ListItem { marker, task } => {
            let class = match (task, marker) {
                (Some(false), _) => 1,
                (Some(true), _) => 2,
                (None, ListMarker::Bullet) => 3,
                (None, ListMarker::Ordered(_)) => 4,
            };
            sig.extend([2, class]);
            inline(&mut sig, &block.content);
        }
        BlockKind::CodeBlock { .. } => sig.push(3),
        BlockKind::Html => sig.push(4),
        BlockKind::Frontmatter => sig.push(5),
        BlockKind::Rule => sig.push(6),
        BlockKind::Table { align, rows } => {
            sig.extend([7, u32::try_from(rows.len()).unwrap_or(u32::MAX)]);
            sig.push(u32::try_from(align.len()).unwrap_or(u32::MAX));
            sig.extend(align.iter().map(|a| match a {
                Align::Center => 1,
                Align::Right => 2,
                _ => 0,
            }));
            for (row_ix, row) in rows.iter().enumerate() {
                sig.extend([
                    u32::from(row_ix == 0),
                    u32::try_from(row.len()).unwrap_or(u32::MAX),
                ]);
                for cell in row {
                    inline(&mut sig, cell);
                }
            }
        }
    }
    sig.into()
}

/// Write the strings of `block` into the subtree `built`, which was built
/// for a block with the same signature. Only strings that differ are
/// written, so Bevy lays out only the text that changed.
pub(crate) fn rebind(
    block: &Block,
    built: &Built,
    texts: &mut Query<&mut Text>,
    spans: &mut Query<&mut TextSpan>,
) {
    let mut slots = built.slots.iter();
    // Compare through `&` first: `&mut` on the component would mark it
    // changed, and Bevy would lay the text out again.
    let mut plain = |slot: Option<&Slot>, new: &str| {
        if let Some(mut text) = slot.and_then(|slot| texts.get_mut(slot.text).ok())
            && text.0 != new
        {
            text.0.clear();
            text.0.push_str(new);
        }
    };
    let mut inline_slot = |slot: Option<&Slot>, inline: &Inline| {
        let Some(slot) = slot else { return };
        for (entity, run) in slot.spans.iter().zip(&inline.runs) {
            if let Ok(mut span) = spans.get_mut(*entity) {
                let new = inline.run_text(run);
                if span.0 != new {
                    span.0.clear();
                    span.0.push_str(new);
                }
            }
        }
    };
    match &block.kind {
        BlockKind::Rule => {}
        BlockKind::CodeBlock { .. } | BlockKind::Html | BlockKind::Frontmatter => {
            plain(slots.next(), block.content.text.trim_end_matches('\n'));
        }
        BlockKind::Table { rows, .. } => {
            for cell in rows.iter().flatten() {
                inline_slot(slots.next(), cell);
            }
        }
        BlockKind::Heading(_) | BlockKind::Paragraph => inline_slot(slots.next(), &block.content),
        BlockKind::ListItem { marker, task } => {
            if let (None, ListMarker::Ordered(n)) = (task, marker) {
                plain(slots.next(), &format!("{n}."));
            } else if task.is_none() {
                slots.next(); // The bullet never changes.
            }
            inline_slot(slots.next(), &block.content);
        }
    }
}

fn spawn_content(
    commands: &mut Commands,
    block: &Block,
    theme: &MarkdownTheme,
    slots: &mut Vec<Slot>,
) -> Entity {
    // A list item hangs its marker in the indent, so its text sits one
    // level in; everything else in a list (a second paragraph, a code
    // block) lines up with that text.
    let list_levels = match block.kind {
        BlockKind::ListItem { .. } => block.list_depth.saturating_sub(1),
        _ => block.list_depth,
    };
    let indent = f32::from(list_levels) * theme.indent;
    let mut node = Node {
        // The column of blocks is as tall as its content, but inside a
        // fixed-height scroll area the layout would shrink empty blocks,
        // such as a rule, to nothing.
        flex_shrink: 0.0,
        margin: UiRect::bottom(Val::Px(theme.block_gap)),
        padding: UiRect::left(Val::Px(indent)),
        ..default()
    };
    match &block.kind {
        BlockKind::Rule => {
            node.height = Val::Px(1.0);
            node.margin = UiRect::vertical(Val::Px(theme.block_gap));
            commands.spawn((node, BackgroundColor(theme.border))).id()
        }
        BlockKind::CodeBlock { .. } | BlockKind::Html | BlockKind::Frontmatter => {
            node.padding = UiRect::all(Val::Px(10.0));
            node.margin.left = Val::Px(indent);
            node.border_radius = BorderRadius::all(Val::Px(6.0));
            let text = block.content.text.trim_end_matches('\n');
            let font = font(theme.mono_font.clone(), theme.code_size);
            let code = commands
                .spawn((node, BackgroundColor(theme.code_background)))
                .id();
            let text = commands
                .spawn((Text::new(text), font, TextColor(theme.text), ChildOf(code)))
                .id();
            slots.push(Slot {
                text,
                spans: Vec::new(),
            });
            code
        }
        BlockKind::Table { align, rows } => {
            node.display = Display::Grid;
            node.grid_template_columns = RepeatedGridTrack::auto(align.len().max(1) as u16);
            node.border = UiRect::all(Val::Px(1.0));
            let table = commands.spawn((node, BorderColor::all(theme.border))).id();
            for (row_ix, row) in rows.iter().enumerate() {
                for (col, cell) in row.iter().enumerate() {
                    let justify = match align.get(col) {
                        Some(Align::Center) => JustifyContent::Center,
                        Some(Align::Right) => JustifyContent::End,
                        _ => JustifyContent::Start,
                    };
                    let cell_node = Node {
                        padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                        border: UiRect::all(Val::Px(0.5)),
                        justify_content: justify,
                        ..default()
                    };
                    let size = theme.body_size;
                    let text = spawn_inline(commands, cell, theme, size, row_ix == 0, slots);
                    commands
                        .spawn((cell_node, BorderColor::all(theme.border)))
                        .add_child(text)
                        .insert(ChildOf(table));
                }
            }
            table
        }
        BlockKind::Heading(level) => {
            let size = theme.heading_sizes[usize::from(level.clamp(&1, &6) - 1)];
            node.margin.top = Val::Px(theme.block_gap);
            let text = spawn_block_text(commands, &block.content, theme, size, true, slots);
            commands.spawn(node).add_child(text).id()
        }
        BlockKind::ListItem { marker, task } => {
            // Items of one list sit closer than separate paragraphs.
            node.margin.bottom = Val::Px(theme.list_gap);
            let marker = spawn_marker(commands, *marker, *task, theme, slots);
            let text = spawn_block_text(
                commands,
                &block.content,
                theme,
                theme.body_size,
                false,
                slots,
            );
            // Fill what the marker leaves and wrap there.
            commands.entity(text).insert(Node {
                flex_grow: 1.0,
                flex_basis: Val::Px(0.0),
                min_width: Val::Px(0.0),
                ..default()
            });
            commands.spawn(node).add_children(&[marker, text]).id()
        }
        BlockKind::Paragraph => {
            let text = spawn_block_text(
                commands,
                &block.content,
                theme,
                theme.body_size,
                false,
                slots,
            );
            commands.spawn(node).add_child(text).id()
        }
    }
}

/// The bullet, number or checkbox in the margin of a list item, in a
/// column as wide as one indent so the text of every item lines up.
fn spawn_marker(
    commands: &mut Commands,
    marker: ListMarker,
    task: Option<bool>,
    theme: &MarkdownTheme,
    slots: &mut Vec<Slot>,
) -> Entity {
    let column = Node {
        width: Val::Px(theme.indent),
        flex_shrink: 0.0,
        justify_content: JustifyContent::End,
        padding: UiRect::right(Val::Px(6.0)),
        ..default()
    };
    let column = commands.spawn(column).id();
    let label = match (task, marker) {
        (Some(checked), _) => {
            spawn_checkbox(commands, checked, theme).insert(ChildOf(column));
            return column;
        }
        (None, ListMarker::Bullet) => "\u{2022}".to_owned(),
        (None, ListMarker::Ordered(n)) => format!("{n}."),
    };
    let text = commands
        .spawn((
            Text::new(label),
            font(theme.body_font.clone(), theme.body_size),
            TextColor(theme.muted),
            ChildOf(column),
        ))
        .id();
    slots.push(Slot {
        text,
        spans: Vec::new(),
    });
    column
}

/// A checkbox drawn from nodes: font glyphs for ballot boxes are missing
/// from most system fonts, or come out as color emoji.
fn spawn_checkbox<'a>(
    commands: &'a mut Commands,
    checked: bool,
    theme: &MarkdownTheme,
) -> EntityCommands<'a> {
    let (border, fill) = if checked {
        (theme.link, theme.link)
    } else {
        (theme.muted, Color::NONE)
    };
    let mut checkbox = commands.spawn((
        Node {
            width: Val::Px(CHECKBOX),
            height: Val::Px(CHECKBOX),
            // Centered on the first line of text.
            margin: UiRect::top(Val::Px((theme.body_size * 1.2 - CHECKBOX) / 2.0)),
            border: UiRect::all(Val::Px(1.5)),
            border_radius: BorderRadius::all(Val::Px(3.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BorderColor::all(border),
        BackgroundColor(fill),
    ));
    if checked {
        // The tick: the bottom and right edges of a box, turned.
        checkbox.with_child((
            Node {
                width: Val::Px(4.0),
                height: Val::Px(8.0),
                margin: UiRect::bottom(Val::Px(2.0)),
                border: UiRect {
                    right: Val::Px(2.0),
                    bottom: Val::Px(2.0),
                    ..default()
                },
                ..default()
            },
            BorderColor::all(Color::WHITE),
            UiTransform::from_rotation(Rot2::degrees(45.0)),
        ));
    }
    checkbox
}

/// The text of a whole block, marked for editors to find.
fn spawn_block_text(
    commands: &mut Commands,
    inline: &Inline,
    theme: &MarkdownTheme,
    size: f32,
    bold: bool,
    slots: &mut Vec<Slot>,
) -> Entity {
    let text = spawn_inline(commands, inline, theme, size, bold, slots);
    commands.entity(text).insert(BlockText);
    text
}

/// A text entity with one span per run.
fn spawn_inline(
    commands: &mut Commands,
    inline: &Inline,
    theme: &MarkdownTheme,
    size: f32,
    bold: bool,
    slots: &mut Vec<Slot>,
) -> Entity {
    let base = font(theme.body_font.clone(), size);
    let root = commands
        .spawn((Text::default(), base.clone(), TextColor(theme.text)))
        .id();
    let mut spans = Vec::with_capacity(inline.runs.len());
    for run in &inline.runs {
        let style = run.style;
        let mut font = if style.contains(Style::CODE) || style.contains(Style::MATH) {
            font(theme.mono_font.clone(), theme.code_size)
        } else {
            base.clone()
        };
        if bold || style.contains(Style::BOLD) {
            font.weight = FontWeight::BOLD;
        }
        if style.contains(Style::ITALIC) || style.contains(Style::IMAGE) {
            font.style = bevy::text::FontStyle::Italic;
        }
        let color = if run.link.is_some() {
            theme.link
        } else {
            theme.text
        };
        let mut span = commands.spawn((
            TextSpan::new(inline.run_text(run)),
            font,
            TextColor(color),
            ChildOf(root),
        ));
        if style.contains(Style::CODE) {
            span.insert(TextBackgroundColor(theme.code_background));
        }
        if style.contains(Style::STRIKE) {
            span.insert(Strikethrough);
        }
        if run.link.is_some() && !style.contains(Style::IMAGE) {
            span.insert(Underline);
        }
        spans.push(span.id());
    }
    slots.push(Slot { text: root, spans });
    root
}

fn font(source: FontSource, size: f32) -> TextFont {
    TextFont {
        font: source,
        font_size: FontSize::Px(size),
        ..default()
    }
}
