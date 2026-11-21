//! Native telescope-style search: fuzzy file finder and live grep (ripgrep
//! underneath), with a preview of the selected hit. State lives in `Search`;
//! `app.rs` drives it, `ui.rs` draws it.

use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Kind {
    #[default]
    Files,
    Grep,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    pub path: String,
    /// 1-based; 1 for file-finder hits.
    pub line: usize,
    /// The matching line (grep) — empty for files.
    pub text: String,
}

#[derive(Debug, Default)]
pub struct Search {
    pub kind: Kind,
    pub root: String,
    pub query: String,
    /// Every candidate path (files mode); loaded once on open.
    pub candidates: Vec<String>,
    pub hits: Vec<Hit>,
    pub cursor: usize,
    /// Preview of the selected hit: lines + the 0-based index to highlight.
    pub preview: Vec<String>,
    pub preview_hl: usize,
    /// First preview line's number in the file (for the gutter).
    pub preview_from: usize,
    /// Mode to return to when closed.
    pub ret: crate::app::Mode,
    pub note: String,
}

const MAX_HITS: usize = 200;
const MAX_FILES: usize = 100_000;

fn rg_ok() -> bool {
    crate::app::on_path("rg")
}

/// All files under `root` (gitignore-aware): `rg --files`, else `git ls-files`.
pub fn list_files(root: &str) -> Vec<String> {
    let run = |prog: &str, args: &[&str]| {
        Command::new(prog).current_dir(root).args(args).output().ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().take(MAX_FILES).map(String::from).collect::<Vec<_>>())
    };
    let v = if rg_ok() { run("rg", &["--files", "--hidden", "-g", "!.git"]) } else { None };
    v.or_else(|| run("git", &["ls-files", "--cached", "--others", "--exclude-standard"])).unwrap_or_default()
}

/// Fuzzy score of `query` against `path` (None = no match). Smart-case
/// subsequence; consecutive runs, word starts and filename hits score higher.
pub fn fuzzy_score(query: &str, path: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(0);
    }
    let case = query.chars().any(|c| c.is_uppercase());
    let norm = |c: char| if case { c } else { c.to_ascii_lowercase() };
    let p: Vec<char> = path.chars().collect();
    let name_start = path.rfind('/').map(|i| path[..=i].chars().count()).unwrap_or(0);
    let (mut score, mut pi, mut prev) = (0i64, 0usize, None::<usize>);
    for q in query.chars().map(norm) {
        let found = (pi..p.len()).find(|&i| norm(p[i]) == q)?;
        score += 10;
        if prev == Some(found.wrapping_sub(1)) && found > 0 {
            score += 15; // consecutive
        }
        if found == 0 || matches!(p[found - 1], '/' | '_' | '-' | '.' | ' ') {
            score += 12; // word start
        }
        if found >= name_start {
            score += 8; // in the filename
        }
        prev = Some(found);
        pi = found + 1;
    }
    Some(score * 4 - p.len() as i64) // shorter paths win ties
}

/// Rank candidates against the query, best first.
pub fn rank(query: &str, candidates: &[String]) -> Vec<Hit> {
    let mut v: Vec<(i64, &String)> = candidates.iter().filter_map(|c| fuzzy_score(query, c).map(|s| (s, c))).collect();
    v.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    v.into_iter().take(MAX_HITS).map(|(_, p)| Hit { path: p.clone(), line: 1, text: String::new() }).collect()
}

/// Parse one `rg --no-heading --line-number --column` line: `path:line:col:text`.
pub fn parse_hit(l: &str) -> Option<Hit> {
    let mut it = l.splitn(4, ':');
    let path = it.next()?.to_string();
    let line = it.next()?.parse().ok()?;
    let _col = it.next()?;
    Some(Hit { path, line, text: it.next().unwrap_or("").trim().to_string() })
}

/// Live grep (smart-case regex). Needs ripgrep; returns an error note if absent.
pub fn grep(root: &str, query: &str) -> Result<Vec<Hit>, String> {
    if query.chars().count() < 2 {
        return Ok(vec![]);
    }
    if !rg_ok() {
        return Err("grep needs ripgrep: brew install ripgrep".into());
    }
    // ponytail: synchronous rg per keystroke; move to a thread if repos get huge
    let out = Command::new("rg")
        .current_dir(root)
        .args(["--no-heading", "--line-number", "--column", "--color", "never", "--smart-case",
               "--hidden", "-g", "!.git", "--max-count", "20", "--max-columns", "200", "-e", query, "--", "."])
        .output().map_err(|e| e.to_string())?;
    // exit 1 = no matches, 2 = bad regex (e.g. a half-typed pattern): both just show nothing
    Ok(String::from_utf8_lossy(&out.stdout).lines()
        .filter_map(|l| parse_hit(l.trim_start_matches("./")))
        .take(MAX_HITS).collect())
}

/// ±`ctx` lines around `hit`; returns (lines, index of the hit within them, first line number).
pub fn preview(root: &str, hit: &Hit, ctx: usize) -> (Vec<String>, usize, usize) {
    let bytes = match std::fs::read(Path::new(root).join(&hit.path)) {
        Ok(b) if !b.contains(&0) && b.len() < (1 << 20) => b,
        Ok(_) => return (vec!["(binary or large file)".into()], 0, 1),
        Err(_) => return (vec!["(unreadable)".into()], 0, 1),
    };
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    let hit_i = hit.line.saturating_sub(1).min(lines.len().saturating_sub(1));
    let (from, to) = if hit.text.is_empty() {
        (0, (ctx * 4).min(lines.len())) // file finder: the top of the file
    } else {
        (hit_i.saturating_sub(ctx), (hit_i + ctx + 1).min(lines.len()))
    };
    (lines[from..to].iter().map(|s| s.to_string()).collect(), if hit.text.is_empty() { usize::MAX } else { hit_i - from }, from + 1)
}

/// Command line to open `path` at `line` in `editor` (which may carry flags).
pub fn editor_argv(editor: &str, path: &str, line: usize) -> Vec<String> {
    let mut argv: Vec<String> = editor.split_whitespace().map(String::from).collect();
    let bin = argv.first().map(|b| b.rsplit('/').next().unwrap_or(b).to_string()).unwrap_or_default();
    match bin.as_str() {
        "nvim" | "vim" | "vi" | "nano" | "emacs" | "emacsclient" | "micro" | "kak" => {
            argv.push(format!("+{line}"));
            argv.push(path.into());
        }
        "hx" | "helix" | "zed" | "subl" => argv.push(format!("{path}:{line}")),
        "code" | "code-insiders" | "codium" | "cursor" | "windsurf" => {
            argv.push("-g".into());
            argv.push(format!("{path}:{line}"));
        }
        _ => argv.push(path.into()),
    }
    argv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_ranks_filename_and_consecutive_hits_first() {
        let c: Vec<String> = ["src/app.rs", "src/main.rs", "docs/approach.md", "README.md"].map(String::from).to_vec();
        let r = rank("app", &c);
        assert_eq!(r[0].path, "src/app.rs", "{r:?}"); // filename + shorter beats docs/approach.md
        assert!(rank("zzz", &c).is_empty());
        assert_eq!(rank("smr", &c)[0].path, "src/main.rs"); // subsequence s..m..r
        assert!(fuzzy_score("APP", "src/app.rs").is_none(), "smart-case: capitals are exact");
        assert_eq!(rank("", &c).len(), 4);
    }

    #[test]
    fn parses_rg_lines_and_keeps_colons_in_text() {
        let h = parse_hit("src/a.rs:12:5:let x: u8 = 1;").unwrap();
        assert_eq!((h.path.as_str(), h.line, h.text.as_str()), ("src/a.rs", 12, "let x: u8 = 1;"));
        assert!(parse_hit("garbage").is_none());
    }

    #[test]
    fn editor_argv_per_editor() {
        assert_eq!(editor_argv("nvim", "a/b.rs", 42), ["nvim", "+42", "a/b.rs"]);
        assert_eq!(editor_argv("/usr/bin/vim", "x", 1)[1], "+1");
        assert_eq!(editor_argv("hx", "x.rs", 7), ["hx", "x.rs:7"]);
        assert_eq!(editor_argv("code -w", "x.rs", 3), ["code", "-w", "-g", "x.rs:3"]);
        assert_eq!(editor_argv("mystery", "x", 9), ["mystery", "x"]);
    }

    #[test]
    fn preview_centres_on_the_hit() {
        let dir = std::env::temp_dir().join(format!("crew-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let body: String = (1..=30).map(|i| format!("line{i}\n")).collect();
        std::fs::write(dir.join("f.txt"), body).unwrap();
        let root = dir.to_string_lossy().to_string();
        let hit = Hit { path: "f.txt".into(), line: 10, text: "line10".into() };
        let (lines, at, from) = preview(&root, &hit, 3);
        assert_eq!((lines.len(), from), (7, 7));
        assert_eq!(lines[at], "line10");
        let (top, hl, from) = preview(&root, &Hit { path: "f.txt".into(), line: 1, text: String::new() }, 3);
        assert_eq!((top[0].as_str(), hl, from), ("line1", usize::MAX, 1)); // file finder: no highlight
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_and_grep_with_ripgrep() {
        if !crate::app::on_path("rg") {
            return; // CI without ripgrep
        }
        let dir = std::env::temp_dir().join(format!("crew-rg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub/a.rs"), "fn needle_here() {}\n").unwrap();
        std::fs::write(dir.join("b.txt"), "nothing\n").unwrap();
        let root = dir.to_string_lossy().to_string();
        let files = list_files(&root);
        assert!(files.contains(&"sub/a.rs".to_string()) && files.contains(&"b.txt".to_string()), "{files:?}");
        let hits = grep(&root, "needle").unwrap();
        assert_eq!(hits.len(), 1, "{hits:?}");
        assert_eq!((hits[0].path.as_str(), hits[0].line), ("sub/a.rs", 1));
        assert!(grep(&root, "n").unwrap().is_empty(), "1-char queries are skipped");
        assert!(grep(&root, "(unclosed").unwrap().is_empty(), "bad regex shows nothing, no error");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
