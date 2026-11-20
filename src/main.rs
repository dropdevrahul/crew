//! crew — a repo-scoped dashboard for running, chatting with, and managing
//! coding agents across harnesses. Each agent runs its harness REPL in a PTY
//! rendered *inside* the TUI. Standalone: creates worktrees natively and keeps
//! its own registry; an engine adds observed sessions + delegation.
//!
//!   crew                 # interactive dashboard
//!   crew --dump          # print the agent list once and exit (no TTY)
//!   crew --snapshot      # render one real frame to text (no TTY)
//!   crew --scripts DIR   # override the optional engine scripts dir

mod agent;
mod app;
mod leader;
mod pty;
mod theme;
mod store;
mod ui;
mod workspace;

use anyhow::Result;
use app::{App, Mode};
use crossterm::{
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyboardEnhancementFlags,
        KeyModifiers, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags, MouseButton, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

type Term = Terminal<CrosstermBackend<io::Stdout>>;

const HELP: &str = "\
crew — a terminal dashboard for coding agents across harnesses

USAGE:
  crew                         interactive dashboard (from inside a git repo)
  crew --new \"task\"            create a worktree + run the harness headless in it
       [--harness claude|opencode|cursor] [--model NAME] [--here]
       (--here runs in the current repo instead of a new worktree)
  crew --dump                  print the agent list once and exit (no TTY)
  crew --snapshot              render one frame to text (no TTY)
  crew --scripts DIR           engine scripts dir
  crew --doctor                check git / harness CLIs / gh / engine
  crew --help | --version

KEYS: press ? inside the dashboard for the full keymap.";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!("{HELP}");
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("crew {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if args.iter().any(|a| a == "--doctor") {
        return doctor(&args);
    }
    let scripts_override = arg_value(&args, "--scripts").map(PathBuf::from);

    // Workspaces are pinned in a global config; first run pins the cwd. Any
    // folder can be pinned — a git repo gets worktree isolation, a plain folder
    // still gets agent management.
    let cfg = workspace::load_global();
    // where agents live: CREW_BACKEND overrides the saved setting (auto|tmux|pty)
    let be = std::env::var("CREW_BACKEND").unwrap_or_else(|_| cfg.backend.clone());
    pty::set_backend(pty::resolve_backend(&be));
    theme::set(&app::theme_setting(&cfg));
    let mut app = if cfg.workspaces.is_empty() {
        let cwd = std::env::current_dir()?;
        let a = App::new(workspace::store_for_path(&cwd, scripts_override.as_deref()));
        a.save_global();
        a
    } else {
        App::from_config(&cfg, scripts_override.as_deref())
    };
    // the launch folder becomes the default repo for new agents (pinned if
    // new) — but crew always opens on "all": you see the whole fleet first
    if app.adopt_cwd(&std::env::current_dir()?, scripts_override.as_deref()) {
        app.save_global();
    }
    for w in &mut app.workspaces {
        let _ = w.refresh();
    }

    if args.iter().any(|a| a == "--dump") {
        dump_table(&app);
        return Ok(());
    }
    if let Some(task) = arg_value(&args, "--new") {
        return run_new(&app, &task, &args);
    }
    if args.iter().any(|a| a == "--snapshot") {
        match std::env::var("CREW_SNAPSHOT_MODE").as_deref() {
            Ok("new") => {
                app.open_new_agent();
                app.new_form.task.insert_str("add auth middleware");
            }
            Ok("help") => app.mode = app::Mode::Help,
            Ok("demo") => {
                // inject a rich sample fleet for docs/marketing screenshots
                use agent::{ManagedAgent, Row};
                let mk = |id: &str, h: &str, st: &str, task: &str, cost: f64, ago: f64| {
                    Row::from_managed(&ManagedAgent {
                        agent_id: id.into(), harness: Some(h.into()), model: Some("sonnet".into()),
                        worktree: Some(format!(".crew/worktrees/{}", id)), branch: Some(format!("crew/{}", id)),
                        status: Some(st.into()), cost_usd: Some(cost), task: Some(task.into()),
                        step_index: None, updated_ts: Some(app_now() - ago), pid: None, log_path: None,
                    })
                };
                app.all_rows = vec![
                    mk("billing-api", "cursor", "running", "add billing dashboard endpoint", 0.42, 12.0),
                    mk("auth-mw", "claude", "running", "add auth middleware + tests", 0.31, 40.0),
                    mk("flaky-test", "opencode", "failed", "fix flaky timezone test", 0.08, 120.0),
                    mk("readme", "claude", "done", "rewrite the README intro", 0.05, 600.0),
                    mk("dark-mode", "cursor", "done", "add dark-mode toggle to settings", 0.19, 3600.0),
                ];
                app.apply_filter();
                app.selected = 0;
                app.marked.insert("auth-mw".into());
                app.marked.insert("flaky-test".into());
            }
            Ok("term") | Ok("quit") => {
                // spawn a stub interactive agent and drop into its terminal
                app.term_size = (80, 24);
                app.open_new_agent();
                app.new_form.task.insert_str("say hello");
                app.new_form.worktree_new = false; // run in cwd, skip worktree.py
                app.spawn_from_form();
                std::thread::sleep(Duration::from_millis(600));
                let _ = app.refresh();
                if std::env::var("CREW_SNAPSHOT_MODE").as_deref() == Ok("quit") {
                    app.mode = app::Mode::ConfirmQuit;
                }
            }
            _ => {}
        }
        snapshot(&app);
        return Ok(());
    }

    run_tui(&mut app)
}

/// Headless launch from the shell: create a worktree natively and run the
/// harness once in it (inheriting the terminal), then print the worktree/branch.
/// Fully standalone — no engine required.
fn run_new(app: &App, task: &str, args: &[String]) -> Result<()> {
    let harness = arg_value(args, "--harness").unwrap_or_else(|| "claude".into());
    let model = arg_value(args, "--model").unwrap_or_else(|| default_model(&harness).into());
    let slug = app::slugify(task);
    // --here runs in the current repo; default is a fresh isolated worktree.
    let here = args.iter().any(|a| a == "--here");
    let (wt, where_) = if here {
        let repo = app.store.repo_root.to_string_lossy().to_string();
        (repo.clone(), format!("repo {repo}"))
    } else {
        match app.store.create_worktree(&slug) {
            Ok(p) => (p.clone(), format!("branch crew/{slug} · worktree {p}")),
            Err(e) => {
                eprintln!("crew: worktree failed: {e}");
                std::process::exit(1);
            }
        }
    };
    let argv = headless_argv(&harness, &model, task);
    println!("crew: launching {harness} ({model}) in {wt}");
    let status = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&wt)
        .status();
    match status {
        Ok(s) => println!("crew: done ({}) · {where_}",
            s.code().map(|c| c.to_string()).unwrap_or_else(|| "signal".into())),
        Err(e) => {
            eprintln!("crew: could not run {}: {e}", argv[0]);
            std::process::exit(1);
        }
    }
    Ok(())
}

/// Print an environment checklist so setup problems are obvious.
fn doctor(args: &[String]) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let scripts_dir = arg_value(args, "--scripts")
        .map(PathBuf::from)
        .or_else(|| std::env::var("CREW_SCRIPTS").ok().map(PathBuf::from))
        .unwrap_or_else(|| store::git_toplevel(&cwd).join("scripts"));
    let mark = |ok: bool| if ok { "✓" } else { "✗" };

    println!("crew {} — environment check\n", env!("CARGO_PKG_VERSION"));
    println!("  {} git installed", mark(app::on_path("git")));
    println!("  {} inside a git repository", mark(store::is_git_repo(&cwd)));
    println!("  {} gh (GitHub CLI, for push→PR / open)", mark(app::gh_available()));
    let setting = std::env::var("CREW_BACKEND").unwrap_or_else(|_| workspace::load_global().backend);
    let be = pty::resolve_backend(&setting);
    println!("  {} tmux — agent backend: {} (setting: {})", mark(app::on_path("tmux")),
        if be == pty::Backend::Tmux { "tmux, agents survive quitting crew" } else { "pty, agents stop when crew exits" },
        if setting.is_empty() { "auto" } else { &setting });
    println!("\n  harness CLIs:");
    for h in app::HARNESSES {
        println!("    {} {:<9} ({})", mark(app::harness_installed(h)), h, app::harness_bin(h));
    }
    let engine = scripts_dir.join("store.py").is_file();
    println!("\n  {} engine ({})",
        mark(engine), scripts_dir.display());
    if !engine {
        println!("      (standalone mode — observed sessions & delegation disabled)");
    }
    Ok(())
}

fn default_model(harness: &str) -> &'static str {
    match harness {
        "opencode" => "anthropic/claude-sonnet-4-6",
        "cursor" => "auto",
        _ => "sonnet",
    }
}

/// One-shot headless argv per harness (mirrors the interactive presets minus the
/// REPL). Overridable via `CREW_HEADLESS_CMD_<HARNESS>` (prompt appended).
fn headless_argv(harness: &str, model: &str, task: &str) -> Vec<String> {
    if let Ok(over) = std::env::var(format!("CREW_HEADLESS_CMD_{}", harness.to_uppercase())) {
        if !over.trim().is_empty() {
            let mut v: Vec<String> = over.split_whitespace().map(String::from).collect();
            v.push(task.into());
            return v;
        }
    }
    match harness {
        "opencode" => vec!["opencode".into(), "run".into(), task.into(), "--model".into(), model.into()],
        "cursor" => vec!["cursor-agent".into(), "-p".into(), task.into(), "--model".into(), model.into()],
        _ => vec!["claude".into(), "-p".into(), task.into(), "--model".into(), model.into()],
    }
}

fn dump_table(app: &App) {
    use std::io::Write;
    let mut out = io::stdout().lock();
    let rows = app.visible_rows();
    let _ = writeln!(out, "crew agents across {} workspace(s) — {} rows", app.workspaces.len(), rows.len());
    for r in &rows {
        let kind = if r.interactive { "agent" } else if r.kind == agent::Kind::Observed { "session" } else { "agent" };
        let repo = app.workspaces[r.ws].name();
        // a closed pipe (`| head`) just ends the listing — never panic
        if writeln!(out, "{:<7} {:<12} {:<9} {:<9} {}", kind, repo, r.harness, r.status_label(), r.task).is_err() {
            return;
        }
    }
}

fn snapshot(app: &App) {
    use ratatui::{backend::TestBackend, Terminal};
    let mut term = Terminal::new(TestBackend::new(100, 30)).unwrap();
    term.draw(|f| ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    use std::io::Write;
    let mut out = io::stdout().lock();
    for y in 0..buf.area.height {
        let mut line = String::new();
        for x in 0..buf.area.width {
            line.push_str(buf[(x, y)].symbol());
        }
        if writeln!(out, "{}", line.trim_end()).is_err() {
            return; // reader went away (`| head`)
        }
    }
}

static KBD_ENHANCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Ask the terminal to report modified Enter (kitty keyboard protocol), so
/// Shift+Enter reaches us as Enter+SHIFT instead of a bare Enter. Terminals
/// without support (Terminal.app) keep the old behaviour; Alt+Enter still works.
fn push_kbd() {
    use std::sync::atomic::Ordering::Relaxed;
    if crossterm::terminal::supports_keyboard_enhancement().unwrap_or(false) {
        let _ = execute!(io::stdout(), PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES));
        KBD_ENHANCED.store(true, Relaxed);
    }
}
fn pop_kbd() {
    use std::sync::atomic::Ordering::Relaxed;
    if KBD_ENHANCED.swap(false, Relaxed) {
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    }
}

fn run_tui(app: &mut App) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    push_kbd();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let res = event_loop(app, &mut terminal);

    pop_kbd();
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture, DisableBracketedPaste)?;
    terminal.show_cursor()?;
    res
}

fn event_loop(app: &mut App, terminal: &mut Term) -> Result<()> {
    // fast tick so the embedded terminal feels live; refresh list less often.
    let refresh_every = Duration::from_millis(800);
    let mut last_refresh = Instant::now();
    // gh is a network call — poll PRs on a slow timer, first pass ~immediately.
    let pr_every = Duration::from_secs(20);
    let mut last_pr = Instant::now()
        .checked_sub(pr_every)
        .unwrap_or_else(Instant::now);
    // per-row git status is heavier than the list refresh — run it less often.
    let dirty_every = Duration::from_millis(2500);
    let mut last_dirty = Instant::now()
        .checked_sub(dirty_every)
        .unwrap_or_else(Instant::now);

    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(60))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    if app.mode == Mode::Terminal {
                        handle_terminal_key(app, key);
                    } else {
                        if !app.status_msg.is_empty() {
                            app.status_msg.clear();
                        }
                        handle_key(app, key);
                    }
                }
                Event::Paste(text) => handle_paste(app, &text),
                Event::Mouse(m) if app.mode == Mode::Terminal => {
                    // wheel over the chat scrolls its history; other mouse
                    // input stays out of the agent's way
                    let w = terminal.size().map(|s| s.width).unwrap_or(80);
                    chat_select(app, m, w);
                    match m.kind {
                        MouseEventKind::ScrollUp => wheel_chat(app, true, m.column, m.row, w),
                        MouseEventKind::ScrollDown => wheel_chat(app, false, m.column, m.row, w),
                        _ => {}
                    }
                }
                Event::Mouse(m) if app.mode == Mode::Normal => {
                    let w = terminal.size().map(|s| s.width).unwrap_or(80);
                    // zoomed: the detail pane owns the whole width
                    let tree_w = if app.zoom { 0 } else { ui::pane_widths(app, w).0 };
                    if app.agent_selected() {
                        chat_select(app, m, w);
                    }
                    match m.kind {
                        MouseEventKind::ScrollDown => {
                            if m.column < tree_w { app.tree_move(1); }
                            else { wheel_chat(app, false, m.column, m.row, w); }
                        }
                        MouseEventKind::ScrollUp => {
                            if m.column < tree_w { app.tree_move(-1); }
                            else { wheel_chat(app, true, m.column, m.row, w); }
                        }
                        MouseEventKind::Down(MouseButton::Left) => {
                            if m.column >= tree_w {
                                // click the chat pane → chat with that agent
                                if app.agent_selected() && app.selected_row().map(|r| r.is_live()).unwrap_or(false) {
                                    app.mode = Mode::Terminal;
                                }
                            } else if m.row >= 2 {
                                // items start at y=2 (header row + panel top border);
                                // ratatui scrolls the list just enough to show the
                                // selection, so add that offset back.
                                let h = terminal.size().map(|s| s.height).unwrap_or(24);
                                let visible = h.saturating_sub(4).max(1) as usize;
                                let tree = app.tree();
                                let pos = tree.iter().position(|x| *x == app.tree_cursor()).unwrap_or(0);
                                let idx = (m.row - 2) as usize + pos.saturating_sub(visible - 1);
                                if let Some(it) = tree.get(idx) {
                                    app.set_tree(*it);
                                    let _ = app.active_mut().refresh();
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }

        // an editor/shell was requested: drop out of the TUI, run it, come back
        if let Some((cwd, argv)) = app.pending_shell.take() {
            run_suspended(terminal, &cwd, &argv)?;
        }

        if last_refresh.elapsed() >= refresh_every {
            if app.mode == Mode::Log {
                app.tick_log(); // live-follow the open log (tail -f)
            } else {
                app.tick();
            }
            if let Some(msg) = app.take_alert() {
                notify(terminal, &msg);
            }
            last_refresh = Instant::now();
        }

        if last_pr.elapsed() >= pr_every && app.mode != Mode::Terminal {
            app.poll_prs();
            last_pr = Instant::now();
        }

        if last_dirty.elapsed() >= dirty_every && app.mode != Mode::Terminal {
            app.compute_dirty();
            last_dirty = Instant::now();
        }

        if app.should_quit {
            app.save_ui_prefs(); // remember view toggles for next run
            app.detach_all(); // tmux agents keep running; pty agents are stopped (resumable)
            return Ok(());
        }
    }
}

/// Suspend the TUI (leave raw mode + alternate screen), run `argv` in `cwd`
/// attached to the real terminal, then restore the dashboard. Used for `e`
/// (editor) and `!` (shell) — the child fully owns the terminal while it runs.
fn run_suspended(terminal: &mut Term, cwd: &str, argv: &[String]) -> Result<()> {
    if argv.is_empty() {
        return Ok(());
    }
    pop_kbd();
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture, DisableBracketedPaste)?;
    let _ = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(cwd)
        .status();
    enable_raw_mode()?;
    execute!(terminal.backend_mut(), EnterAlternateScreen, EnableMouseCapture, EnableBracketedPaste)?;
    push_kbd();
    terminal.clear()?;
    Ok(())
}

/// In Terminal mode every key is forwarded to the agent's PTY, except the
/// detach key, Ctrl-q (Ctrl-o belongs to Claude Code's transcript toggle).
fn handle_terminal_key(app: &mut App, key: KeyEvent) {
    if app.leader_pending || leader::key_matches(&app.leader.chat_key, &key) {
        return leader_key(app, key);
    }
    if leader::key_matches("ctrl-q", &key) {
        return run_action(app, leader::Action::Back); // legacy leave key
    }
    // Shift-PgUp/PgDn page through the chat's history (not sent to the agent)
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        let page = app.term_size.1.max(4) as i64 - 2;
        match key.code {
            KeyCode::PageUp => return scroll_chat(app, page),
            KeyCode::PageDown => return scroll_chat(app, -page),
            _ => {}
        }
    }
    let bytes = key_to_bytes(key);
    if !bytes.is_empty() {
        app.send_to_terminal(&bytes);
    }
}

/// Vim scroll keys shared by every list/viewer: Some(delta) for j/k, ctrl-d/u
/// (half page), ctrl-f/b + PgDn/PgUp (page), g/G (ends). `page` = rows visible.
fn vim_delta(key: &KeyEvent, page: i64) -> Option<i64> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    Some(match key.code {
        KeyCode::Char('d') if ctrl => page / 2,
        KeyCode::Char('u') if ctrl => -page / 2,
        KeyCode::Char('f') if ctrl => page,
        KeyCode::Char('b') if ctrl => -page,
        KeyCode::Char('e') if ctrl => 1,
        KeyCode::Char('y') if ctrl => -1,
        _ if ctrl => return None,
        KeyCode::Char('j') | KeyCode::Down => 1,
        KeyCode::Char('k') | KeyCode::Up => -1,
        KeyCode::PageDown => page,
        KeyCode::PageUp => -page,
        KeyCode::Char('g') | KeyCode::Home => i64::MIN / 2,
        KeyCode::Char('G') | KeyCode::End => i64::MAX / 2,
        _ => return None,
    })
}

/// Leader flow: the first key arms `leader_pending`, the next one runs its
/// bound action (Esc / unknown keys cancel). Swallows keys while pending.
fn leader_key(app: &mut App, key: KeyEvent) {
    if !app.leader_pending {
        app.leader_pending = true;
        return;
    }
    app.leader_pending = false;
    match app.leader.lookup(&key) {
        Some(action) => run_action(app, action),
        None if key.code == KeyCode::Esc => app.status_msg = "leader cancelled".into(),
        None => app.status_msg = format!("no leader binding for '{}'", leader::key_label(&key)),
    }
}

fn run_action(app: &mut App, action: leader::Action) {
    use leader::Action::*;
    match action {
        Back if app.mode == Mode::Terminal => app.mode = Mode::Normal,
        Back => request_quit(app),
        Agents => app.open_picker(),
        NewAgent => app.open_new_agent(),
        Zoom => app.zoom = !app.zoom,
        Help => { app.help_scroll = 0; app.mode = Mode::Help; }
        Settings => app.open_settings(),
        _ if !app.agent_selected() => app.status_msg = "select an agent first".into(),
        Diff => app.show_diff(),
        Push => app.open_push(),
        Stop => {
            let has_live = app.marked_total() > 0
                || app.selected_row().map(|r| r.interactive).unwrap_or(false);
            if has_live {
                app.mode = Mode::ConfirmStop;
            } else {
                app.status_msg = "only interactive agents can be stopped".into();
            }
        }
    }
}

fn request_quit(app: &mut App) {
    // always confirm; the dialog says whether agents stop or keep running (tmux)
    app.mode = Mode::ConfirmQuit;
}

/// Scroll the selected agent's chat history (>0 = older).
fn scroll_chat(app: &mut App, lines: i64) {
    if let Some(p) = app.selected_proc() {
        p.scroll(lines);
    }
}

/// Mouse wheel at screen (col,row) over the chat pane → the agent, translated
/// to pane-relative cells (pane interior starts below header + border).
fn wheel_chat(app: &mut App, up: bool, col: u16, row: u16, width: u16) {
    let x0 = if app.zoom { 1 } else { ui::pane_widths(app, width).0 + 1 };
    if let Some(p) = app.selected_proc() {
        p.wheel(up, col.saturating_sub(x0), row.saturating_sub(2));
    }
}

/// Left-drag over the chat pane selects text (never forwarded to the agent);
/// releasing a non-empty selection copies it. Down starts, Drag extends, Up
/// finishes; a plain click leaves nothing behind.
fn chat_select(app: &mut App, m: crossterm::event::MouseEvent, width: u16) {
    let x0 = if app.zoom { 1 } else { ui::pane_widths(app, width).0 + 1 };
    let Some((rows, cols)) = app.selected_proc()
        .and_then(|p| p.parser.lock().ok().map(|p| p.screen().size())) else { return };
    let inside = m.column >= x0 && m.row >= 2 && m.column < x0 + cols && m.row < 2 + rows;
    let cell = (
        m.row.saturating_sub(2).min(rows.saturating_sub(1)),
        m.column.saturating_sub(x0).min(cols.saturating_sub(1)),
    );
    match m.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            app.selection = inside.then_some((cell, cell));
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if let Some((s, _)) = app.selection {
                app.selection = Some((s, cell));
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            if let Some((s, e)) = app.selection.take() {
                if s != e {
                    let text = app.selected_proc().map(|p| p.selection_text(s, e)).unwrap_or_default();
                    app.yank_text(&text, "selection");
                    app.status_msg = format!("copied {} chars", text.chars().count());
                }
            }
        }
        _ => {}
    }
}

/// Ring the bell and post a desktop notification (OSC 9: iTerm2, kitty,
/// WezTerm, Ghostty; ignored elsewhere) — so a waiting/finished agent reaches
/// you even when crew isn't the window you're looking at.
fn notify(terminal: &mut Term, msg: &str) {
    use std::io::Write;
    let clean: String = msg.chars().filter(|c| !c.is_control()).collect();
    let _ = write!(terminal.backend_mut(), "\x1b]9;crew: {}\x07\x07", clean);
    let _ = terminal.backend_mut().flush();
}

/// Bracketed paste arrives as one string. Route it as a unit so newlines never
/// act as Enter: into the agent (wrapped for it), the task editor, or the
/// one-line inputs (first line only). Pasting on the dashboard starts a new
/// agent with the text as its task.
fn handle_paste(app: &mut App, text: &str) {
    let first = text.lines().next().unwrap_or("").to_string();
    match app.mode {
        Mode::Terminal => {
            if let Some(p) = app.selected_proc() {
                p.paste(text);
            }
        }
        Mode::NewAgent => {
            if let Some(buf) = app.new_form.image_input.as_mut() {
                buf.push_str(first.trim());
            } else {
                app.new_form.field = app::FIELD_TASK;
                app.new_form.task.insert_str(text);
            }
        }
        Mode::Normal => {
            app.open_new_agent();
            app.new_form.task.insert_str(text);
        }
        Mode::Filter => { app.filter_text.push_str(&first); app.apply_filter(); }
        Mode::Rename => app.rename_buf.push_str(&first),
        Mode::BrowseFolder => {
            app.browser.editing = true;
            app.browser.typed = first.trim().to_string();
        }
        Mode::Diff | Mode::Log if app.find_input => app.find_query.push_str(&first),
        _ => {}
    }
}

fn handle_key(app: &mut App, key: KeyEvent) {
    let code = key.code;
    let page = app.term_size.1.max(4) as i64;
    match app.mode {
        Mode::Normal => {
            if app.leader_pending || leader::key_matches(&app.leader.key, &key) {
                return leader_key(app, key);
            }
            // One tree: j/k walk repos and their agents. Agent keys only act on
            // an agent line, so a repo line can never trigger one by accident.
            if let Some(d) = vim_delta(&key, page) {
                match d {
                    d if d <= i64::MIN / 2 => app.tree_move(i64::MIN / 2),
                    d if d >= i64::MAX / 2 => app.tree_move(i64::MAX / 2),
                    d => app.tree_move(d),
                }
                return;
            }
            if code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
                app.open_picker();
                return;
            }
            if !app.agent_selected() {
                match code {
                    KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => { app.tree_enter_repo(); return; }
                    KeyCode::Char('x') => { app.remove_workspace_at_selected(); return; }
                    KeyCode::Char('q') | KeyCode::Char('w') | KeyCode::Char('n') | KeyCode::Char('a')
                    | KeyCode::Char('z') | KeyCode::Char('H') | KeyCode::Char('t') | KeyCode::Char(',')
                    | KeyCode::Char('/') | KeyCode::Char('?') | KeyCode::Char('1') | KeyCode::Char('2')
                    | KeyCode::Char('3') | KeyCode::Char('4') | KeyCode::Char('S') | KeyCode::Char('A')
                    | KeyCode::Esc | KeyCode::Char('[') | KeyCode::Char(']') => {}
                    _ => return, // not an agent line: nothing to act on
                }
            }
            match code {
            KeyCode::Char('h') | KeyCode::Left => app.tree_up_to_repo(),
            KeyCode::Char('q') => request_quit(app),
            KeyCode::Char(']') => app.jump_failed(1),
            KeyCode::Char('[') => app.jump_failed(-1),
            KeyCode::Char('y') => app.yank_worktree(),
            KeyCode::Char('w') => app.open_browser(),
            KeyCode::Char('a') => app.set_tree(app::TreeItem::All),
            KeyCode::Char('n') => app.open_new_agent(),
            KeyCode::Char('s') => {
                let has_live = app.marked_total() > 0
                    || app.selected_row().map(|r| r.interactive).unwrap_or(false);
                if has_live {
                    app.mode = Mode::ConfirmStop;
                } else {
                    app.status_msg = "only interactive agents can be stopped".into();
                }
            }
            KeyCode::Char(' ') => app.toggle_mark(),
            KeyCode::Char('A') => app.mark_all_visible(),
            KeyCode::Esc => {
                // peel one layer per press: marks → search → category
                if app.marked_total() > 0 {
                    app.clear_marks();
                } else if !app.filter_text.is_empty() {
                    app.filter_text.clear();
                    app.apply_filter();
                } else if app.category != app::Category::All {
                    app.set_category(app::Category::All);
                }
            }
            KeyCode::Char('x') => {
                if app.selected_row().is_some() || app.marked_total() > 0 {
                    app.mode = Mode::ConfirmRemove;
                }
            }
            KeyCode::Char('r') => app.open_rename(),
            KeyCode::Char('D') => app.duplicate_selected(),
            KeyCode::Char('S') => {
                if app.visible_rows().iter().any(|r| r.is_live()) {
                    app.mode = Mode::ConfirmStopAll;
                } else {
                    app.status_msg = "no running agents".into();
                }
            }
            KeyCode::Char('c') => app.mode = Mode::ConfirmClear,
            KeyCode::Char('R') => app.open_retry(),
            KeyCode::Char('P') => app.open_push(),
            KeyCode::Char('o') => app.show_worktree(),
            KeyCode::Char('Y') => app.yank_branch(),
            KeyCode::Char('O') => app.open_branch_web(),
            KeyCode::Char('f') => app.fetch_selected(),
            KeyCode::Char('e') => app.open_editor(),
            KeyCode::Char('!') => app.open_shell(),
            KeyCode::Char('d') => app.show_diff(),
            KeyCode::Char('L') => app.show_log(),
            KeyCode::Char('H') => { let on = !app.show_observed; app.set_show_observed(on); }
            KeyCode::Char('z') => app.zoom = !app.zoom,
            KeyCode::Char('1') => app.set_category(app::Category::All),
            KeyCode::Char('2') => app.set_category(app::Category::Running),
            KeyCode::Char('3') => app.set_category(app::Category::Agents),
            KeyCode::Char('4') => app.set_category(app::Category::Sessions),
            KeyCode::Char('t') => app.cycle_sort(),
            KeyCode::Char(',') => app.open_settings(),
            KeyCode::Char('/') => app.mode = Mode::Filter,
            KeyCode::Char('?') => { app.help_scroll = 0; app.mode = Mode::Help; }
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => app.open_selected(),
            _ => {}
            }
        }
        Mode::Help => match vim_delta(&key, page) {
            Some(d) => app.help_scroll = (app.help_scroll as i64 + d).clamp(0, 200) as u16,
            None => app.mode = Mode::Normal,
        },
        Mode::Diff | Mode::Log => handle_viewer_key(app, key),
        Mode::Retry => match code {
            KeyCode::Left | KeyCode::Char('h') => app.cycle_retry_harness(-1),
            KeyCode::Right | KeyCode::Char('l') => app.cycle_retry_harness(1),
            KeyCode::Enter => app.confirm_retry(),
            KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
            _ => {}
        },
        Mode::ConfirmQuit => match code {
            KeyCode::Char('y') | KeyCode::Enter => app.should_quit = true,
            KeyCode::Char('n') | KeyCode::Esc => app.mode = Mode::Normal,
            _ => {}
        },
        Mode::ConfirmStop => match code {
            KeyCode::Char('y') => { app.stop_selected(); app.mode = Mode::Normal; }
            KeyCode::Char('n') | KeyCode::Esc => app.mode = Mode::Normal,
            _ => {}
        },
        Mode::ConfirmStopAll => match code {
            KeyCode::Char('y') => { app.stop_all(); app.mode = Mode::Normal; }
            KeyCode::Char('n') | KeyCode::Esc => app.mode = Mode::Normal,
            _ => {}
        },
        Mode::ConfirmPush => match code {
            KeyCode::Char('y') | KeyCode::Enter => app.confirm_push(),
            KeyCode::Char('n') | KeyCode::Esc => app.cancel_push(),
            _ => {}
        },
        Mode::Settings => match code {
            KeyCode::Left | KeyCode::Char('h') => app.settings_change(-1),
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => app.settings_change(1),
            KeyCode::Tab | KeyCode::Down | KeyCode::Char('j') => app.settings_move(1),
            KeyCode::BackTab | KeyCode::Up | KeyCode::Char('k') => app.settings_move(-1),
            KeyCode::Enter | KeyCode::Char('s') => app.save_settings(),
            KeyCode::Esc | KeyCode::Char('q') => app.cancel_settings(),
            _ => {}
        },
        Mode::ConfirmRemove => match code {
            KeyCode::Char('y') => { app.remove_selected(); app.mode = Mode::Normal; }
            KeyCode::Char('n') | KeyCode::Esc => app.mode = Mode::Normal,
            _ => {}
        },
        Mode::ConfirmClear => match code {
            KeyCode::Char('y') => { app.clear_finished(); app.mode = Mode::Normal; }
            KeyCode::Char('n') | KeyCode::Esc => app.mode = Mode::Normal,
            _ => {}
        },
        Mode::Rename => match code {
            KeyCode::Enter => app.commit_rename(),
            KeyCode::Esc => app.mode = Mode::Normal,
            KeyCode::Backspace => { app.rename_buf.pop(); }
            KeyCode::Char(c) => app.rename_buf.push(c),
            _ => {}
        },
        Mode::Picker => {
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let last = app.picker_items().len().saturating_sub(1);
            match code {
                KeyCode::Esc => app.mode = app.picker.ret,
                KeyCode::Enter => app.picker_pick(),
                KeyCode::Down => app.picker.cursor = (app.picker.cursor + 1).min(last),
                KeyCode::Up => app.picker.cursor = app.picker.cursor.saturating_sub(1),
                KeyCode::Char('n') if ctrl => app.picker.cursor = (app.picker.cursor + 1).min(last),
                KeyCode::Char('p') if ctrl => app.picker.cursor = app.picker.cursor.saturating_sub(1),
                KeyCode::Backspace => { app.picker.query.pop(); app.picker.cursor = 0; }
                KeyCode::Char(c) if !ctrl => { app.picker.query.push(c); app.picker.cursor = 0; }
                _ => {}
            }
        }
        Mode::ConfirmDiscard => match code {
            KeyCode::Char('y') => { app.new_form = app::NewAgentForm::default(); app.mode = Mode::Normal; }
            KeyCode::Char('n') | KeyCode::Esc => app.mode = Mode::NewAgent,
            _ => {}
        },
        Mode::Filter => match code {
            KeyCode::Esc => { app.filter_text.clear(); app.apply_filter(); app.mode = Mode::Normal; }
            KeyCode::Enter => app.mode = Mode::Normal,
            KeyCode::Backspace => { app.filter_text.pop(); app.apply_filter(); }
            KeyCode::Char(c) => { app.filter_text.push(c); app.apply_filter(); }
            _ => {}
        },
        Mode::NewAgent => handle_new_agent_key(app, key),
        Mode::BrowseFolder => handle_browser_key(app, key),
        Mode::Terminal => {}
    }
}

/// Keys for the folder browser (`Mode::BrowseFolder`). While editing a typed
/// path every printable key goes to the buffer; otherwise j/k navigate and
/// `a` pins.
fn handle_browser_key(app: &mut App, key: KeyEvent) {
    if app.browser.editing {
        match key.code {
            KeyCode::Enter => app.browser_enter(),
            KeyCode::Esc => app.browser.editing = false,
            KeyCode::Backspace => { app.browser.typed.pop(); }
            KeyCode::Char(c) => app.browser.typed.push(c),
            _ => {}
        }
        return;
    }
    if let Some(d) = vim_delta(&key, 10) {
        let n = app.browser.entries.len().max(1) as i64;
        let to = (app.browser.selected as i64 + d).clamp(0, n - 1);
        app.browser.selected = to as usize;
        return;
    }
    match key.code {
        KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => app.browser_enter(),
        KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => app.browser_up(),
        KeyCode::Char('a') => app.browser_add(),
        KeyCode::Char('/') => app.browser_start_edit(),
        KeyCode::Esc | KeyCode::Char('q') => app.browser_close(),
        _ => {}
    }
}

/// Shared key handling for the full-screen diff and log viewers: scrolling,
/// vim jumps, and an in-viewer `/` search (Enter/n/N to cycle matches).
fn handle_viewer_key(app: &mut App, key: KeyEvent) {
    let code = key.code;
    if app.find_input {
        match code {
            KeyCode::Enter => { app.find_input = false; app.find_next(1); }
            KeyCode::Esc => { app.find_input = false; app.find_query.clear(); app.status_msg.clear(); }
            KeyCode::Backspace => { app.find_query.pop(); }
            KeyCode::Char(c) => app.find_query.push(c),
            _ => {}
        }
        return;
    }
    if let Some(d) = vim_delta(&key, 20) {
        app.scroll_viewer(d);
        return;
    }
    match code {
        KeyCode::Char(' ') => app.scroll_viewer(20),
        KeyCode::Char('/') => { app.find_input = true; app.find_query.clear(); }
        KeyCode::Char('n') => app.find_next(1),
        KeyCode::Char('N') => app.find_next(-1),
        KeyCode::Char('r') if app.mode == Mode::Log => app.show_log(),
        KeyCode::Char('q') | KeyCode::Esc => {
            if app.mode == Mode::Log { app.log_path_open = None; }
            app.mode = Mode::Normal;
            app.find_query.clear();
            app.find_input = false;
        }
        _ => {}
    }
}

/// Next visible form field in direction `d` (the workflow picker is hidden
/// without the engine — its plan/build workflows need the engine's skills).
fn next_field(field: usize, d: i64, has_engine: bool) -> usize {
    use app::{FIELD_COUNT, FIELD_WORKFLOW};
    let n = FIELD_COUNT as i64;
    let mut f = field as i64;
    loop {
        f = (f + d).rem_euclid(n);
        if has_engine || f as usize != FIELD_WORKFLOW {
            return f as usize;
        }
    }
}

fn handle_new_agent_key(app: &mut App, key: KeyEvent) {
    use app::{FIELD_HARNESS, FIELD_IMAGES, FIELD_MODEL, FIELD_REPO, FIELD_TASK, FIELD_WORKFLOW, FIELD_WORKTREE};
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.intersects(KeyModifiers::ALT | KeyModifiers::SHIFT);
    let engine = app.store.has_engine();

    if app.new_form.image_input.is_none() {
        // Enter launches (chat-app convention); Alt/Shift-Enter or Ctrl-j add
        // a newline in the task. Ctrl-S still launches too.
        let newline = (key.code == KeyCode::Enter && alt) || (ctrl && key.code == KeyCode::Char('j'));
        if newline && app.new_form.field == FIELD_TASK {
            app.new_form.task.insert_newline();
            return;
        }
        if (key.code == KeyCode::Enter && !alt) || (ctrl && key.code == KeyCode::Char('s')) {
            app.spawn_from_form();
            return;
        }
    }
    match key.code {
        KeyCode::Esc => {
            // cancel an in-progress image path first; otherwise confirm before
            // throwing away a form the user has typed into.
            if app.new_form.image_input.is_some() {
                app.new_form.image_input = None;
            } else if app.new_form.task_text().trim().is_empty() && app.new_form.images.is_empty() {
                app.mode = Mode::Normal;
            } else {
                app.mode = Mode::ConfirmDiscard;
            }
            return;
        }
        KeyCode::Tab => { app.new_form.field = next_field(app.new_form.field, 1, engine); return; }
        KeyCode::BackTab => { app.new_form.field = next_field(app.new_form.field, -1, engine); return; }
        _ => {}
    }

    // image-path capture sub-mode
    if let Some(buf) = app.new_form.image_input.as_mut() {
        match key.code {
            KeyCode::Enter => {
                let path = buf.trim().to_string();
                let f = &mut app.new_form;
                if !path.is_empty() { f.images.push(path); }
                f.image_input = None;
            }
            KeyCode::Backspace => { buf.pop(); }
            KeyCode::Char(c) => buf.push(c),
            _ => {}
        }
        return;
    }

    // a printable key that isn't a picker key, typed on a picker field, was
    // meant for the task: jump back and type it there (h/l/j/k/space/a/d stay keys)
    if app.new_form.field != FIELD_TASK {
        if let KeyCode::Char(c) = key.code {
            let picker_key = matches!(c, 'h' | 'l' | 'j' | 'k' | ' ')
                || (app.new_form.field == FIELD_IMAGES && matches!(c, 'a' | 'd'));
            if !picker_key && !ctrl {
                app.new_form.field = FIELD_TASK;
                app.new_form.task.input(key);
                return;
            }
        }
    }
    // on picker fields, j/k (↑/↓) move between fields like any vim list
    if app.new_form.field != FIELD_TASK {
        let d = match key.code {
            KeyCode::Char('j') | KeyCode::Down => 1,
            KeyCode::Char('k') | KeyCode::Up => -1,
            _ => 0,
        };
        if d != 0 {
            app.new_form.field = next_field(app.new_form.field, d, engine);
            return;
        }
    }
    let dir = match key.code {
        KeyCode::Left | KeyCode::Char('h') => -1,
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Char(' ') => 1,
        _ => 0,
    };
    if app.new_form.field == FIELD_REPO {
        if dir != 0 { app.cycle_form_repo(dir); }
        return;
    }
    let is_git = app.is_git;
    let f = &mut app.new_form;
    match f.field {
        FIELD_TASK => { f.task.input(key); }
        FIELD_WORKFLOW if dir != 0 => f.cycle_workflow(dir),
        FIELD_HARNESS if dir != 0 => f.cycle_harness(dir),
        FIELD_MODEL if dir != 0 => f.cycle_model(dir),
        // a plain folder can't host a git worktree — the picker stays put
        FIELD_WORKTREE if dir != 0 && is_git => f.worktree_new = !f.worktree_new,
        FIELD_IMAGES => match key.code {
            KeyCode::Char('a') => f.image_input = Some(String::new()),
            KeyCode::Char('d') | KeyCode::Backspace => { f.images.pop(); }
            _ => {}
        },
        _ => {}
    }
}

/// Convert a keypress into the bytes a terminal would send to the PTY.
fn key_to_bytes(key: KeyEvent) -> Vec<u8> {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    // Alt/Meta = ESC prefix (readline word motions, Claude Code's alt-enter, …)
    if key.modifiers.contains(KeyModifiers::ALT) {
        let inner = key_to_bytes(KeyEvent::new(key.code, key.modifiers - KeyModifiers::ALT));
        return if inner.is_empty() { inner } else { [vec![0x1b], inner].concat() };
    }
    match key.code {
        KeyCode::Char(c) => {
            if ctrl {
                let b = (c.to_ascii_uppercase() as u8).wrapping_sub(0x40) & 0x7f;
                vec![b]
            } else {
                c.to_string().into_bytes()
            }
        }
        // Shift+Enter = newline in the agent's input (same bytes as Alt+Enter)
        KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => vec![0x1b, b'\r'],
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(), // shift-tab: lets Claude Code cycle modes

        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        _ => vec![],
    }
}

fn app_now() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn arg_value(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1).cloned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use app::App;
    use std::path::PathBuf;

    fn app() -> App {
        App::new(store::Store::new(PathBuf::from("/tmp/r"), PathBuf::from("/tmp/r/scripts")))
    }
    fn esc() -> KeyEvent {
        KeyEvent::new(KeyCode::Esc, KeyModifiers::empty())
    }

    #[test]
    fn leader_q_on_dashboard_requests_quit() {
        let mut a = app();
        handle_key(&mut a, ch(' '));
        assert!(a.leader_pending);
        handle_key(&mut a, ch('q'));
        assert!(!a.leader_pending);
        assert_eq!(a.mode, Mode::ConfirmQuit);
    }

    #[test]
    fn leader_in_chat_leaves_and_unknown_cancels() {
        let mut a = app();
        let chat_key = KeyEvent::new(KeyCode::Char(' '), KeyModifiers::CONTROL);
        a.mode = Mode::Terminal;
        handle_terminal_key(&mut a, chat_key);
        assert!(a.leader_pending);
        handle_terminal_key(&mut a, ch('q'));
        assert_eq!(a.mode, Mode::Normal);

        a.mode = Mode::Terminal;
        handle_terminal_key(&mut a, chat_key);
        handle_terminal_key(&mut a, ch('#'));
        assert!(!a.leader_pending);
        assert_eq!(a.mode, Mode::Terminal);
        assert!(a.status_msg.contains("no leader binding"));
    }

    #[test]
    fn esc_on_empty_form_closes_without_confirm() {
        let mut a = app();
        a.open_new_agent();
        handle_new_agent_key(&mut a, esc());
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn esc_with_typed_task_asks_to_confirm() {
        let mut a = app();
        a.open_new_agent();
        a.new_form.task.insert_str("do the thing");
        handle_new_agent_key(&mut a, esc());
        assert_eq!(a.mode, Mode::ConfirmDiscard);
        // [n]/esc keeps editing, task preserved
        handle_key(&mut a, esc());
        assert_eq!(a.mode, Mode::NewAgent);
        assert_eq!(a.new_form.task_text(), "do the thing");
        // reopen confirm, [y] discards and resets the form
        handle_new_agent_key(&mut a, esc());
        handle_key(&mut a, KeyEvent::new(KeyCode::Char('y'), KeyModifiers::empty()));
        assert_eq!(a.mode, Mode::Normal);
        assert!(a.new_form.task_text().is_empty());
    }

    fn ch(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::empty())
    }

    #[test]
    fn vim_keys_drive_panes_and_list() {
        use agent::{ManagedAgent, Row};
        let mk = |id: &str| Row::from_managed(&ManagedAgent {
            agent_id: id.into(), harness: None, model: None, worktree: None, branch: None,
            status: Some("done".into()), cost_usd: None, task: None, step_index: None,
            updated_ts: None, pid: None, log_path: None,
        });
        let mut a = app();
        a.all_rows = (0..30).map(|i| mk(&format!("r{i}"))).collect();
        a.apply_filter();
        handle_key(&mut a, ch('j')); // all -> repo line
        assert!(!a.agent_selected());
        handle_key(&mut a, ch('j')); // repo -> its first agent
        assert!(a.agent_selected());
        assert_eq!(a.selected, 0);
        handle_key(&mut a, ch('j'));
        assert_eq!(a.selected, 1);
        handle_key(&mut a, ch('G'));
        assert_eq!(a.selected, 29);
        handle_key(&mut a, ch('g'));
        assert!(!a.agent_selected(), "g = top of the tree");
        handle_key(&mut a, ch('j'));
        handle_key(&mut a, ch('l'));
        assert_eq!(a.selected, 0, "l on a repo steps into its agents");
        handle_key(&mut a, KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
        assert!(a.selected > 1, "ctrl-d half-pages");
        // h climbs to the repo line; agent keys never act there
        handle_key(&mut a, ch('h'));
        assert!(!a.agent_selected());
        handle_key(&mut a, ch('x'));
        assert_eq!(a.mode, Mode::Normal, "x on a repo = unpin (refused: last repo)");
        handle_key(&mut a, ch('d'));
        assert_eq!(a.mode, Mode::Normal, "agent key ignored on a repo line");
        handle_key(&mut a, ch('l'));
        // x always confirms
        handle_key(&mut a, ch('x'));
        assert_eq!(a.mode, Mode::ConfirmRemove);
    }

    #[test]
    fn form_enter_launches_alt_enter_newlines() {
        let mut a = app();
        a.open_new_agent();
        handle_new_agent_key(&mut a, ch('a'));
        handle_new_agent_key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        handle_new_agent_key(&mut a, ch('b'));
        assert_eq!(a.new_form.task_text(), "a\nb");
        a.new_form.task = Default::default();
        // plain Enter submits: empty task → validation error, not a newline
        handle_new_agent_key(&mut a, KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()));
        assert!(a.new_form.error.is_some());
        assert_eq!(a.new_form.task_text(), "");
    }

    #[test]
    fn ctrl_o_reaches_the_agent_ctrl_q_detaches() {
        let mut a = app();
        a.mode = Mode::Terminal;
        handle_terminal_key(&mut a, KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
        assert_eq!(a.mode, Mode::Terminal, "Ctrl-o is Claude's transcript key");
        handle_terminal_key(&mut a, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn typing_on_a_picker_field_goes_to_the_task() {
        let mut a = app();
        a.open_new_agent();
        handle_new_agent_key(&mut a, KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(a.new_form.field, app::FIELD_REPO);
        for c in "fix".chars() { handle_new_agent_key(&mut a, ch(c)); }
        assert_eq!(a.new_form.field, app::FIELD_TASK);
        assert_eq!(a.new_form.task_text(), "fix", "'f' jumped to task; 'i','x' typed there");
    }

    #[test]
    fn paste_never_submits_line_by_line() {
        let mut a = app();
        // on the dashboard: opens a new agent with the whole text as the task
        handle_paste(&mut a, "fix this\nstack trace line\n");
        assert_eq!(a.mode, Mode::NewAgent);
        assert!(a.new_form.task_text().contains("fix this\nstack trace line"), "{:?}", a.new_form.task_text());
        assert!(a.new_form.error.is_none(), "paste must not trigger launch");
        // one-line inputs take the first line only
        a.mode = Mode::Filter;
        handle_paste(&mut a, "abc\ndef");
        assert_eq!(a.filter_text, "abc");
    }

    #[test]
    fn alt_key_sends_esc_prefix() {
        let b = key_to_bytes(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::ALT));
        assert_eq!(b, b"\x1bb");
    }

    #[test]
    fn shift_enter_sends_newline_sequence() {
        let k = KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(key_to_bytes(k), vec![0x1b, b'\r']);
        assert_eq!(key_to_bytes(KeyEvent::new(KeyCode::Enter, KeyModifiers::empty())), vec![b'\r']);
    }

    #[test]
    fn backtab_forwards_shift_tab_sequence() {
        // shift-tab must reach the PTY as CSI Z so Claude Code can cycle modes.
        let bytes = key_to_bytes(KeyEvent::new(KeyCode::BackTab, KeyModifiers::empty()));
        assert_eq!(bytes, b"\x1b[Z");
    }

    #[test]
    fn form_tab_reaches_workflow_field_and_cycles() {
        let mut a = app();
        a.open_new_agent();
        // Tab off the task editor lands on the repo picker, then harness.
        handle_new_agent_key(&mut a, KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(a.new_form.field, app::FIELD_REPO);
        handle_new_agent_key(&mut a, KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(a.new_form.field, app::FIELD_HARNESS);
        handle_new_agent_key(&mut a, KeyEvent::new(KeyCode::Right, KeyModifiers::empty()));
        assert_eq!(a.new_form.harness(), "opencode");
        // no engine → the workflow picker is skipped
        handle_new_agent_key(&mut a, KeyEvent::new(KeyCode::Tab, KeyModifiers::empty()));
        assert_eq!(a.new_form.field, app::FIELD_MODEL);
    }

    #[test]
    fn diff_mode_opens_scrolls_and_closes() {
        let mut a = app();
        // no worktree selected → show_diff reports, stays Normal
        a.show_diff();
        assert_eq!(a.mode, Mode::Normal);
        // simulate an open diff view and drive scroll/close keys
        a.diff_text = (0..50).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        a.mode = Mode::Diff;
        handle_key(&mut a, ch('j'));
        assert_eq!(a.diff_scroll, 1);
        handle_key(&mut a, KeyEvent::new(KeyCode::PageDown, KeyModifiers::empty()));
        assert_eq!(a.diff_scroll, 21);
        handle_key(&mut a, ch('k'));
        assert_eq!(a.diff_scroll, 20);
        handle_key(&mut a, ch('q'));
        assert_eq!(a.mode, Mode::Normal);
    }

    #[test]
    fn log_view_reads_file_scrolls_and_closes() {
        use agent::{ManagedAgent, Row};
        let dir = std::env::temp_dir().join(format!("crew-logv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let logp = dir.join("a.log");
        std::fs::write(&logp, "line1\nERROR boom\nline3\n").unwrap();
        let ma = ManagedAgent {
            agent_id: "m1".into(), harness: Some("cursor".into()), model: None,
            worktree: None, branch: None, status: Some("running".into()),
            cost_usd: None, task: Some("t".into()), step_index: None,
            updated_ts: None, pid: None,
            log_path: Some(logp.to_string_lossy().into_owned()),
        };
        let mut a = app();
        a.rows = vec![Row::from_managed(&ma)];
        a.selected = 0;
        a.show_log();
        assert_eq!(a.mode, Mode::Log);
        assert!(a.log_text.contains("ERROR boom"), "{}", a.log_text);
        handle_key(&mut a, ch('k')); // scroll
        handle_key(&mut a, ch('q'));
        assert_eq!(a.mode, Mode::Normal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_follow_tracks_bottom_but_respects_scrollback() {
        use agent::{ManagedAgent, Row};
        let dir = std::env::temp_dir().join(format!("crew-logf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let logp = dir.join("f.log");
        std::fs::write(&logp, "l1\nl2\nl3\n").unwrap();
        let ma = ManagedAgent {
            agent_id: "f".into(), harness: Some("cursor".into()), model: None,
            worktree: None, branch: None, status: Some("running".into()),
            cost_usd: None, task: None, step_index: None, updated_ts: None, pid: None,
            log_path: Some(logp.to_string_lossy().into_owned()),
        };
        let mut a = app();
        a.rows = vec![Row::from_managed(&ma)];
        a.selected = 0;
        a.show_log();
        assert_eq!(a.log_scroll, 2); // parked at bottom (3 lines → idx 2)

        // new output arrives; following → scroll advances to the new bottom
        std::fs::write(&logp, "l1\nl2\nl3\nl4\nl5\n").unwrap();
        a.tick_log();
        assert_eq!(a.log_scroll, 4);

        // reader scrolls up; new output must NOT yank them back down
        a.scroll_log(-3); // now at 1
        assert_eq!(a.log_scroll, 1);
        std::fs::write(&logp, "l1\nl2\nl3\nl4\nl5\nl6\n").unwrap();
        a.tick_log();
        assert_eq!(a.log_scroll, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Full drive of a live agent: launch a stub PTY agent, focus its terminal,
    /// forward a keystroke, leave, bulk-stop, then clear it.
    #[test]
    fn e2e_spawn_chat_stopall_clear() {
        let dir = std::env::temp_dir().join(format!("crew-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("CREW_INTERACTIVE_CMD_CLAUDE", "sh -c cat");
        let mut a = App::new(store::Store::new(dir.clone(), dir.join("scripts")));
        a.term_size = (80, 24);

        // launch via the form, running in the dir itself (no worktree.py)
        a.open_new_agent();
        a.new_form.task.insert_str("hello agent");
        a.new_form.worktree_new = false;
        a.spawn_from_form();
        assert_eq!(a.mode, Mode::Terminal, "should drop into chat");
        assert_eq!(a.agents.len(), 1);
        assert!(a.agents[0].is_alive());

        // forward a keystroke to the PTY, then leave the terminal (Ctrl-q)
        handle_terminal_key(&mut a, ch('x'));
        handle_terminal_key(&mut a, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        assert_eq!(a.mode, Mode::Normal);

        // review its worktree diff (dir isn't a git repo → friendly message)
        handle_key(&mut a, ch('d'));
        assert_eq!(a.mode, Mode::Diff);
        assert!(!a.diff_text.is_empty());
        handle_key(&mut a, esc());

        // bulk stop-all: S → confirm → y
        handle_key(&mut a, ch('S'));
        assert_eq!(a.mode, Mode::ConfirmStopAll);
        handle_key(&mut a, ch('y'));
        assert_eq!(a.mode, Mode::Normal);
        // give the kill a moment to land
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert!(!a.agents[0].is_alive(), "stop_all should have killed the agent");

        // the stopped agent becomes resumable; enter relaunches it in place
        a.refresh().unwrap();
        let r = a.selected_row().unwrap().clone();
        assert!(r.resumable && r.proc_idx.is_some(), "stopped agent should be resumable: {:?}", r.status);
        std::env::set_var("CREW_RESUME_CMD_CLAUDE", "cat");
        handle_key(&mut a, ch('l'));
        assert_eq!(a.mode, Mode::Terminal, "l should resume into chat");
        assert_eq!(a.agents.len(), 1, "dead proc replaced, not duplicated");
        assert!(a.agents[0].is_alive());
        handle_terminal_key(&mut a, KeyEvent::new(KeyCode::Char('q'), KeyModifiers::CONTROL));
        a.stop_all();
        std::thread::sleep(std::time::Duration::from_millis(200));
        std::env::remove_var("CREW_RESUME_CMD_CLAUDE");

        // clear finished (confirmed) drops the dead proc
        handle_key(&mut a, ch('c'));
        assert_eq!(a.mode, Mode::ConfirmClear);
        handle_key(&mut a, ch('y'));
        assert!(a.agents.is_empty(), "clear_finished should remove the dead agent");

        std::env::remove_var("CREW_INTERACTIVE_CMD_CLAUDE");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// e2e: `crew --new` creates a worktree natively and runs the harness
    /// headless in it (stub harness writes a marker) — no engine needed.
    #[test]
    fn e2e_new_launches_headless_in_worktree() {
        use std::process::Command;
        let manifest = env!("CARGO_MANIFEST_DIR");
        let bin = PathBuf::from(manifest).join("target/debug/crew");
        let fake = PathBuf::from(manifest).join("tests/fixtures/fake_harness.py");
        if !bin.exists() || !fake.exists() { return; }
        let py = std::env::var("CREW_PYTHON").unwrap_or_else(|_| "python3".into());

        let repo = std::env::temp_dir().join(format!("crew-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let g = |a: &[&str]| { Command::new("git").arg("-C").arg(&repo).args(a).output().unwrap(); };
        g(&["init", "-q"]); g(&["config", "user.email", "t@t"]); g(&["config", "user.name", "t"]);
        std::fs::write(repo.join("seed.txt"), "x").unwrap();
        g(&["add", "seed.txt"]); g(&["commit", "-qm", "init"]);

        let out = Command::new(&bin)
            .args(["--new", "hello task", "--harness", "claude"])
            .current_dir(&repo)
            .env("XDG_CONFIG_HOME", repo.join(".xdg")) // never touch the real pinned list
            .env("CREW_HEADLESS_CMD_CLAUDE", format!("{} {}", py, fake.display()))
            .output().unwrap();
        assert!(out.status.success(), "stderr: {}", String::from_utf8_lossy(&out.stderr));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("branch crew/"), "stdout: {stdout}");

        // the stub harness ran inside the created worktree
        let slug = stdout.split("branch crew/").nth(1).unwrap()
            .split_whitespace().next().unwrap().to_string();
        let marker = repo.join(".crew/worktrees").join(&slug).join("fake_out.txt");
        assert!(marker.exists(), "marker missing at {}", marker.display());
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// e2e: push commits the worktree and pushes its branch to origin (a local
    /// bare remote — no network). Proves the push path works standalone.
    #[test]
    fn e2e_push_pushes_branch_to_origin() {
        use std::process::Command;
        let base = std::env::temp_dir().join(format!("crew-push-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let origin = base.join("origin.git");
        let work = base.join("work");
        Command::new("git").args(["init", "--bare", "-q"]).arg(&origin).output().unwrap();
        let g = |a: &[&str]| { Command::new("git").arg("-C").arg(&work).args(a).output().unwrap(); };
        std::fs::create_dir_all(&work).unwrap();
        g(&["init", "-q"]); g(&["config", "user.email", "t@t"]); g(&["config", "user.name", "t"]);
        std::fs::write(work.join("seed.txt"), "x").unwrap();
        g(&["add", "seed.txt"]); g(&["commit", "-qm", "init"]);
        g(&["remote", "add", "origin", origin.to_str().unwrap()]);

        let work = std::fs::canonicalize(&work).unwrap();
        let mut app = App::new(store::Store::new(work.clone(), work.join("no-scripts")));
        let wt = app.store.create_worktree("push-me").unwrap();
        std::fs::write(std::path::Path::new(&wt).join("new.txt"), "hi").unwrap();
        app.push_queue = vec![(wt, "crew/push-me".into(), "add new.txt".into())];
        app.confirm_push();

        // origin now has the branch
        let out = Command::new("git").arg("-C").arg(&origin)
            .args(["branch", "--list", "crew/push-me"]).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("crew/push-me"),
            "branch not on origin; status: {}", app.status_msg);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// e2e: retry/re-delegate from the TUI dispatches the task on the chosen
    /// harness and records a new agent row (polled from the store).
    #[test]
    fn e2e_retry_redelegates_and_records_agent() {
        use agent::{ManagedAgent, Row};
        use std::process::Command;
        let manifest = env!("CARGO_MANIFEST_DIR");
        let root = PathBuf::from(manifest);
        let scripts = root.join("scripts");
        let fake = root.join("tests/fixtures/fake_harness.py");
        if !fake.exists() || !scripts.join("store.py").exists() { return; }
        let py = std::env::var("CREW_PYTHON").unwrap_or_else(|_| "python3".into());

        let repo = std::env::temp_dir().join(format!("crew-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let git = |a: &[&str]| { Command::new("git").arg("-C").arg(&repo).args(a).output().unwrap(); };
        git(&["init", "-q"]); git(&["config", "user.email", "t@t"]); git(&["config", "user.name", "t"]);
        std::fs::write(repo.join("seed.txt"), "seed").unwrap();
        git(&["add", "seed.txt"]); git(&["commit", "-qm", "init"]);

        let data = repo.join(".crew-data");
        std::env::set_var("CLAUDE_PLUGIN_DATA", &data);
        std::env::set_var("CREW_HARNESS_CMD_CURSOR", format!("{} {}", py, fake.display()));

        let mut app = App::new(store::Store::new(std::fs::canonicalize(&repo).unwrap(), scripts.clone()));
        let ma = ManagedAgent {
            agent_id: "orig".into(), harness: Some("cursor".into()), model: None,
            worktree: None, branch: None, status: Some("failed".into()), cost_usd: None,
            task: Some("retry writes a file".into()), step_index: None, updated_ts: None,
            pid: None, log_path: None,
        };
        app.rows = vec![Row::from_managed(&ma)];
        app.selected = 0;
        app.open_retry();
        assert_eq!(app.mode, Mode::Retry);
        assert_eq!(app.retry_harness_idx, 2); // cursor
        app.confirm_retry();
        assert_eq!(app.mode, Mode::Normal);
        let id = app.status_msg.rsplit("→ ").next().unwrap().trim().to_string();
        assert!(id.starts_with("retry_"), "status: {}", app.status_msg);

        // poll the store until the detached dispatch finishes
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        let mut done = false;
        while std::time::Instant::now() < deadline {
            let out = Command::new(&py).arg(scripts.join("store.py"))
                .args(["agent-get", "--id", &id])
                .env("CLAUDE_PLUGIN_DATA", &data).output().unwrap();
            let s = String::from_utf8_lossy(&out.stdout);
            if s.contains("\"status\": \"done\"") { done = true; break; }
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(done, "retry agent never reached done");

        std::env::remove_var("CLAUDE_PLUGIN_DATA");
        std::env::remove_var("CREW_HARNESS_CMD_CURSOR");
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// Cross-boundary e2e: a delegated sub-agent recorded by dispatch.py (via the
    /// same path the MCP `delegate` tool uses) must appear in the TUI's list.
    #[test]
    fn e2e_delegated_agent_shows_in_tui() {
        use std::process::Command;
        let manifest = env!("CARGO_MANIFEST_DIR");
        let root = PathBuf::from(manifest);
        let scripts = root.join("scripts");
        let fake = root.join("tests/fixtures/fake_harness.py");
        if !fake.exists() || !scripts.join("dispatch.py").exists() {
            return; // engine scripts not present — skip
        }
        let py = std::env::var("CREW_PYTHON").unwrap_or_else(|_| "python3".into());

        // throwaway git repo
        let repo = std::env::temp_dir().join(format!("crew-deleg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| {
            Command::new("git").arg("-C").arg(&repo).args(args).output().unwrap();
        };
        git(&["init", "-q"]);
        git(&["config", "user.email", "t@t"]);
        git(&["config", "user.name", "t"]);
        std::fs::write(repo.join("seed.txt"), "seed").unwrap();
        git(&["add", "seed.txt"]);
        git(&["commit", "-qm", "init"]);

        let data = repo.join(".crew-data");
        let step = repo.join("step.json");
        std::fs::write(&step,
            r#"{"title":"delegated bit","objective":"write file","acceptance":"test -f fake_out.txt","files":["fake_out.txt"],"scope_deny":[]}"#).unwrap();

        // run dispatch exactly like the delegation MCP does, with the fake harness
        let out = Command::new(&py)
            .arg(scripts.join("dispatch.py"))
            .args(["--harness", "cursor", "--model", "auto",
                   "--repo", repo.to_str().unwrap(),
                   "--new-worktree", "--slug", "deleg",
                   "--step-file", step.to_str().unwrap(),
                   "--agent-id", "tui_deleg_e2e"])
            .env("CLAUDE_PLUGIN_DATA", &data)
            .env("CREW_HARNESS_CMD_CURSOR", format!("{} {}", py, fake.display()))
            .env("PYTHONPATH", &scripts)
            .output()
            .unwrap();
        assert!(out.status.success(), "dispatch failed: {}",
                String::from_utf8_lossy(&out.stderr));

        // now the TUI store must read that agent back and refresh must list it
        std::env::set_var("CLAUDE_PLUGIN_DATA", &data);
        let mut app = App::new(store::Store::new(
            std::fs::canonicalize(&repo).unwrap(), scripts.clone()));
        app.refresh().unwrap();
        let found = app.rows.iter().find(|r| r.id == "tui_deleg_e2e")
            .expect("delegated agent should appear in the TUI list");
        assert_eq!(found.harness, "cursor");
        assert_eq!(found.status, "done");
        assert!(found.worktree.is_some());

        std::env::remove_var("CLAUDE_PLUGIN_DATA");
        let _ = std::fs::remove_dir_all(&repo);
    }
}
