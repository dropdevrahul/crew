//! In-TUI interactive agents: each agent's REPL is shown in a PTY we own,
//! parsed by a vt100 emulator and rendered in a pane *inside* the dashboard.
//! Keystrokes are written straight back to the PTY.
//!
//! Two backends:
//! - `Pty`: the harness runs directly in crew's PTY — it dies with crew.
//! - `Tmux`: the harness runs in a detached session on a private tmux server
//!   (`tmux -L crew`, no user config); crew's PTY runs a `tmux attach` client.
//!   Quitting crew only detaches; agents keep working and are re-attached on
//!   the next start (or `tmux -L crew attach -t crew-<id>` from anywhere).

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

/// Per-session env markers of a parent Claude Code process (see `spawn`).
const SESSION_ENV: [&str; 9] = [
    "CLAUDECODE", "CLAUDE_CODE_CHILD_SESSION", "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT", "CLAUDE_CODE_EXECPATH", "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_MESSAGING_SOCKET", "CLAUDE_CODE_MESSAGING_TOKEN", "CLAUDE_PID",
];

/// Where agent processes live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Pty,
    Tmux,
}

static BACKEND: AtomicU8 = AtomicU8::new(0); // tests default to Pty

pub fn set_backend(b: Backend) {
    BACKEND.store(if b == Backend::Tmux { 1 } else { 0 }, Ordering::Relaxed);
}
pub fn backend() -> Backend {
    if BACKEND.load(Ordering::Relaxed) == 1 { Backend::Tmux } else { Backend::Pty }
}

/// Setting → backend: "pty" | "tmux" | anything else = auto (tmux if installed).
/// "tmux" without tmux on PATH falls back to pty rather than failing launches.
pub fn resolve_backend(setting: &str) -> Backend {
    let has_tmux = crate::app::on_path("tmux");
    match setting {
        "pty" => Backend::Pty,
        _ if has_tmux => Backend::Tmux,
        _ => Backend::Pty,
    }
}

thread_local! {
    /// Test-only socket override, per thread (env vars would leak across
    /// parallel tests).
    static SOCK_OVERRIDE: std::cell::RefCell<Option<String>> = const { std::cell::RefCell::new(None) };
}

/// Private tmux server name (`tmux -L <sock>`).
fn sock() -> String {
    SOCK_OVERRIDE.with(|o| o.borrow().clone())
        .or_else(|| std::env::var("CREW_TMUX_SOCKET").ok())
        .unwrap_or_else(|| "crew".into())
}

/// `tmux -L <sock> -f /dev/null …` with the parent-session env stripped, so
/// the server (and every agent it starts) never inherits it.
fn tmux_cmd() -> Command {
    let mut c = Command::new("tmux");
    c.args(["-L", &sock(), "-f", "/dev/null"]);
    for k in SESSION_ENV {
        c.env_remove(k);
    }
    c.env_remove("TMUX");
    c
}

/// tmux session name for an agent id (tmux forbids '.' and ':').
pub fn session_name(id: &str) -> String {
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    format!("crew-{}", safe)
}

/// Live crew tmux sessions (one `list-sessions`, cached ~1s — refresh asks
/// every tick for every workspace).
pub fn tmux_sessions() -> HashSet<String> {
    static CACHE: Mutex<Option<(std::time::Instant, HashSet<String>)>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, set)) = c.as_ref() {
        if at.elapsed() < std::time::Duration::from_millis(900) {
            return set.clone();
        }
    }
    let set: HashSet<String> = tmux_cmd().args(["list-sessions", "-F", "#S"]).output().ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(String::from).collect())
        .unwrap_or_default();
    *c = Some((std::time::Instant::now(), set.clone()));
    set
}

fn tmux_live(name: &str) -> bool {
    crate::app::on_path("tmux")
        && tmux_cmd().args(["has-session", "-t", &format!("={}", name)]).output()
            .map(|o| o.status.success()).unwrap_or(false)
}

/// Create the detached session running `argv`, configured to be invisible
/// inside crew's pane: no status bar, no prefix key, mouse on (wheel scrolls),
/// no Esc delay.
fn tmux_create(name: &str, cwd: &str, argv: &[String], rows: u16, cols: u16) -> Result<()> {
    let mut c = tmux_cmd();
    c.args(["new-session", "-d", "-s", name, "-x", &cols.to_string(), "-y", &rows.to_string(), "-c", cwd]);
    // a 256-color TERM that exists in every terminfo (macOS lacks tmux-256color)
    c.args(["env", "TERM=screen-256color", "COLORTERM=truecolor"]);
    c.args(argv);
    for opt in [["status", "off"], ["prefix", "None"], ["prefix2", "None"], ["mouse", "on"], ["history-limit", "10000"]] {
        c.args([";", "set-option", "-t", name, opt[0], opt[1]]);
    }
    c.args([";", "set-option", "-s", "escape-time", "0"]);
    let out = c.output().context("run tmux")?;
    if !out.status.success() {
        anyhow::bail!("tmux: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

/// A single interactive agent: PTY + vt100 screen + child process handle.
pub struct AgentProc {
    pub id: String,
    pub harness: String,
    pub model: String,
    pub task: String,
    pub worktree: String,
    /// Unix seconds when this agent was launched (for "running Nm" in the list).
    pub started: f64,
    /// vt100 screen, fed by a background reader thread; read by the renderer.
    pub parser: Arc<Mutex<vt100::Parser>>,
    /// Unix millis of the last byte the agent printed (0 = nothing yet). A
    /// live agent that's gone quiet is waiting for input.
    last_output: Arc<AtomicU64>,
    /// Stop feeding the screen: keeps the agent's last frame visible after it
    /// ends, instead of the tmux client's exit (screen clear + "[exited]").
    frozen: Arc<AtomicBool>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    /// tmux session holding the real process (Tmux backend); `child` is then
    /// just the attach client.
    pub tmux: Option<String>,
}

/// Order two (row,col) selection corners reading-order: (start, end).
pub fn normalize_selection(a: (u16, u16), b: (u16, u16)) -> ((u16, u16), (u16, u16)) {
    if a <= b { (a, b) } else { (b, a) }
}

impl AgentProc {
    /// Spawn `argv` in a PTY at `cwd`. `task` is display-only — the first
    /// prompt travels in argv (see `interactive_argv`), never typed in.
    pub fn spawn(
        id: String, harness: String, model: String, task: String, worktree: String,
        argv: &[String], rows: u16, cols: u16,
    ) -> Result<AgentProc> {
        Self::spawn_with(backend(), id, harness, model, task, worktree, argv, rows, cols)
    }

    /// Like `spawn`, with an explicit backend. On Tmux — or whenever a live
    /// session for this id already exists — `argv` runs in (or is replaced by
    /// attaching to) the agent's tmux session.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn_with(
        be: Backend, id: String, harness: String, model: String, task: String, worktree: String,
        argv: &[String], rows: u16, cols: u16,
    ) -> Result<AgentProc> {
        let name = session_name(&id);
        let (argv, tmux): (Vec<String>, Option<String>) = if tmux_live(&name) {
            (Vec::new(), Some(name)) // still running from a previous crew: just attach
        } else if be == Backend::Tmux {
            tmux_create(&name, &worktree, argv, rows, cols)?;
            (Vec::new(), Some(name))
        } else {
            (argv.to_vec(), None)
        };
        let argv = match &tmux {
            Some(n) => ["tmux", "-L", &sock(), "-f", "/dev/null", "attach", "-t", &format!("={}", n)]
                .iter().map(|s| s.to_string()).collect(),
            None => argv,
        };
        let pty = native_pty_system();
        let pair = pty
            .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
            .context("openpty")?;

        let mut cmd = CommandBuilder::new(&argv[0]);
        for a in &argv[1..] {
            cmd.arg(a);
        }
        cmd.cwd(&worktree);
        cmd.env("TERM", "xterm-256color");
        // crew may itself run inside a Claude Code session; its per-session
        // markers would make each agent a "child" (transcripts off → resume
        // breaks) and leak the parent's messaging socket/token. User settings
        // (CLAUDE_CONFIG_DIR, CLAUDE_EFFORT, …) pass through untouched.
        for k in SESSION_ENV {
            cmd.env_remove(k);
        }
        cmd.env_remove("TMUX"); // attach refuses to nest otherwise

        let child = pair.slave.spawn_command(cmd).context("spawn agent command")?;
        // slave no longer needed in this process; dropping it lets EOF propagate.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().context("clone reader")?;
        let writer = pair.master.take_writer().context("take writer")?;
        let parser = Arc::new(Mutex::new(vt100::Parser::new(rows, cols, 2000)));
        let last_output = Arc::new(AtomicU64::new(0));
        let frozen = Arc::new(AtomicBool::new(false));

        {
            let parser = parser.clone();
            let last = last_output.clone();
            let frozen = frozen.clone();
            let is_tmux = tmux.is_some();
            thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            // the tmux client's goodbye ("[exited]", "[server exited]",
                            // "[detached…]") comes with a screen clear — drop it
                            let tail = &buf[..n];
                            if is_tmux && [&b"[exited]"[..], b"[server exited]", b"[detached"]
                                .iter().any(|m| tail.windows(m.len()).any(|w| w == *m))
                            {
                                frozen.store(true, Ordering::Relaxed);
                            }
                            if frozen.load(Ordering::Relaxed) {
                                continue;
                            }
                            last.store(now_ms(), Ordering::Relaxed);
                            if let Ok(mut p) = parser.lock() {
                                p.process(&buf[..n]);
                            }
                        }
                    }
                }
            });
        }

        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs_f64())
            .unwrap_or(0.0);
        let proc = AgentProc {
            id,
            harness,
            model,
            task,
            worktree,
            started,
            parser,
            last_output,
            frozen,
            writer,
            master: pair.master,
            child,
            tmux,
        };
        Ok(proc)
    }

    /// Text under a drag-selection (pane-relative (row,col) corners, either
    /// order; both end cells inclusive), trailing blanks trimmed per line.
    pub fn selection_text(&self, a: (u16, u16), b: (u16, u16)) -> String {
        let ((r0, c0), (r1, c1)) = normalize_selection(a, b);
        let Ok(p) = self.parser.lock() else { return String::new() };
        let text = p.screen().contents_between(r0, c0, r1, c1.saturating_add(1));
        text.lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
    }

    /// Forward raw bytes (a keypress) to the agent. Typing snaps a scrolled-
    /// back view to the live bottom, like any terminal.
    pub fn send(&mut self, bytes: &[u8]) {
        if let Ok(mut p) = self.parser.lock() {
            p.set_scrollback(0);
        }
        let _ = self.writer.write_all(bytes);
        let _ = self.writer.flush();
    }

    /// Paste text as ONE unit. If the agent enabled bracketed paste (Claude
    /// Code, opencode do) wrap it so embedded newlines don't submit early;
    /// otherwise send it as typed (newline = Enter).
    pub fn paste(&mut self, text: &str) {
        let bracketed = self.parser.lock().map(|p| p.screen().bracketed_paste()).unwrap_or(false);
        let body = text.replace("\r\n", "\n");
        let bytes = if bracketed {
            format!("\x1b[200~{}\x1b[201~", body).into_bytes()
        } else {
            body.replace('\n', "\r").into_bytes()
        };
        self.send(&bytes);
    }

    /// Mouse wheel over the chat at pane cell (col,row), like a real terminal:
    /// forward it if the agent asked for mouse input (Claude's fullscreen UI
    /// scrolls itself); on an alternate screen without mouse mode send arrow
    /// keys ("alternate scroll"); otherwise scroll crew's own scrollback.
    pub fn wheel(&mut self, up: bool, col: u16, row: u16) {
        let (mode, enc, alt, app_cursor) = match self.parser.lock() {
            Ok(p) => {
                let s = p.screen();
                (s.mouse_protocol_mode(), s.mouse_protocol_encoding(), s.alternate_screen(), s.application_cursor())
            }
            Err(_) => return,
        };
        let btn: u8 = if up { 64 } else { 65 };
        if mode != vt100::MouseProtocolMode::None {
            let bytes = if enc == vt100::MouseProtocolEncoding::Sgr {
                format!("\x1b[<{};{};{}M", btn, col + 1, row + 1).into_bytes()
            } else {
                // legacy X10 encoding: 32-offset bytes, coords capped at 223
                let c = |v: u16| (32 + (v + 1).min(223)) as u8;
                vec![0x1b, b'[', b'M', 32 + btn, c(col), c(row)]
            };
            self.send(&bytes);
        } else if alt {
            let k: &[u8] = match (up, app_cursor) {
                (true, true) => b"\x1bOA",
                (true, false) => b"\x1b[A",
                (false, true) => b"\x1bOB",
                (false, false) => b"\x1b[B",
            };
            self.send(&k.repeat(3));
        } else {
            self.scroll(if up { 3 } else { -3 });
        }
    }

    /// Scroll the view into scrollback: `delta` > 0 = up (older), < 0 = down.
    /// vt100 clamps to what it has (2000 lines).
    pub fn scroll(&self, delta: i64) {
        if let Ok(mut p) = self.parser.lock() {
            let cur = p.screen().scrollback() as i64;
            p.set_scrollback((cur + delta).max(0) as usize);
        }
    }

    /// Resize the PTY + emulator to fit the render area. Takes `&self` (both
    /// `MasterPty::resize` and `Mutex::lock` do) so the renderer can call it.
    pub fn resize(&self, rows: u16, cols: u16) {
        if rows == 0 || cols == 0 {
            return;
        }
        if let Ok(mut p) = self.parser.lock() {
            if p.screen().size() == (rows, cols) {
                return;
            }
            let _ = self.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
            p.set_size(rows, cols);
        }
    }

    /// Alive, has printed something, and silent for `idle_ms` — i.e. the REPL
    /// finished its turn and is sitting at its prompt.
    /// ponytail: silence heuristic; a harness that animates while idle never reads as waiting
    pub fn is_waiting(&mut self, idle_ms: u64) -> bool {
        let last = self.last_output.load(Ordering::Relaxed);
        last > 0 && now_ms().saturating_sub(last) >= idle_ms && self.is_alive()
    }

    /// True while the child process is still running.
    pub fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Terminate the agent (its tmux session too, if any).
    pub fn kill(&mut self) {
        if let Some(n) = &self.tmux {
            self.frozen.store(true, Ordering::Relaxed); // keep the last frame
            let _ = tmux_cmd().args(["kill-session", "-t", &format!("={}", n)]).output();
        }
        let _ = self.child.kill();
    }

    /// Leave the agent running: drop only our view of it. Tmux agents keep
    /// working in their session; pty agents can't outlive crew, so this kills.
    pub fn detach(&mut self) {
        self.frozen.store(true, Ordering::Relaxed);
        let _ = self.child.kill();
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_normalizes_reversed_drag() {
        assert_eq!(normalize_selection((3, 5), (1, 2)), ((1, 2), (3, 5)));
        assert_eq!(normalize_selection((1, 2), (3, 5)), ((1, 2), (3, 5)));
    }

    #[test]
    fn selection_extracts_text_from_screen() {
        let mut p = vt100::Parser::new(5, 20, 0);
        p.process(b"hello world\r\nsecond line");
        let s = p.screen();
        assert_eq!(s.contents_between(0, 6, 0, 11), "world");
        assert_eq!(s.contents_between(0, 6, 1, 6), "world\nsecond");
    }

    #[test]
    fn pty_captures_command_output() {
        // spawn a trivial command in a PTY and confirm vt100 captures its output.
        let dir = std::env::temp_dir();
        let argv = vec!["printf".to_string(), "HELLO_PTY".to_string()];
        let mut a = AgentProc::spawn(
            "t1".into(), "stub".into(), "m".into(), String::new(),
            dir.to_string_lossy().to_string(), &argv, 24, 80,
        )
        .unwrap();
        // let the reader thread drain the output
        std::thread::sleep(std::time::Duration::from_millis(400));
        let screen = a.parser.lock().unwrap().screen().contents();
        assert!(screen.contains("HELLO_PTY"), "vt100 did not capture output: {:?}", screen);
        assert!(a.last_output.load(Ordering::Relaxed) > 0, "output timestamp not recorded");
        a.kill();
    }

    #[test]
    fn quiet_live_agent_reads_as_waiting() {
        let dir = std::env::temp_dir();
        let argv: Vec<String> = ["sh", "-c", "echo ready; sleep 5"].iter().map(|s| s.to_string()).collect();
        let mut a = AgentProc::spawn("w".into(), "stub".into(), "m".into(), String::new(),
            dir.to_string_lossy().to_string(), &argv, 24, 80).unwrap();
        assert!(!a.is_waiting(200), "nothing printed yet / just printed");
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(a.is_waiting(200), "printed then went quiet → waiting");
        assert!(!a.is_waiting(60_000), "not quiet that long");
        a.kill();
    }

    #[test]
    fn scrollback_and_paste() {
        let dir = std::env::temp_dir();
        // 60 lines into a 10-row screen → 50 lines of scrollback; then cat echoes
        let argv: Vec<String> = ["sh", "-c", "seq 1 60; exec cat"].iter().map(|s| s.to_string()).collect();
        let mut a = AgentProc::spawn("s".into(), "stub".into(), "m".into(), String::new(),
            dir.to_string_lossy().to_string(), &argv, 10, 40).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        a.scroll(20);
        let top = a.parser.lock().unwrap().screen().contents();
        assert!(top.contains("\n35\n") || top.starts_with("3"), "scrolled view: {top:?}");
        assert!(!top.contains("\n60"), "live bottom hidden while scrolled: {top:?}");
        // typing/pasting snaps back to live; plain (non-bracketed) paste = typed lines
        a.paste("p1\np2\n");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let p = a.parser.lock().unwrap();
        assert_eq!(p.screen().scrollback(), 0);
        let live = p.screen().contents();
        assert!(live.contains("p1") && live.contains("p2"), "{live:?}");
        drop(p);
        a.kill();
    }

    #[test]
    fn wheel_is_forwarded_when_the_agent_wants_mouse() {
        let dir = std::env::temp_dir();
        // enable SGR mouse reporting (1000 + 1006), then echo raw input back as hex
        let argv: Vec<String> = ["sh", "-c", "printf '\\033[?1000h\\033[?1006h'; stty raw -echo; head -c 10 | od -An -c"]
            .iter().map(|s| s.to_string()).collect();
        let mut a = AgentProc::spawn("m".into(), "stub".into(), "m".into(), String::new(),
            dir.to_string_lossy().to_string(), &argv, 24, 80).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        a.wheel(true, 4, 2);
        std::thread::sleep(std::time::Duration::from_millis(400));
        let out = a.parser.lock().unwrap().screen().contents();
        assert!(out.contains("<   6   4   ;   5   ;   3   M"), "SGR wheel not forwarded: {out:?}");
        a.kill();
    }

    #[test]
    fn session_markers_are_not_inherited() {
        std::env::set_var("CLAUDE_CODE_CHILD_SESSION", "1");
        let dir = std::env::temp_dir();
        let argv: Vec<String> = ["sh", "-c", "echo marker=${CLAUDE_CODE_CHILD_SESSION:-unset}; sleep 1"]
            .iter().map(|s| s.to_string()).collect();
        let mut a = AgentProc::spawn("e".into(), "stub".into(), "m".into(), String::new(),
            dir.to_string_lossy().to_string(), &argv, 24, 80).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(400));
        let out = a.parser.lock().unwrap().screen().contents();
        std::env::remove_var("CLAUDE_CODE_CHILD_SESSION");
        assert!(out.contains("marker=unset"), "{out:?}");
        a.kill();
    }

    /// Tmux backend: the agent survives its viewer (crew) going away, a new
    /// viewer re-attaches to the same session, and kill ends it.
    #[test]
    fn tmux_agent_survives_detach_and_reattaches() {
        if !crate::app::on_path("tmux") { return; }
        SOCK_OVERRIDE.with(|o| *o.borrow_mut() = Some(format!("crew-test-{}", std::process::id())));
        let dir = std::env::temp_dir().to_string_lossy().to_string();
        let argv: Vec<String> = ["sh", "-c", "echo AGENT_UP; exec cat"].iter().map(|s| s.to_string()).collect();
        let id = format!("t{}", std::process::id());
        let mut a = AgentProc::spawn_with(Backend::Tmux, id.clone(), "stub".into(), "m".into(),
            String::new(), dir.clone(), &argv, 24, 80).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(700));
        assert!(a.tmux.is_some());
        assert!(a.parser.lock().unwrap().screen().contents().contains("AGENT_UP"));
        a.send(b"typed-before\r");
        std::thread::sleep(std::time::Duration::from_millis(300));
        a.detach(); // crew quits
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(tmux_live(&session_name(&id)), "session must outlive the viewer");
        // next crew start: same id → attaches (argv ignored), history intact
        let mut b = AgentProc::spawn_with(Backend::Pty, id.clone(), "stub".into(), "m".into(),
            String::new(), dir, &["false".to_string()], 24, 80).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(700));
        assert!(b.tmux.is_some() && b.is_alive());
        let screen = b.parser.lock().unwrap().screen().contents();
        assert!(screen.contains("typed-before"), "reattached view lost history: {screen:?}");
        b.kill();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!tmux_live(&session_name(&id)));
        let last = b.parser.lock().unwrap().screen().contents();
        assert!(last.contains("typed-before") && !last.contains("[exited]"), "last frame lost: {last:?}");
        let _ = tmux_cmd().arg("kill-server").output();
    }

    #[test]
    fn pty_forwards_input() {
        // `cat` echoes stdin back to stdout — prove send() reaches the process
        // and the echoed bytes land on the vt100 screen.
        let dir = std::env::temp_dir();
        let argv = vec!["cat".to_string()];
        let mut a = AgentProc::spawn(
            "t2".into(), "stub".into(), "m".into(), String::new(),
            dir.to_string_lossy().to_string(), &argv, 24, 80,
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(200));
        a.send(b"PING_INPUT\r");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let screen = a.parser.lock().unwrap().screen().contents();
        assert!(screen.contains("PING_INPUT"), "input not echoed to screen: {:?}", screen);
        a.kill();
    }
}
