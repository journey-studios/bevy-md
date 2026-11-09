# bevy_md

Fast markdown for [Bevy](https://bevyengine.org) UI: rendering today, Notion-style editing next.

- **Incremental by design.** A keystroke or a streamed token reparses only the blocks it can reach, and only their UI nodes are rebuilt.
- **Streaming-ready.** `Markdown::push_str` takes an LLM answer chunk by chunk.
- **Background parsing for big pastes.** Text over `Markdown::SYNC_LIMIT` (4 KiB) is parsed off the main thread, with revisions, and streamed chunks that arrive meanwhile are merged and applied after it lands. Cold opening is not yet guaranteed to stay below 16 ms per frame.
- **Virtualized.** Only the blocks near the screen have UI nodes, whatever the size of the document; subtrees that leave wait in a pool and are reused.
- **Engine-free core.** `bevy_md_core` has no Bevy dependency: a flat block model on top of [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark), usable from any renderer.

> Status: early. See [PLAN.md](PLAN.md) for the roadmap (in Portuguese).

For the implementation handoff, remaining performance criteria, and commands, see
[docs/HANDOFF.md](docs/HANDOFF.md).

## Crates

| Crate | What it does |
|---|---|
| `bevy_md_core` | Markdown to a flat list of blocks with styled runs; incremental edits and appends that always equal a full parse. |
| `bevy_md` | Bevy plugin: a `Markdown` component that renders the blocks as UI nodes and syncs only what changed. |
| `bevy_md_editor` | Rich text editing (phase 3, in progress): a caret over the rendered text whose edits go to the markdown source, with the editor only on the focused block and undo/redo (`Cmd/Ctrl+Z`, `Cmd/Ctrl+Shift+Z`) kept as inverted splices. |

## Usage

```rust
use bevy::prelude::*;
use bevy_md::{Markdown, MarkdownPlugin};

fn main() {
    App::new()
        .add_plugins((DefaultPlugins, MarkdownPlugin))
        .add_systems(Startup, |mut commands: Commands| {
            commands.spawn(Camera2d);
            commands.spawn(Markdown::new("# Hello\n\nFrom **bevy_md**."));
        })
        .run();
}
```

Streaming:

```rust
// From any system that owns the stream, e.g. with `mut markdown: Single<&mut Markdown>`:
markdown.push_str(token); // rebuilds only the blocks the token reached
```

## Development

```sh
cargo test -p bevy_md_core                       # fast, no Bevy build
BEVY_MD_SEEDS=3000 cargo test -p bevy_md_core --release --test incremental
cargo bench -p bevy_md_core                      # parse, keystroke and streaming budgets
cargo run -p bevy_md --example streaming --release
```

```sh
cargo run -p bevy_md_editor --example spike     # click, type, Backspace, arrows in a rendered paragraph
```

Block controls: add `MarkdownGutter` next to `Markdown` (with some left padding) and hovering a block shows a `+` button and a drag handle in the margin. It is one entity moved from block to block, never one per block; only the look is there so far, dragging and the menu come in phase 4.

```rust
commands.spawn((
    Markdown::new(text),
    MarkdownGutter,
    Node { padding: UiRect { left: px(64), ..UiRect::all(px(32)) }, ..default() },
));
```

Large texts: `set_source`, `push_str` and `edit` with more than `Markdown::SYNC_LIMIT` bytes are parsed on the `AsyncComputeTaskPool` (the `multi_threaded` feature, on by default; without it the parse runs on the main thread). `Markdown::source()` is always the text after every call so far, `is_parsing()` says whether the document shown is still behind it, and `parse_revision()` / `discarded_parses()` expose the revisions.

Fonts: the default theme uses the system's sans-serif and monospace families, which Bevy resolves only with its `system_font_discovery` feature. `bevy_md` turns it on through its default `system-fonts` feature; disable that if your theme uses font assets.

Supported Bevy version: **0.20**. Toolchain is pinned in `rust-toolchain.toml`.

## License

Dual-licensed under [MIT](LICENSE-MIT) or [Apache 2.0](LICENSE-APACHE), at your option.
