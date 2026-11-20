# Why this is vendored

vt100 0.15.2 with ONE change (`src/grid.rs`, `visible_rows`): `rows_len -
scrollback_offset` → `rows_len.saturating_sub(scrollback_offset)`. Upstream
panics (subtract overflow) when the view is scrolled back further than one
screen height, which crew's chat history scroll does.

Fixed upstream in vt100 0.16, but 0.16 needs unicode-width ≥0.2.1 while
ratatui 0.29 pins =0.2.0. Drop this directory + the `[patch.crates-io]` entry
in Cargo.toml once crew moves to ratatui 0.30 / vt100 ≥0.16.
