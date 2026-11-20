//! Rendering. Master-detail: an agent list on the left; on the right either the
//! selected agent's *embedded terminal* (interactive) or a preview (observed).

use crate::agent::{rel_time, Kind};
use crate::app::{App, Mode, TreeItem};
use ratatui::{
    layout::{Constraint, Direction, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Wrap,
    },
    Frame,
};

// ── theme ────────────────────────────────────────────────────────────────
// One accent so the UI reads as a system (lazygit/k9s style). Borders are
// rounded and dim until a panel is focused, then they light up in the accent.
const ACCENT: Color = Color::Cyan;
const ACCENT_DIM: Color = Color::DarkGray;

/// Widths of the left tree and the (hidden, reserved for file previews) middle
/// pane for a terminal `total` cols wide. The tree carries repos *and* their
/// agents, so it gets ~30%; the chat/detail pane takes everything else.
pub fn pane_widths(_app: &App, total: u16) -> (u16, u16) {
    ((total * 30 / 100).clamp(34, 60).min(total / 2), 0)
}

/// A rounded panel with a dim border and a subtle title.
fn panel(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT_DIM))
        .title(Span::styled(title.to_string(), Style::default().fg(Color::Gray)))
}

/// A rounded panel whose border + title are lit in the accent (focused).
fn panel_focused(title: &str, color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(title.to_string(), Style::default().fg(color).add_modifier(Modifier::BOLD)))
}

/// A rounded modal dialog block with a caller-styled title span.
fn dialog(title: Span<'static>) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(ACCENT_DIM))
        .title(title)
}

/// Braille spinner frame for "running", cycled from wall-clock time.
fn spinner() -> &'static str {
    const F: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    let i = (now_millis() / 100 % 10) as usize;
    F[i]
}

/// Render a vertical scrollbar on the right edge of `area` for a scrolled view.
fn scrollbar(f: &mut Frame, area: Rect, pos: u16, total: usize) {
    if total <= area.height.saturating_sub(2) as usize {
        return; // everything fits — no scrollbar
    }
    let mut state = ScrollbarState::new(total).position(pos as usize);
    f.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None).end_symbol(None)
            .thumb_style(Style::default().fg(ACCENT)),
        area.inner(Margin { vertical: 1, horizontal: 0 }),
        &mut state,
    );
}

fn status_color(status: &str) -> Color {
    match status {
        "waiting" => Color::Yellow,
        "running" => Color::Green,
        "done" => Color::Green,
        "failed" => Color::Red,
        "resumable" => Color::Yellow,
        "exited" => Color::DarkGray,
        _ => Color::Cyan, // observed
    }
}

/// Per-harness accent so you can scan the list by tool at a glance (k9s-style).
fn harness_color(h: &str) -> Color {
    match h {
        "claude" => Color::Magenta,
        "cursor" => Color::Blue,
        "opencode" => Color::Green,
        "session" => Color::DarkGray,
        _ => Color::Gray,
    }
}

/// A compact unicode meter like `███░░░` for a 0–100 percentage.
fn meter(pct: f64, width: usize) -> String {
    let p = (pct / 100.0).clamp(0.0, 1.0);
    let filled = (p * width as f64).round() as usize;
    let mut s = String::new();
    for i in 0..width {
        s.push(if i < filled { '█' } else { '░' });
    }
    s
}

fn status_icon(status: &str) -> &'static str {
    match status {
        "waiting" => "⏸",
        "running" => "●",
        "done" => "✓",
        "failed" => "✗",
        "resumable" => "▷",
        "exited" => "□",
        _ => "·",
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    draw_inner(f, app);
    crate::theme::apply(f.buffer_mut()); // palette remap, last
}

fn draw_inner(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(5), Constraint::Length(1)])
        .split(f.area());

    draw_header(f, app, chunks[0]);

    if app.zoom {
        draw_right(f, app, chunks[1]); // [z]: detail/chat takes the full width
    } else {
        let (tree_w, _) = pane_widths(app, chunks[1].width);
        let cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Length(tree_w), Constraint::Min(20)])
            .split(chunks[1]);
        draw_tree(f, app, cols[0]);
        draw_right(f, app, cols[1]);
    }

    draw_footer(f, app, chunks[2]);

    match app.mode {
        Mode::NewAgent => draw_form(f, app),
        Mode::BrowseFolder => draw_browser(f, app),
        Mode::Help => draw_help(f, app),
        Mode::Diff => draw_diff(f, app),
        Mode::Log => draw_log(f, app),
        Mode::ConfirmStop => draw_confirm(f, app),
        Mode::ConfirmStopAll => draw_confirm_stop_all(f, app),
        Mode::ConfirmQuit => draw_confirm_quit(f, app),
        Mode::ConfirmDiscard => draw_confirm_discard(f),
        Mode::Retry => draw_retry(f, app),
        Mode::ConfirmPush => draw_confirm_push(f, app),
        Mode::Settings => draw_settings(f, app),
        Mode::ConfirmRemove => draw_confirm_remove(f, app),
        Mode::ConfirmClear => draw_confirm_clear(f, app),
        Mode::Rename => draw_rename(f, app),
        _ => {}
    }
    if app.leader_pending {
        draw_which_key(f, app);
    }
}

/// Which-key overlay (bottom-right) while the leader key is armed.
fn draw_which_key(f: &mut Frame, app: &App) {
    let b = &app.leader.bindings;
    let full = f.area();
    let w = 26.min(full.width);
    let h = (b.len() as u16 + 2).min(full.height);
    let area = Rect { x: full.right().saturating_sub(w), y: full.bottom().saturating_sub(h + 1), width: w, height: h };
    f.render_widget(Clear, area);
    let lines: Vec<Line> = b.iter().map(|(k, a)| Line::from(vec![
        Span::styled(format!("{:<6}", crate::leader::spec_label(k)), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
        Span::raw(a.name().replace('_', " ")),
    ])).collect();
    f.render_widget(Paragraph::new(lines)
        .block(dialog(Span::styled(" leader ", Style::default().fg(ACCENT)))), area);
}

fn draw_confirm_remove(f: &mut Frame, app: &App) {
    let area = centered(52, 20, f.area());
    f.render_widget(Clear, area);
    let what = if app.marked_total() == 0 {
        format!("'{}'", truncate(app.selected_row().map(|r| r.display()).unwrap_or(""), 30))
    } else {
        format!("{} marked agent(s)", app.marked_total())
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::raw(format!("  Remove {}?", what))),
        Line::from(Span::styled("  kills it if live, forgets it (no resume);", Style::default().fg(Color::DarkGray))),
        Line::from(Span::styled("  clean crew worktrees are pruned, dirty ones kept", Style::default().fg(Color::DarkGray))),
        Line::from(""),
        Line::from(Span::styled("  [y] remove   [n/esc] cancel", Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines)
        .block(dialog(Span::styled(" confirm remove ", Style::default().fg(Color::Red)))), area);
}

fn draw_confirm_clear(f: &mut Frame, app: &App) {
    let area = centered(56, 20, f.area());
    f.render_widget(Clear, area);
    let n = app.visible_rows().iter().filter(|r| r.resumable || r.status == "exited").count();
    let lines = vec![
        Line::from(""),
        Line::from(Span::raw(format!("  Forget {} finished agent(s)?", n))),
        Line::from(Span::styled("  they will no longer be resumable (worktrees kept)", Style::default().fg(Color::DarkGray))),
        Line::from(""),
        Line::from(Span::styled("  [y] clear   [n/esc] cancel", Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines)
        .block(dialog(Span::styled(" confirm clear ", Style::default().fg(Color::Red)))), area);
}

fn draw_rename(f: &mut Frame, app: &App) {
    let area = centered(56, 20, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled("  Label this agent (blank clears):", Style::default().fg(Color::Gray))),
        Line::from(vec![Span::raw("  "),
            Span::styled(format!("{}_", app.rename_buf),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(Span::styled("  [enter] save   [esc] cancel", Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines).block(panel_focused(" rename ", ACCENT)), area);
}

fn draw_settings(f: &mut Frame, app: &App) {
    let Some(p) = &app.settings else { return };
    let area = centered(62, 44, f.area());
    f.render_widget(Clear, area);
    use crate::app::{model_presets, HARNESSES, WORKFLOWS};
    let harness = HARNESSES[p.harness_idx.min(2)];
    let presets = model_presets(p.harness_idx);
    let model = presets[p.model_idx.min(presets.len() - 1)];
    let workflow = WORKFLOWS[p.workflow_idx.min(WORKFLOWS.len() - 1)];
    let wt = if p.worktree_new { "new git worktree (isolated)" } else { "current repo (shared)" };

    let row = |i: usize, label: &str, val: &str| {
        let active = app.settings_field == i;
        let arrow = if active { ACCENT } else { ACCENT_DIM };
        Line::from(vec![
            Span::styled(format!("{}{:<10}", if active { "▶ " } else { "  " }, label),
                if active { Style::default().fg(Color::White).add_modifier(Modifier::BOLD) }
                else { Style::default().fg(Color::Gray) }),
            Span::styled("‹ ", Style::default().fg(arrow)),
            Span::styled(val.to_string(), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
            Span::styled(" ›", Style::default().fg(arrow)),
        ])
    };
    let lines = vec![
        Line::from(Span::styled("  Default launch settings", Style::default().add_modifier(Modifier::BOLD))),
        Line::from(Span::styled("  used to pre-fill the new-agent form", Style::default().fg(Color::DarkGray))),
        Line::from(""),
        row(0, "harness", harness),
        row(1, "model", model),
        row(2, "workflow", workflow),
        row(3, "worktree", wt),
        row(4, "backend", &format!("{}  (all repos)", match app.settings_backend.as_str() {
            "tmux" => "tmux — agents survive quitting crew",
            "pty" => "pty — agents stop when crew exits",
            _ => "auto — tmux if installed, else pty",
        })),
        row(5, "theme", crate::theme::current()),
        Line::from(""),
        Line::from(Span::styled("  [tab] field   [←→] change   [enter] save   [esc] cancel",
            Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines).block(panel_focused(" settings ", ACCENT)), area);
}

fn draw_confirm_push(f: &mut Frame, app: &App) {
    let area = centered(64, 34, f.area());
    f.render_widget(Clear, area);
    let has_gh = crate::app::gh_available();
    let n = app.push_total();
    let target = if n == 1 {
        Line::from(vec![Span::styled("  push branch  ", Style::default().fg(Color::DarkGray)),
            Span::styled(app.push_queue.first().map(|q| q.1.clone()).unwrap_or_default(),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))])
    } else {
        Line::from(Span::styled(format!("  push {} marked agent(s)", n),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
    };
    let lines = vec![
        Line::from(""),
        target,
        Line::from(""),
        Line::from(Span::styled(
            if has_gh { "  → commits all changes, pushes to origin, opens a PR (gh)" }
            else { "  → commits all changes, pushes to origin (install gh for a PR)" },
            Style::default().fg(Color::Gray))),
        Line::from(""),
        Line::from(Span::styled("  [y] push   [n/esc] cancel", Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false })
        .block(panel_focused(" push + PR ", Color::Green)), area);
}

fn draw_retry(f: &mut Frame, app: &App) {
    let area = centered(60, 34, f.area());
    f.render_widget(Clear, area);
    let harness = crate::app::HARNESSES[app.retry_harness_idx.min(2)];
    let lines = vec![
        Line::from(""),
        Line::from(vec![Span::raw("  re-delegate: "),
            Span::styled(truncate(&app.retry_task, area.width.saturating_sub(18) as usize),
                Style::default().fg(Color::White).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from(vec![
            Span::styled("  harness  ", Style::default().fg(Color::DarkGray)),
            Span::styled("‹ ", Style::default().fg(ACCENT)),
            Span::styled(harness, Style::default().fg(harness_color(harness)).add_modifier(Modifier::BOLD)),
            Span::styled(" ›", Style::default().fg(ACCENT)),
            Span::styled("   (←/→ change)", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(""),
        Line::from(Span::styled("  runs headless in a fresh worktree — appears as a new agent",
            Style::default().fg(Color::DarkGray))),
        Line::from(Span::styled("  [enter] dispatch   [esc] cancel", Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false })
        .block(panel_focused(" retry / re-delegate ", ACCENT)), area);
}

fn draw_header(f: &mut Frame, app: &App, area: Rect) {
    let mode = match app.mode {
        Mode::Terminal => " CHAT ",
        _ => "",
    };
    // per-status tallies + total metered cost across every visible row
    let rows = app.visible_rows();
    let count = |s: &str| rows.iter().filter(|r| r.status_label() == s).count();
    let (waiting, running, done, failed) = (count("waiting"), count("running"), count("done"), count("failed"));
    let total_cost: f64 = rows.iter().map(|r| r.cost_usd).sum();

    let mut spans = vec![
        Span::styled(" crew ", Style::default().bg(Color::Blue).fg(Color::White).add_modifier(Modifier::BOLD)),
    ];
    if app.view_all {
        let n = app.workspaces.len();
        spans.push(Span::raw(format!(" all · {} repo{} ", n, if n == 1 { "" } else { "s" })));
    } else {
        spans.push(Span::raw(format!(" {} ", app.name())));
        if !app.repo_branch.is_empty() {
            spans.push(Span::styled(format!(" {} ", app.repo_branch), Style::default().fg(Color::Yellow)));
        }
        if app.store.has_engine() {
            spans.push(Span::styled("⚙ engine ", Style::default().fg(Color::Green)));
        }
    }
    let n = rows.len();
    spans.push(Span::styled(format!("· {} agent{}  ", n, if n == 1 { "" } else { "s" }), Style::default().fg(Color::Gray)));
    // colored status legend, only showing non-zero buckets
    let legend = [(waiting, "⏸", Color::Yellow), (running, "●", Color::Green), (done, "✓", Color::Green), (failed, "✗", Color::Red)];
    for (n, glyph, color) in legend {
        if n > 0 {
            spans.push(Span::styled(format!("{}{} ", glyph, n), Style::default().fg(color)));
        }
    }
    if total_cost > 0.0 {
        spans.push(Span::styled(format!(" ${:.3} ", total_cost),
            Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)));
    }
    if app.marked_total() > 0 {
        spans.push(Span::styled(format!(" ◉ {} marked ", app.marked_total()),
            Style::default().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD)));
    }
    // active category / non-default sort chips (k9s-style)
    if app.category != crate::app::Category::All {
        spans.push(Span::styled(format!(" [{}] ", app.category.label()),
            Style::default().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD)));
    }
    if app.sort != crate::app::SortKey::Status {
        spans.push(Span::styled(format!(" ↓{} ", app.sort.label()), Style::default().fg(ACCENT)));
    }
    if app.mode == Mode::Filter || !app.filter_text.is_empty() {
        let caret = if app.mode == Mode::Filter { "_" } else { "" };
        spans.push(Span::styled(format!(" /{}{} ", app.filter_text, caret), Style::default().fg(ACCENT)));
    }
    if !mode.is_empty() {
        spans.push(Span::styled(mode, Style::default().bg(Color::Green).fg(Color::Black).add_modifier(Modifier::BOLD)));
    }
    spans.push(Span::styled(format!(" {}", app.status_msg), Style::default().fg(Color::Yellow)));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The workspaces pane: pinned folders, the active one highlighted, each with a
/// live-agent count badge. Focused when `Tab` has moved focus here.
/// Left pane: "all", then each repo with its agents nested beneath it.
fn draw_tree(f: &mut Frame, app: &App, area: Rect) {
    let n_agents: usize = app.workspaces.iter().map(|w| w.rows.len()).sum();
    let title = format!(" repos ({}) · agents ({}) ", app.workspaces.len(), n_agents);
    let inner_w = area.width.saturating_sub(4) as usize; // borders + highlight symbol
    let focused = app.mode == Mode::Normal || app.mode == Mode::Filter;
    let block = if focused { panel_focused(&title, ACCENT) } else { panel(&title) };

    if app.workspaces.is_empty() {
        f.render_widget(
            Paragraph::new("\n  no workspaces.\n\n  press [w] to pin one.").style(Style::default().fg(Color::DarkGray)).block(block),
            area,
        );
        return;
    }

    let cur = app.tree_cursor();
    let now = now_secs();
    let green = Style::default().fg(Color::Green);
    let badge = |n: usize| if n > 0 { format!(" ●{n}") } else { String::new() };
    let mut items: Vec<ListItem> = Vec::new();
    let mut sel = 0;
    for (pos, it) in app.tree().into_iter().enumerate() {
        if it == cur {
            sel = pos;
        }
        items.push(match it {
            TreeItem::All => {
                let total_live: usize = app.workspaces.iter().map(|w| w.live_count).sum();
                ListItem::new(Line::from(vec![
                    Span::styled("◆ ", Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
                    Span::styled("all", Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                    Span::styled(badge(total_live), green),
                ]))
            }
            TreeItem::Repo(i) => {
                let w = &app.workspaces[i];
                let b = badge(w.live_count);
                let launched = if Some(i) == app.cwd_ws { "› " } else { "" };
                let name_w = inner_w.saturating_sub(2 + launched.len() + b.chars().count());
                ListItem::new(Line::from(vec![
                    Span::styled("▾ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(launched, Style::default().fg(Color::DarkGray)),
                    Span::styled(truncate(&w.name(), name_w.max(4)), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
                    Span::styled(b, green),
                ]))
            }
            TreeItem::Agent(i, j) => {
                let ws = &app.workspaces[i];
                let r = &ws.rows[j];
                let st = r.status_label();
                let glyph = if st == "running" { spinner() } else { status_icon(st) };
                let marked = ws.marked.contains(&r.id);
                let dirt = if ws.dirty.get(&r.id).copied().unwrap_or(false) { "±" } else { " " };
                let meta = rel_time(r.updated_ts, now).trim_end_matches(" ago").to_string();
                let cost = if r.cost_usd > 0.0 { format!("${:.2} ", r.cost_usd) } else { String::new() };
                let tail = cost.len() + meta.len();
                let shown = if r.display().is_empty() { "(no task)" } else { r.display() };
                let task_w = inner_w.saturating_sub(2 + 2 + 2 + tail + 1).max(6);
                let task = truncate(shown, task_w);
                let pad = inner_w.saturating_sub(2 + 2 + 2 + task.chars().count() + tail);
                ListItem::new(Line::from(vec![
                    Span::styled(if marked { "◉ " } else { "  " }, Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
                    Span::styled(format!("{glyph} "), Style::default().fg(status_color(st))),
                    Span::styled(format!("{dirt} "), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    Span::raw(task),
                    Span::raw(" ".repeat(pad)),
                    Span::styled(cost, Style::default().fg(Color::Magenta)),
                    Span::styled(meta, Style::default().fg(Color::DarkGray)),
                ]))
            }
        });
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(if focused {
            Style::default().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::REVERSED)
        })
        .highlight_symbol("▶ ");
    let mut state = ListState::default();
    state.select(Some(sel));
    f.render_stateful_widget(list, area, &mut state);
}

/// Empty-state onboarding: what crew found (git, harness CLIs) + next step.
fn onboarding(app: &App) -> Vec<Line<'static>> {
    let ok = |b: bool| if b { Span::styled("✓ ", Style::default().fg(Color::Green)) } else { Span::styled("✗ ", Style::default().fg(Color::Red)) };
    let dim = Style::default().fg(Color::DarkGray);
    let mut l = vec![Line::from(""), Line::from(Span::styled("  no agents yet", Style::default().fg(Color::White).add_modifier(Modifier::BOLD))), Line::from("")];
    if !app.view_all {
        l.push(Line::from(vec![Span::raw("  "), ok(app.is_git),
            Span::styled(if app.is_git { "git repo · own worktree per agent" } else { "plain folder · agents share it" }, dim)]));
    }
    for h in crate::app::HARNESSES {
        l.push(Line::from(vec![Span::raw("  "), ok(crate::app::harness_installed(h)),
            Span::styled(format!("{} ({})", h, crate::app::harness_bin(h)), dim)]));
    }
    l.push(Line::from(""));
    l.push(Line::from(Span::styled("  [n] launch an agent", Style::default().fg(ACCENT))));
    l.push(Line::from(Span::styled("  [w] pin another repo", dim)));
    l.push(Line::from(Span::styled("  [?] all keys", dim)));
    l
}

fn draw_right(f: &mut Frame, app: &App, area: Rect) {
    // live agents, and exited ones whose last screen we still hold
    if !app.agent_selected() {
        draw_overview(f, app, area);
        return;
    }
    if let Some(idx) = app.selected_row().and_then(|r| r.proc_idx) {
        draw_terminal(f, app, idx, area);
        return;
    }
    draw_preview(f, app, area);
}

fn vt_color(c: vt100::Color) -> Color {
    match c {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// Paint a vt100 screen into `area` cell by cell (colors, bold/italic/
/// underline/inverse, wide chars), plus a block cursor when focused and at the
/// live bottom.
fn render_screen(buf: &mut ratatui::buffer::Buffer, area: Rect, screen: &vt100::Screen, cursor: bool) {
    for row in 0..area.height {
        for col in 0..area.width {
            let Some(c) = screen.cell(row, col) else { continue };
            if c.is_wide_continuation() {
                continue; // the wide char to the left covers this cell
            }
            let mut st = Style::default().fg(vt_color(c.fgcolor())).bg(vt_color(c.bgcolor()));
            if c.bold() { st = st.add_modifier(Modifier::BOLD); }
            if c.italic() { st = st.add_modifier(Modifier::ITALIC); }
            if c.underline() { st = st.add_modifier(Modifier::UNDERLINED); }
            if c.inverse() { st = st.add_modifier(Modifier::REVERSED); }
            let cell = &mut buf[(area.x + col, area.y + row)];
            let sym = c.contents();
            cell.set_symbol(if c.has_contents() { &sym } else { " " });
            cell.set_style(st);
        }
    }
    let (r, c) = screen.cursor_position();
    if cursor && !screen.hide_cursor() && screen.scrollback() == 0 && r < area.height && c < area.width {
        let cell = &mut buf[(area.x + c, area.y + r)];
        cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
    }
}

/// The embedded terminal — the agent's live REPL, rendered inside the TUI.
fn draw_terminal(f: &mut Frame, app: &App, idx: usize, area: Rect) {
    let focused = app.mode == Mode::Terminal;
    let live = app.selected_row().map(|r| r.is_live()).unwrap_or(false);
    let zoom = if app.zoom { "[z] unzoom" } else { "[z] zoom" };
    let title = if focused {
        format!(" chat · typing goes to the agent · [{}] leader ", crate::leader::spec_label(&app.leader.chat_key))
    } else if live {
        format!(" terminal · [enter/l] chat · {} ", zoom)
    } else {
        format!(" exited · last screen · [enter/l] resume · {} ", zoom)
    };
    // scrolled into history? say so, and how to get back
    let back = app.agents.get(idx)
        .and_then(|p| p.parser.lock().ok().map(|p| p.screen().scrollback()))
        .unwrap_or(0);
    let title = if back > 0 {
        format!(" ↑ {} lines back · wheel / Shift-PgDn / type to return ", back)
    } else {
        title
    };
    let block = if focused {
        panel_focused(&title, Color::Green)
    } else {
        panel(&title)
    };
    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(proc) = app.agents.get(idx) {
        // keep the PTY sized to the visible pane
        proc.resize(inner.height.max(1), inner.width.max(1));
        if let Ok(parser) = proc.parser.lock() {
            render_screen(f.buffer_mut(), inner, parser.screen(), focused);
        }
    }
}

/// Right pane while the cursor is on "all" or a repo (no agent picked).
fn draw_overview(f: &mut Frame, app: &App, area: Rect) {
    let block = panel(" crew ");
    if app.workspaces.iter().all(|w| w.all_rows.is_empty()) {
        f.render_widget(Paragraph::new(onboarding(app)).block(block), area);
        return;
    }
    let what = match app.tree_cursor() {
        TreeItem::Repo(i) => format!(" {} ", app.workspaces[i].name()),
        _ => " all repos ".to_string(),
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(what, Style::default().fg(Color::White).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(Span::styled("  select an agent to chat or inspect it", Style::default().fg(Color::DarkGray))),
        Line::from(Span::styled("  [n] launch an agent here · [w] pin another repo", Style::default().fg(ACCENT))),
    ];
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_preview(f: &mut Frame, app: &App, area: Rect) {
    let block = panel(" preview ");
    let Some(r) = app.selected_row() else {
        let p = Paragraph::new("\n  select an agent, or press [n] to launch one.")
            .style(Style::default().fg(Color::DarkGray)).block(block);
        f.render_widget(p, area);
        return;
    };
    let inner_w = block.inner(area).width as usize;
    let field = |k: &str| Span::styled(format!("{:<9}", k), Style::default().fg(Color::DarkGray));
    let st = r.status_label();
    let glyph = if st == "running" { spinner() } else { status_icon(st) };
    let mut lines = vec![
        Line::from(Span::styled(
            truncate(if r.display().is_empty() { "(no task)" } else { r.display() }, inner_w.saturating_sub(1)),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from(vec![field("status"), Span::styled(
            format!("{} {}", glyph, if r.waiting { "waiting for input" } else { st }),
            Style::default().fg(status_color(st)))]),
        Line::from(vec![field("repo"), Span::raw(app.name())]),
        Line::from(vec![field("harness"),
            Span::styled(r.harness.clone(), Style::default().fg(harness_color(&r.harness))),
            Span::styled(format!(" · {}", r.model), Style::default().fg(Color::Gray))]),
    ];
    if let Some((k, n)) = r.steps {
        lines.push(Line::from(vec![field("steps"),
            Span::styled(format!("{}/{}", k, n), Style::default().fg(ACCENT))]));
    } else if let Some(i) = r.step_index {
        lines.push(Line::from(vec![field("step"), Span::raw(format!("#{}", i))]));
    }
    if !r.branch.is_empty() && r.branch != "-" {
        lines.push(Line::from(vec![field("branch"),
            Span::styled(r.branch.clone(), Style::default().fg(Color::Yellow))]));
    }
    if let Some(rem) = &app.sel_remote {
        if !rem.is_empty() && r.worktree.is_some() {
            let color = match rem.as_str() {
                "up to date" => Color::Green,
                "not pushed" => Color::DarkGray,
                _ => Color::Yellow,
            };
            lines.push(Line::from(vec![field("remote"), Span::styled(rem.clone(), Style::default().fg(color))]));
        }
    }
    if let Some((n, _)) = app.selected_pr() {
        lines.push(Line::from(vec![field("PR"),
            Span::styled(format!("#{} open  [O] open", n),
                Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD))]));
    }
    if r.cost_usd > 0.0 {
        lines.push(Line::from(vec![field("cost"),
            Span::styled(format!("${:.3}", r.cost_usd), Style::default().fg(Color::Magenta))]));
    }
    if let Some(p) = r.ctx_pct {
        // btop-style meter; reddens as context fills up
        let color = if p >= 85.0 { Color::Red } else if p >= 60.0 { Color::Yellow } else { Color::Green };
        lines.push(Line::from(vec![field("context"),
            Span::styled(meter(p, 12), Style::default().fg(color)),
            Span::styled(format!(" {:.0}%", p), Style::default().fg(color))]));
    }
    if r.updated_ts > 0.0 {
        let label = if r.status == "running" { "running" } else { "updated" };
        lines.push(Line::from(vec![field(label),
            Span::styled(rel_time(r.updated_ts, now_secs()), Style::default().fg(Color::DarkGray))]));
    }
    // git status of the selected worktree (cached)
    if let Some((path, sum, clean)) = &app.worktree_status {
        if Some(path) == r.worktree.as_ref() {
            let color = if *clean { Color::Green } else { Color::Yellow };
            lines.push(Line::from(vec![field("changes"),
                Span::styled(sum.clone(), Style::default().fg(color))]));
        }
    }
    if let Some(wt) = &r.worktree {
        lines.push(Line::from(vec![field("worktree"),
            Span::styled(truncate(wt, inner_w.saturating_sub(10)), Style::default().fg(Color::DarkGray))]));
    }
    lines.push(Line::from(""));
    let hint = match r.kind {
        Kind::Observed => "[enter] take over this claude session as an agent",
        _ if r.resumable => "[enter] resume · [d]iff [e]dit [!]shell [P]ush [x]rm",
        _ => "[enter] log · [d]iff [L]og [e]dit [!]shell [P]ush [R]etry [x]rm",
    };
    lines.push(Line::from(Span::styled(hint, Style::default().fg(ACCENT_DIM))));
    f.render_widget(Paragraph::new(lines).block(block).wrap(Wrap { trim: false }), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let chat = format!("CHAT — [{}] leader · q back · Tab agents · [wheel / Shift-PgUp] history · [Shift-drag] select text · [shift-tab] agent modes", crate::leader::spec_label(&app.leader.chat_key));
    let keys = match app.mode {
        Mode::Terminal => chat.as_str(),
        Mode::Diff => "DIFF — [j/k] line [^d/^u] half [^f/^b] page [g/G] ends [/] search [n/N] next   ·   [q/esc] close",
        Mode::Log => "LOG — [j/k] line [^d/^u] half [g/G] ends (G follows) [/] search [r]eload   ·   [q/esc] close",
        Mode::BrowseFolder => "PIN A REPO — [j/k] move · [l/enter] open · [h/bksp] up · [a] pin · [/] type path · [esc] cancel",
        Mode::Normal | Mode::Filter if !app.agent_selected() => "REPO — [j/k] move · [l] first agent · [n]ew agent · [w] pin folder · [x] unpin · [?] help",
        // most-needed first: narrow terminals cut from the right
        Mode::Normal | Mode::Filter => "[?]help [n]ew [enter]chat/resume [q]uit [j/k]move [h]repo [d]iff [P]ush [s]top [x]rm [z]oom",
        _ => "", // modals carry their own key hints
    };
    f.render_widget(
        Paragraph::new(Span::styled(keys, Style::default().fg(Color::DarkGray))),
        area,
    );
}

fn draw_form(f: &mut Frame, app: &App) {
    let area = centered(76, 66, f.area());
    f.render_widget(Clear, area);
    let form = &app.new_form;
    use crate::app::{FIELD_HARNESS, FIELD_IMAGES, FIELD_MODEL, FIELD_REPO, FIELD_TASK, FIELD_WORKFLOW, FIELD_WORKTREE};
    let engine = app.store.has_engine();
    // progressive disclosure: advanced options fold into one line until focused
    let open = form.field >= FIELD_WORKFLOW;

    let block = panel_focused(" new agent ", ACCENT);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let adv_h = if open { 4 } else { 1 };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),     // label: task
            Constraint::Length(6),     // task editor
            Constraint::Length(1),     // repo
            Constraint::Length(1),     // harness
            Constraint::Length(adv_h), // advanced (summary or 4 pickers)
            Constraint::Length(1),     // images
            Constraint::Length(1),     // where it runs
            Constraint::Min(1),        // hints / error
        ])
        .split(inner);

    let lbl = |i: usize, t: &str| {
        let active = form.field == i;
        Span::styled(
            format!("{}{:<10}", if active { "▶ " } else { "  " }, t),
            if active { Style::default().fg(Color::White).add_modifier(Modifier::BOLD) }
            else { Style::default().fg(Color::Gray) },
        )
    };
    let picker = |i: usize, value: &str, note: Span<'static>| {
        let arrow = if form.field == i { Color::Cyan } else { Color::DarkGray };
        let mut v = Vec::new();
        v.push(Span::styled("‹ ", Style::default().fg(arrow)));
        v.push(Span::styled(value.to_string(), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)));
        v.push(Span::styled(" ›", Style::default().fg(arrow)));
        v.push(note);
        v
    };
    let line = |i: usize, label: &str, value: &str, note: Span<'static>| {
        let mut v = vec![lbl(i, label)];
        v.extend(picker(i, value, note));
        Line::from(v)
    };
    let dim = |t: String| Span::styled(t, Style::default().fg(Color::DarkGray));

    f.render_widget(Paragraph::new(Line::from(lbl(FIELD_TASK, "task *"))), rows[0]);
    let mut task = form.task.clone();
    let te_border = if form.field == FIELD_TASK { Color::Cyan } else { Color::DarkGray };
    task.set_block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(te_border)));
    f.render_widget(&task, rows[1]);

    let repo_note = dim(format!("  {}/{}{}", form.ws_idx + 1, app.workspaces.len(),
        if Some(form.ws_idx) == app.cwd_ws { " · launched here" } else { "" }));
    f.render_widget(Paragraph::new(line(FIELD_REPO, "repo", &app.name(), repo_note)), rows[2]);

    let inst = if crate::app::harness_installed(form.harness()) {
        Span::styled("  ✓ installed", Style::default().fg(Color::Green))
    } else {
        Span::styled(format!("  ✗ '{}' not on PATH", crate::app::harness_bin(form.harness())), Style::default().fg(Color::Red))
    };
    let inst = if form.harness() == "claude" && !form.claude_trusted && crate::app::harness_installed("claude") {
        Span::styled("  ⚠ untrusted: claude asks, pick Yes", Style::default().fg(Color::Yellow))
    } else {
        inst
    };
    f.render_widget(Paragraph::new(line(FIELD_HARNESS, "harness", form.harness(), inst)), rows[3]);

    let wt = if !app.is_git { "this folder (not a git repo)" }
        else if form.worktree_new { "new git worktree (isolated)" } else { "this repo checkout (shared)" };
    if open {
        let adv = Layout::default().direction(Direction::Vertical)
            .constraints([Constraint::Length(1); 4]).split(rows[4]);
        // workflow picker only exists with the engine (its skills run plan/build)
        let mut slot = adv.iter();
        if engine {
            f.render_widget(Paragraph::new(line(FIELD_WORKFLOW, "workflow", form.workflow(), dim(String::new()))), *slot.next().unwrap());
        }
        f.render_widget(Paragraph::new(line(FIELD_MODEL, "model", form.model(), dim(String::new()))), *slot.next().unwrap());
        f.render_widget(Paragraph::new(line(FIELD_WORKTREE, "runs in", wt, dim(String::new()))), *slot.next().unwrap());
    } else {
        let wf = if engine && form.workflow_idx != 0 { format!("{} · ", form.workflow()) } else { String::new() };
        f.render_widget(Paragraph::new(Line::from(vec![
            Span::styled("  options   ", Style::default().fg(Color::Gray)),
            dim(format!("{}{} · {}  [tab]", wf, form.model(), wt)),
        ])), rows[4]);
    }

    let img = if let Some(buf) = &form.image_input {
        format!("typing path: {}_", buf)
    } else if form.images.is_empty() {
        "(none — [a] add, [d] remove)".to_string()
    } else {
        format!("{}  ([a] add, [d] remove)", form.images.join(", "))
    };
    f.render_widget(Paragraph::new(Line::from(vec![lbl(FIELD_IMAGES, "images"),
        Span::styled(img, Style::default().fg(Color::Gray))])), rows[5]);

    // say exactly where this lands before anything is created
    let base = if app.repo_branch.is_empty() { "HEAD".to_string() } else { app.repo_branch.clone() };
    let where_ = if app.is_git && form.worktree_new {
        let slug = crate::app::slugify(&form.task_text());
        let slug = slug.rsplit_once('-').map(|(a, _)| a).unwrap_or(&slug).to_string();
        format!("  → new branch crew/{}-… off {}", slug, base)
    } else if app.is_git {
        format!("  → works directly on {} in {}", base, app.path())
    } else {
        format!("  → runs in {}", app.path())
    };
    f.render_widget(Paragraph::new(Span::styled(truncate(&where_, inner.width as usize),
        Style::default().fg(Color::Yellow))), rows[6]);

    let mut hints = Vec::new();
    if let Some(err) = &form.error {
        hints.push(Line::from(Span::styled(format!("  ⚠ {}", err), Style::default().fg(Color::Red))));
    }
    hints.push(Line::from(Span::styled(
        "  [enter] launch [alt-enter] newline [tab] field [h/l] pick [esc] cancel",
        Style::default().fg(Color::DarkGray))));
    f.render_widget(Paragraph::new(hints).wrap(Wrap { trim: false }), rows[7]);
}

fn draw_help(f: &mut Frame, app: &App) {
    let area = centered(86, 92, f.area());
    f.render_widget(Clear, area);
    // grouped cheatsheet (lazygit-style): (section, [(keys, desc)])
    let groups: [(&str, &[(&str, &str)]); 6] = [
        ("Navigate (vim)", &[
            ("j / k · ↑ / ↓", "move"),
            ("h / l · ← / →", "repos pane · open (chat / resume / log)"),
            ("g / G · ^d / ^u", "top / bottom · half page (lists + viewers)"),
            ("tab · a", "toggle panes · back to all repos"),
            ("] / [", "next / prev failed agent"),
            ("space / A", "mark row / mark all"),
            ("esc", "clear marks → search → category"),
        ]),
        ("Launch & drive", &[
            ("n · D", "new agent · duplicate selected into the form"),
            ("enter / l", "chat (live) · resume ▷"),
            ("Ctrl-q", "leave the chat"),
            ("z", "zoom chat / preview to full width"),
            ("wheel · S-PgUp/Dn", "scroll the chat's history (typing returns)"),
            ("Shift-drag", "select text to copy (crew captures the mouse)"),
            ("paste", "on the list: starts a new agent with it as the task"),
            ("trust", "untrusted repo: claude defaults to No — pick Yes, or run claude in the repo once"),
            ("shift-tab", "(in chat) cycle the agent's modes"),
            ("w", "pin another repo"),
        ]),
        ("Inspect", &[
            ("d / L", "worktree diff · log tail (headless agents)"),
            ("o / y / Y", "worktree path · yank path · yank branch"),
            ("e / !", "$EDITOR · $SHELL in the worktree"),
        ]),
        ("Ship", &[
            ("P", "commit + push selected/marked, open PR (gh)"),
            ("O / f", "open PR/branch in browser · git fetch"),
            ("R", "retry on another harness (engine only)"),
        ]),
        ("Manage", &[
            ("s / S", "stop selected/marked · stop all (resumable)"),
            ("x / c", "remove selected/marked · forget finished"),
            ("r", "rename (label) an agent"),
        ]),
        ("Filter & sort", &[
            ("1 2 3 4", "all / running / agents / sessions"),
            ("/ · t · H", "search · cycle sort · hide observed sessions"),
            (",", "settings: default harness/model/workflow"),
        ]),
    ];
    let mut lines: Vec<Line> = Vec::new();
    for (title, rows) in groups {
        lines.push(Line::from(Span::styled(format!(" {}", title),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))));
        for (k, d) in rows {
            lines.push(Line::from(vec![
                Span::styled(format!("  {:<19}", k), Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)),
                Span::styled(d.to_string(), Style::default().fg(Color::Gray)),
            ]));
        }
        lines.push(Line::from(""));
    }
    lines.push(Line::from(vec![
        Span::styled(" legend  ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("⏸ waiting  ", Style::default().fg(Color::Yellow)),
        Span::styled("● running  ", Style::default().fg(Color::Green)),
        Span::styled("✓ done  ", Style::default().fg(Color::Green)),
        Span::styled("✗ failed  ", Style::default().fg(Color::Red)),
        Span::styled("▷ resumable  ", Style::default().fg(Color::Yellow)),
        Span::styled("± uncommitted  ", Style::default().fg(Color::Yellow)),
        Span::styled("◉ marked", Style::default().fg(ACCENT)),
    ]));
    f.render_widget(Paragraph::new(lines)
        .scroll((app.help_scroll, 0))
        .block(panel_focused(" help — [j/k ^d/^u g/G] scroll · any other key closes ", ACCENT)), area);
}

/// Modal folder browser for pinning a new workspace.
fn draw_browser(f: &mut Frame, app: &App) {
    let area = centered(74, 82, f.area());
    f.render_widget(Clear, area);
    let b = &app.browser;
    let inner_w = area.width.saturating_sub(4) as usize;

    let path_line = if b.editing {
        vec![
            Span::styled("  path  ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}_", b.typed), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        ]
    } else {
        vec![
            Span::styled("  dir   ", Style::default().fg(Color::DarkGray)),
            Span::styled(truncate(&b.cwd.to_string_lossy(), inner_w.saturating_sub(8)), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ]
    };

    let mut lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(path_line),
        Line::from(""),
    ];
    // scroll window around the selection so long dirs stay usable
    let (_, list_h) = (area.height, area.height.saturating_sub(7) as usize);
    let start = b.selected.saturating_sub(list_h.saturating_sub(1));
    for (i, (name, _)) in b.entries.iter().enumerate().skip(start).take(list_h) {
        let selected = i == b.selected;
        let style = if selected {
            Style::default().bg(ACCENT).fg(Color::Black).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        let prefix = if selected { "▶ " } else { "  " };
        let pinned = name != ".." && app.workspaces.iter().any(|w| w.store.repo_root == b.cwd.join(name));
        lines.push(Line::from(vec![
            Span::styled(format!("{}{}/", prefix, name), style),
            Span::styled(if pinned { "  · pinned" } else { "" }, Style::default().fg(Color::DarkGray)),
        ]));
    }
    if let Some(err) = &b.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(format!("  ⚠ {}", err), Style::default().fg(Color::Red))));
    }
    if b.editing {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  [type] path   [enter] go   [esc] cancel edit",
            Style::default().fg(Color::DarkGray))));
    } else {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled("  [j/k] move  [l] open  [h] up  [a] pin  [/] type path  [esc] cancel",
            Style::default().fg(Color::DarkGray))));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false })
        .block(panel_focused(" pin a repo ", ACCENT)), area);
}

/// Full-screen colored diff of the selected agent's worktree.
fn draw_diff(f: &mut Frame, app: &App) {
    let area = centered(90, 88, f.area());
    f.render_widget(Clear, area);
    let q = app.find_query.to_lowercase();
    let lines: Vec<Line> = app.diff_text.lines().map(|l| {
        if !q.is_empty() && l.to_lowercase().contains(&q) {
            return Line::from(Span::styled(l.to_string(),
                Style::default().bg(Color::Yellow).fg(Color::Black).add_modifier(Modifier::BOLD)));
        }
        let style = if l.starts_with('+') && !l.starts_with("+++") {
            Style::default().fg(Color::Green)
        } else if l.starts_with('-') && !l.starts_with("---") {
            Style::default().fg(Color::Red)
        } else if l.starts_with("@@") {
            Style::default().fg(Color::Cyan)
        } else if l.starts_with("diff ") || l.starts_with("── ") {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        Line::from(Span::styled(l.to_string(), style))
    }).collect();
    let total = lines.len();
    let title = format!(" diff · {} lines ·{} [/]search [q]close ", total, find_hint(app));
    f.render_widget(
        Paragraph::new(lines)
            .scroll((app.diff_scroll, 0))
            .block(panel_focused(&title, ACCENT)),
        area,
    );
    scrollbar(f, area, app.diff_scroll, total);
}

/// Title fragment showing the in-viewer search state ("" when idle).
fn find_hint(app: &App) -> String {
    if app.find_input {
        format!(" /{}_ ·", app.find_query)
    } else if !app.find_query.is_empty() {
        format!(" /{} [n/N] ·", app.find_query)
    } else {
        String::new()
    }
}

/// Scrollable tail of a headless agent's log file.
fn draw_log(f: &mut Frame, app: &App) {
    let area = centered(90, 88, f.area());
    f.render_widget(Clear, area);
    let total = app.log_text.lines().count();
    let q = app.find_query.to_lowercase();
    let lines: Vec<Line> = app.log_text.lines().map(|l| {
        let low = l.to_ascii_lowercase();
        if !q.is_empty() && low.contains(&q) {
            return Line::from(Span::styled(l.to_string(),
                Style::default().bg(Color::Yellow).fg(Color::Black).add_modifier(Modifier::BOLD)));
        }
        // highlight anything that reads like an error
        let style = if low.contains("error") || low.contains("\"is_error\":true") || low.contains("traceback") {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::Gray)
        };
        Line::from(Span::styled(l.to_string(), style))
    }).collect();
    let at_bottom = app.log_scroll as usize >= total.saturating_sub(1);
    let follow = if at_bottom { "⏵following" } else { "⏸scrollback" };
    let title = format!(" log · {} lines · {} ·{} [/]search [q]close ", total, follow, find_hint(app));
    f.render_widget(
        Paragraph::new(lines)
            .scroll((app.log_scroll, 0))
            .block(panel_focused(&title, ACCENT)),
        area,
    );
    scrollbar(f, area, app.log_scroll, total);
}

fn draw_confirm(f: &mut Frame, app: &App) {
    let area = centered(50, 20, f.area());
    f.render_widget(Clear, area);
    let what = if app.marked_total() > 0 {
        format!("{} marked agent(s)", app.marked_total())
    } else {
        format!("'{}'", truncate(app.selected_row().map(|r| r.display()).unwrap_or(""), 34))
    };
    let lines = vec![
        Line::from(""),
        Line::from(Span::raw(format!("  Stop {}?", what))),
        Line::from(Span::styled("  it stays ▷ resumable", Style::default().fg(Color::DarkGray))),
        Line::from(""),
        Line::from(Span::styled("  [y] confirm   [n/esc] cancel", Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines)
        .block(dialog(Span::styled(" confirm stop ", Style::default().fg(Color::Red)))), area);
}

fn draw_confirm_stop_all(f: &mut Frame, app: &App) {
    let area = centered(52, 20, f.area());
    f.render_widget(Clear, area);
    let n = app.visible_rows().iter().filter(|r| r.is_live()).count();
    let lines = vec![
        Line::from(""),
        Line::from(Span::raw(format!("  Stop ALL {} running agent(s)?", n))),
        Line::from(""),
        Line::from(Span::styled("  [y] confirm   [n/esc] cancel", Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines)
        .block(dialog(Span::styled(" confirm stop all ", Style::default().fg(Color::Red)))), area);
}

fn draw_confirm_quit(f: &mut Frame, app: &App) {
    let area = centered(58, 26, f.area());
    f.render_widget(Clear, area);
    let n: usize = app.workspaces.iter()
        .flat_map(|w| w.agents.iter()).filter(|p| p.tmux.is_none()).count();
    let dirty = app.dirty.values().filter(|d| **d).count();
    let head = if n > 0 { format!("  Quit? {} agent(s) will stop (pty backend).", n) }
        else { "  Quit crew? Agents keep running in tmux.".to_string() };
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(head, Style::default().add_modifier(Modifier::BOLD))),
    ];
    if dirty > 0 {
        lines.push(Line::from(Span::styled(
            format!("  ⚠ {} worktree(s) have uncommitted changes (± in the list).", dirty),
            Style::default().fg(Color::Yellow))));
    }
    if n > 0 {
        lines.extend([
            Line::from(""),
            Line::from(Span::styled("  Worktrees are kept; next time, [enter] on a ▷", Style::default().fg(Color::Gray))),
            Line::from(Span::styled("  agent resumes its conversation. Install tmux to", Style::default().fg(Color::Gray))),
            Line::from(Span::styled("  keep agents running after quit ([,] settings).", Style::default().fg(Color::Gray))),
        ]);
    }
    lines.extend([
        Line::from(""),
        Line::from(Span::styled("  [y] quit   [n/esc] cancel", Style::default().fg(Color::DarkGray))),
    ]);
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false })
        .block(dialog(Span::styled(" confirm quit ", Style::default().fg(Color::Yellow)))), area);
}

fn draw_confirm_discard(f: &mut Frame) {
    let area = centered(50, 20, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from(""),
        Line::from(Span::raw("  Discard this new agent?")),
        Line::from(""),
        Line::from(Span::styled("  [y] discard   [n/esc] keep editing", Style::default().fg(Color::DarkGray))),
    ];
    f.render_widget(Paragraph::new(lines)
        .block(dialog(Span::styled(" confirm discard ", Style::default().fg(Color::Yellow)))), area);
}

fn centered(pw: u16, ph: u16, area: Rect) -> Rect {
    let w = area.width * pw / 100;
    let h = area.height * ph / 100;
    Rect {
        x: area.x + (area.width.saturating_sub(w)) / 2,
        y: area.y + (area.height.saturating_sub(h)) / 2,
        width: w.max(24),
        height: h.max(10),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(n.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::App;
    use crate::store::Store;
    use ratatui::{backend::TestBackend, Terminal};
    use std::path::PathBuf;

    fn buffer_text(term: &Terminal<TestBackend>) -> String {
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn meter_fills_proportionally() {
        assert_eq!(meter(0.0, 10), "░░░░░░░░░░");
        assert_eq!(meter(100.0, 10), "██████████");
        assert_eq!(meter(50.0, 10), "█████░░░░░");
        assert_eq!(meter(200.0, 4), "████"); // clamps over 100
    }

    #[test]
    fn renders_empty_dashboard_chrome() {
        let store = Store::new(PathBuf::from("/tmp/myrepo"), PathBuf::from("/tmp/myrepo/scripts"));
        let app = App::new(store);
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(&term);
        assert!(text.contains("crew"));
        assert!(text.contains("myrepo"));
        assert!(text.contains("agents ("));
        assert!(text.contains("all"), "all-repos row missing");
        assert!(text.contains("[n]ew"));
    }

    #[test]
    fn renders_populated_list_with_markers() {
        use crate::agent::{ManagedAgent, Row};
        let mk = |id: &str, h: &str, st: &str, cost: f64| Row::from_managed(&ManagedAgent {
            agent_id: id.into(), harness: Some(h.into()), model: Some("sonnet".into()),
            worktree: Some(format!("/tmp/{id}")), branch: Some(format!("crew/{id}")),
            status: Some(st.into()), cost_usd: Some(cost), task: Some(format!("task {id}")),
            step_index: None, updated_ts: Some(1.0), pid: None, log_path: None,
        });
        let store = Store::new(PathBuf::from("/tmp/myrepo"), PathBuf::from("/tmp/myrepo/scripts"));
        let mut app = App::new(store);
        app.rows = vec![mk("a", "cursor", "running", 0.42), mk("b", "claude", "failed", 0.08)];
        app.marked.insert("a".into());
        app.dirty.insert("b".into(), true);
        let mut term = Terminal::new(TestBackend::new(120, 24)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(&term);
        assert!(text.contains("cursor") && text.contains("claude"));
        assert!(text.contains("$0.42"), "cost missing");
        assert!(text.contains('◉'), "mark missing");
        assert!(text.contains('±'), "dirty marker missing");
        assert!(text.contains("✗1") || text.contains('✗'), "failed legend missing");
    }

    #[test]
    fn renders_new_agent_form() {
        let store = Store::new(PathBuf::from("/tmp/myrepo"), PathBuf::from("/tmp/myrepo/scripts"));
        let mut app = App::new(store);
        app.open_new_agent();
        app.new_form.task.insert_str("add billing endpoint");
        let mut term = Terminal::new(TestBackend::new(120, 30)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        let text = buffer_text(&term);
        assert!(text.contains("new agent"));
        assert!(text.contains("task *"));
        assert!(text.contains("repo") && text.contains("myrepo"));
        assert!(text.contains("harness"));
        assert!(text.contains("options"), "advanced options should be collapsed");
        assert!(text.contains("[enter] launch"));
    }
}
