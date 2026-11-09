use bevy::prelude::*;
use bevy::text::{FontSource, GenericFontFamily};

/// Fonts, sizes and colors of rendered markdown.
#[derive(Resource, Clone, Debug)]
pub struct MarkdownTheme {
    /// Font of prose.
    pub body_font: FontSource,
    /// Font of code.
    pub mono_font: FontSource,
    /// Size of body text, in logical pixels.
    pub body_size: f32,
    /// Size of `#` to `######` headings, in logical pixels.
    pub heading_sizes: [f32; 6],
    /// Size of code, in logical pixels.
    pub code_size: f32,
    /// Prose color.
    pub text: Color,
    /// Quotes, list markers and other secondary text.
    pub muted: Color,
    /// Links.
    pub link: Color,
    /// Code backgrounds.
    pub code_background: Color,
    /// Rules, quote bars and table borders.
    pub border: Color,
    /// Vertical gap between blocks, in logical pixels.
    pub block_gap: f32,
    /// Vertical gap between two items of a list, in logical pixels.
    pub list_gap: f32,
    /// Indent per list level, in logical pixels; also the width of the
    /// column holding a list item's bullet, number or checkbox.
    pub indent: f32,
}

impl Default for MarkdownTheme {
    fn default() -> Self {
        Self {
            body_font: FontSource::Generic(GenericFontFamily::SansSerif),
            mono_font: FontSource::Generic(GenericFontFamily::Monospace),
            body_size: 16.0,
            heading_sizes: [30.0, 24.0, 20.0, 18.0, 16.0, 15.0],
            code_size: 14.0,
            text: Color::srgb(0.9, 0.9, 0.92),
            muted: Color::srgb(0.6, 0.6, 0.66),
            link: Color::srgb(0.45, 0.66, 1.0),
            code_background: Color::srgb(0.16, 0.16, 0.19),
            border: Color::srgb(0.3, 0.3, 0.35),
            block_gap: 10.0,
            list_gap: 4.0,
            indent: 24.0,
        }
    }
}
