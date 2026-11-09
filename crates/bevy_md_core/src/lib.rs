//! Engine-free core of `bevy_md`: markdown parsed into a flat list of blocks,
//! reparsed incrementally on edits and appends.
//!
//! ```
//! use bevy_md_core::{BlockKind, Document};
//!
//! let mut doc = Document::parse("# Title\n\nSome **bold** text.\n");
//! assert_eq!(doc.block(0).unwrap().kind, BlockKind::Heading(1));
//!
//! // Streaming: only the blocks the new text can reach are rebuilt.
//! let splices = doc.push_str(" More.\n");
//! assert_eq!(splices.as_slice().len(), 1);
//! assert_eq!(splices[0].new, 1..2);
//! ```

mod block;
mod definitions;
mod document;
pub mod estimate;
mod inline;
mod parse;
mod positions;
mod splices;
pub mod sum_tree;
pub mod synthetic;

pub use block::{Align, Block, BlockKind, BlockRef, ListMarker};
pub use document::{Blocks, Definition, Document, MAX_DIFF_SPLICES, Stats};
pub use inline::{Bias, Inline, Run, Style};
pub use parse::default_options;
pub use pulldown_cmark::Options;
pub use splices::{Fate, Splice, Splices};
