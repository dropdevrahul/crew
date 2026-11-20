//! The global workspace list — Cursor-style pinned folders, persisted at
//! `~/.config/crew/workspaces.json` (honouring `$XDG_CONFIG_HOME`). Each pinned
//! folder becomes a `Workspace` in the running app; the list and the active
//! selection survive restarts so you reopen where you left off.

use crate::app::Workspace;
use crate::store::{git_toplevel, Store};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// One pinned folder (the folder itself is the identity; the name is just for
/// display).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceEntry {
    pub path: String,
    pub name: String,
}

/// The persisted global config: which folders are pinned + which is active.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceEntry>,
    /// Where agents run: "auto" (tmux if installed), "tmux", or "pty".
    #[serde(default)]
    pub backend: String,
    /// Color theme name (see theme.rs); empty = terminal default.
    #[serde(default)]
    pub theme: String,
}

/// Resolve the global config path: `$XDG_CONFIG_HOME/crew/workspaces.json`,
/// falling back to `~/.config/crew/workspaces.json`.
pub fn global_config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("crew").join("workspaces.json")
}

/// Load the global config (defaults if absent/unparseable).
pub fn load_global() -> GlobalConfig {
    std::fs::read_to_string(global_config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the global config (best-effort).
pub fn save_global(cfg: &GlobalConfig) {
    let path = global_config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(j) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, j);
    }
}

/// The scripts dir for a workspace: `$CREW_SCRIPTS` if set, else `<root>/scripts`.
fn scripts_for(root: &Path) -> PathBuf {
    std::env::var("CREW_SCRIPTS")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("scripts"))
}

/// Build the `Store` for a folder. The store is rooted at the folder's git
/// toplevel when it is inside a repo (so worktrees/branches work), otherwise at
/// the folder itself (crew still manages agents, just no worktree isolation).
pub fn store_for_path(path: &Path, scripts_override: Option<&Path>) -> Store {
    let root = git_toplevel(path);
    let scripts = scripts_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| scripts_for(&root));
    Store::new(root, scripts)
}

/// Build a `Workspace` from a folder (see [`store_for_path`]).
pub fn workspace_from_path(path: &Path, scripts_override: Option<&Path>) -> Workspace {
    Workspace::new(store_for_path(path, scripts_override))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_path_honours_xdg() {
        std::env::set_var("XDG_CONFIG_HOME", "/tmp/xdg-crew");
        assert_eq!(global_config_path(), PathBuf::from("/tmp/xdg-crew/crew/workspaces.json"));
        std::env::remove_var("XDG_CONFIG_HOME");
    }

    #[test]
    fn config_round_trips() {
        let tmp = std::env::temp_dir().join(format!("crew-wscfg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::env::set_var("XDG_CONFIG_HOME", &tmp);
        let cfg = GlobalConfig {
            version: 1,
            backend: "pty".into(),
            theme: "nord".into(),
            active: 1,
            workspaces: vec![
                WorkspaceEntry { path: "/a".into(), name: "a".into() },
                WorkspaceEntry { path: "/b".into(), name: "b".into() },
            ],
        };
        save_global(&cfg);
        let back = load_global();
        assert_eq!(back.active, 1);
        assert_eq!(back.workspaces.len(), 2);
        assert_eq!(back.workspaces[1].name, "b");
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
