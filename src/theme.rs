//! Color themes. crew draws with the 16 ANSI colors, so a theme is just a
//! terminal palette (fg, bg, ANSI 0–15) applied to the finished frame — the same
//! model as iTerm/Ghostty/Alacritty themes. The embedded agent terminals get
//! remapped too, so everything on screen matches. Palettes are from the
//! projects' published (MIT-licensed) terminal ports.

use ratatui::buffer::Buffer;
use ratatui::style::Color;
use std::sync::atomic::{AtomicUsize, Ordering};

pub struct Theme {
    pub name: &'static str,
    fg: u32,
    bg: u32,
    /// black red green yellow blue magenta cyan white, then the bright eight.
    ansi: [u32; 16],
}

pub const THEMES: &[Theme] = &[
    // "default" = your terminal's own colors (no remapping)
    Theme { name: "default", fg: 0, bg: 0, ansi: [0; 16] },
    Theme { name: "catppuccin", fg: 0xcdd6f4, bg: 0x1e1e2e, ansi: [
        0x45475a, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xbac2de,
        0x585b70, 0xf38ba8, 0xa6e3a1, 0xf9e2af, 0x89b4fa, 0xf5c2e7, 0x94e2d5, 0xa6adc8] },
    Theme { name: "dracula", fg: 0xf8f8f2, bg: 0x282a36, ansi: [
        0x21222c, 0xff5555, 0x50fa7b, 0xf1fa8c, 0xbd93f9, 0xff79c6, 0x8be9fd, 0xf8f8f2,
        0x6272a4, 0xff6e6e, 0x69ff94, 0xffffa5, 0xd6acff, 0xff92df, 0xa4ffff, 0xffffff] },
    Theme { name: "gruvbox", fg: 0xebdbb2, bg: 0x282828, ansi: [
        0x282828, 0xcc241d, 0x98971a, 0xd79921, 0x458588, 0xb16286, 0x689d6a, 0xa89984,
        0x928374, 0xfb4934, 0xb8bb26, 0xfabd2f, 0x83a598, 0xd3869b, 0x8ec07c, 0xebdbb2] },
    Theme { name: "nord", fg: 0xd8dee9, bg: 0x2e3440, ansi: [
        0x3b4252, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x88c0d0, 0xe5e9f0,
        0x4c566a, 0xbf616a, 0xa3be8c, 0xebcb8b, 0x81a1c1, 0xb48ead, 0x8fbcbb, 0xeceff4] },
    Theme { name: "tokyonight", fg: 0xc0caf5, bg: 0x1a1b26, ansi: [
        0x15161e, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xa9b1d6,
        0x414868, 0xf7768e, 0x9ece6a, 0xe0af68, 0x7aa2f7, 0xbb9af7, 0x7dcfff, 0xc0caf5] },
    Theme { name: "onedark", fg: 0xabb2bf, bg: 0x282c34, ansi: [
        0x3f4451, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xabb2bf,
        0x5c6370, 0xe06c75, 0x98c379, 0xe5c07b, 0x61afef, 0xc678dd, 0x56b6c2, 0xffffff] },
    // ponytail: brights mostly reuse the normals; strict Solarized brights are
    // greys, which would wash out crew's status colors.
    Theme { name: "solarized", fg: 0x839496, bg: 0x002b36, ansi: [
        0x073642, 0xdc322f, 0x859900, 0xb58900, 0x268bd2, 0xd33682, 0x2aa198, 0xeee8d5,
        0x586e75, 0xcb4b16, 0x859900, 0xb58900, 0x268bd2, 0x6c71c4, 0x2aa198, 0xfdf6e3] },
];

static CURRENT: AtomicUsize = AtomicUsize::new(0);

/// Select a theme by name; unknown or empty names fall back to "default".
pub fn set(name: &str) {
    let i = THEMES.iter().position(|t| t.name == name).unwrap_or(0);
    CURRENT.store(i, Ordering::Relaxed);
}

pub fn current() -> &'static str {
    THEMES[CURRENT.load(Ordering::Relaxed)].name
}

/// Step through the list (settings ←/→).
pub fn cycle(d: i64) {
    let n = THEMES.len() as i64;
    let i = (CURRENT.load(Ordering::Relaxed) as i64 + d).rem_euclid(n);
    CURRENT.store(i as usize, Ordering::Relaxed);
}

fn rgb(c: u32) -> Color {
    Color::Rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

fn ansi_index(c: Color) -> Option<usize> {
    Some(match c {
        Color::Black => 0, Color::Red => 1, Color::Green => 2, Color::Yellow => 3,
        Color::Blue => 4, Color::Magenta => 5, Color::Cyan => 6, Color::Gray => 7,
        Color::DarkGray => 8, Color::LightRed => 9, Color::LightGreen => 10,
        Color::LightYellow => 11, Color::LightBlue => 12, Color::LightMagenta => 13,
        Color::LightCyan => 14, Color::White => 15,
        Color::Indexed(i) if i < 16 => i as usize,
        _ => return None,
    })
}

fn map(t: &Theme, c: Color, default: u32) -> Color {
    match c {
        Color::Reset => rgb(default),
        c => ansi_index(c).map(|i| rgb(t.ansi[i])).unwrap_or(c),
    }
}

/// Remap the finished frame through the active palette. Truecolor and the
/// 256-color cube (≥16) pass through untouched.
pub fn apply(buf: &mut Buffer) {
    let i = CURRENT.load(Ordering::Relaxed);
    if i == 0 {
        return;
    }
    let t = &THEMES[i];
    for cell in buf.content.iter_mut() {
        cell.fg = map(t, cell.fg, t.fg);
        cell.bg = map(t, cell.bg, t.bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::layout::Rect;

    #[test]
    fn remaps_ansi_and_defaults_only() {
        set("dracula");
        let mut b = Buffer::empty(Rect::new(0, 0, 3, 1));
        b[(0, 0)].set_fg(Color::Red);
        b[(1, 0)].set_fg(Color::Rgb(1, 2, 3));
        b[(2, 0)].set_fg(Color::Indexed(200));
        apply(&mut b);
        assert_eq!(b[(0, 0)].fg, Color::Rgb(0xff, 0x55, 0x55));
        assert_eq!(b[(0, 0)].bg, Color::Rgb(0x28, 0x2a, 0x36)); // Reset → theme bg
        assert_eq!(b[(1, 0)].fg, Color::Rgb(1, 2, 3));
        assert_eq!(b[(2, 0)].fg, Color::Indexed(200));
        set("nope");
        assert_eq!(current(), "default");
    }
}
