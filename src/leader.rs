//! Leader-key bindings: press the leader (dashboard) or chat key (in a chat),
//! then one more key to run an `Action`. Configurable via the `leader` object
//! in `~/.config/crew/workspaces.json`; every field falls back to its default.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Back,
    Agents,
    NewAgent,
    Diff,
    Push,
    Stop,
    Zoom,
    Help,
    Settings,
    Review,
    Grep,
    Files,
}

impl Action {
    pub fn from_name(s: &str) -> Option<Action> {
        Some(match s {
            "back" => Action::Back,
            "agents" => Action::Agents,
            "new_agent" => Action::NewAgent,
            "diff" => Action::Diff,
            "push" => Action::Push,
            "stop" => Action::Stop,
            "zoom" => Action::Zoom,
            "help" => Action::Help,
            "settings" => Action::Settings,
            "review" => Action::Review,
            "grep" => Action::Grep,
            "files" => Action::Files,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Action::Back => "back",
            Action::Agents => "agents",
            Action::NewAgent => "new_agent",
            Action::Diff => "diff",
            Action::Push => "push",
            Action::Stop => "stop",
            Action::Zoom => "zoom",
            Action::Help => "help",
            Action::Settings => "settings",
            Action::Review => "review",
            Action::Grep => "grep",
            Action::Files => "files",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Leader {
    /// Leader key on the dashboard.
    #[serde(default = "default_key")]
    pub key: String,
    /// Leader key inside a chat (everything else goes to the agent).
    #[serde(default = "default_chat_key")]
    pub chat_key: String,
    /// Second key → action.
    #[serde(default = "default_bindings")]
    pub bindings: Vec<(String, Action)>,
}

fn default_key() -> String {
    "ctrl-q".into()
}
fn default_chat_key() -> String {
    "ctrl-q".into()
}
fn default_bindings() -> Vec<(String, Action)> {
    [
        ("q", Action::Back),
        ("tab", Action::Agents),
        ("n", Action::NewAgent),
        ("d", Action::Diff),
        ("P", Action::Push),
        ("s", Action::Stop),
        ("z", Action::Zoom),
        ("?", Action::Help),
        (",", Action::Settings),
        ("v", Action::Review),
        ("/", Action::Grep),
        ("f", Action::Files),
    ]
    .into_iter()
    .map(|(k, a)| (k.to_string(), a))
    .collect()
}

impl Default for Leader {
    fn default() -> Self {
        Leader { key: default_key(), chat_key: default_chat_key(), bindings: default_bindings() }
    }
}

impl Leader {
    /// Bind actions added in newer versions that a saved config doesn't
    /// mention, unless their default key is already taken by the user.
    pub fn add_missing_defaults(&mut self) {
        for (key, action) in default_bindings() {
            let has_action = self.bindings.iter().any(|(_, a)| *a == action);
            let key_taken = self.bindings.iter().any(|(k, _)| *k == key);
            if !has_action && !key_taken {
                self.bindings.push((key, action));
            }
        }
    }

    /// The action bound to `key`, if any.
    pub fn lookup(&self, key: &KeyEvent) -> Option<Action> {
        self.bindings.iter().find(|(spec, _)| key_matches(spec, key)).map(|(_, a)| *a)
    }
}

/// Does `key` match a spec: "space", "tab", "enter", "esc", "ctrl-<c>" or a
/// single char. Ctrl+Space arrives as `Char(' ')` + CONTROL.
pub fn key_matches(spec: &str, key: &KeyEvent) -> bool {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let plain = !ctrl && !key.modifiers.contains(KeyModifiers::ALT);
    if let Some(rest) = spec.strip_prefix("ctrl-") {
        let c = if rest == "space" { Some(' ') } else { single(rest) };
        return ctrl && c.is_some_and(|c| key.code == KeyCode::Char(c.to_ascii_lowercase()));
    }
    match spec {
        "space" => plain && key.code == KeyCode::Char(' '),
        "tab" => plain && key.code == KeyCode::Tab,
        "enter" => plain && key.code == KeyCode::Enter,
        "esc" => key.code == KeyCode::Esc,
        _ => plain && single(spec).is_some_and(|c| key.code == KeyCode::Char(c)),
    }
}

fn single(s: &str) -> Option<char> {
    let mut it = s.chars();
    match (it.next(), it.next()) {
        (Some(c), None) => Some(c),
        _ => None,
    }
}

/// A key event as a spec string (inverse of `key_matches`).
pub fn key_label(key: &KeyEvent) -> String {
    let base = match key.code {
        KeyCode::Char(' ') => "space".to_string(),
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Tab => "tab".into(),
        KeyCode::Enter => "enter".into(),
        KeyCode::Esc => "esc".into(),
        other => format!("{:?}", other).to_lowercase(),
    };
    if key.modifiers.contains(KeyModifiers::CONTROL) { format!("ctrl-{}", base) } else { base }
}

/// A spec for display: "ctrl-space" → "Ctrl-Space", "tab" → "Tab", "q" → "q".
pub fn spec_label(spec: &str) -> String {
    spec.split('-')
        .map(|p| if p.chars().count() > 1 { p[..1].to_uppercase() + &p[1..] } else { p.to_string() })
        .collect::<Vec<_>>()
        .join("-")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(c: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(c, m)
    }

    #[test]
    fn matches_specs() {
        let none = KeyModifiers::empty();
        assert!(key_matches("space", &k(KeyCode::Char(' '), none)));
        assert!(!key_matches("space", &k(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert!(key_matches("ctrl-space", &k(KeyCode::Char(' '), KeyModifiers::CONTROL)));
        assert!(!key_matches("ctrl-space", &k(KeyCode::Char(' '), none)));
        assert!(key_matches("tab", &k(KeyCode::Tab, none)));
        assert!(key_matches("ctrl-q", &k(KeyCode::Char('q'), KeyModifiers::CONTROL)));
        assert!(key_matches("q", &k(KeyCode::Char('q'), none)));
        assert!(!key_matches("q", &k(KeyCode::Char('q'), KeyModifiers::CONTROL)));
        assert!(key_matches("P", &k(KeyCode::Char('P'), KeyModifiers::SHIFT)));
        assert!(!key_matches("p", &k(KeyCode::Char('P'), KeyModifiers::SHIFT)));
        assert_eq!(key_label(&k(KeyCode::Char(' '), KeyModifiers::CONTROL)), "ctrl-space");
        assert_eq!(spec_label("ctrl-space"), "Ctrl-Space");
    }

    #[test]
    fn default_bindings_resolve() {
        let l = Leader::default();
        assert_eq!(l.lookup(&k(KeyCode::Char('q'), KeyModifiers::empty())), Some(Action::Back));
        assert_eq!(l.lookup(&k(KeyCode::Tab, KeyModifiers::empty())), Some(Action::Agents));
        assert_eq!(l.lookup(&k(KeyCode::Char('x'), KeyModifiers::empty())), None);
        assert_eq!(Action::from_name(Action::NewAgent.name()), Some(Action::NewAgent));
    }

    #[test]
    fn saved_bindings_gain_new_default_actions() {
        let mut l: Leader = serde_json::from_str(r#"{"bindings":[["q","back"],["f","diff"]]}"#).unwrap();
        l.add_missing_defaults();
        assert_eq!(l.lookup(&k(KeyCode::Char('f'), KeyModifiers::empty())), Some(Action::Diff), "user's key wins");
        assert_eq!(l.lookup(&k(KeyCode::Char('/'), KeyModifiers::empty())), Some(Action::Grep), "new action added");
        assert!(!l.bindings.iter().any(|(_, a)| *a == Action::Files), "its default key is taken: skipped");
    }

    #[test]
    fn default_leader_is_not_persisted() {
        // a saved copy of the defaults would freeze them and hide new bindings
        let cfg = crate::workspace::GlobalConfig::default();
        assert!(!serde_json::to_string(&cfg).unwrap().contains("leader"));
    }

    #[test]
    fn partial_config_loads() {
        let l: Leader = serde_json::from_str(r#"{"key":"ctrl-g"}"#).unwrap();
        assert_eq!(l.key, "ctrl-g");
        assert_eq!(l.chat_key, "ctrl-q");
        assert_eq!(l.bindings.len(), 12);
    }
}
