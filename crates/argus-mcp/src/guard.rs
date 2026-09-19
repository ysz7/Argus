//! What the agent may do: applications it may observe and act in, key
//! combinations it may press, whether it may act at all.

use argus_input::Combo;
use argus_protocol::Application;

/// Applications never observed or driven unless unblocked explicitly:
/// shells, credential stores, system settings, and AI chat clients and the
/// editors that host them (the agent must not drive its own conversation). Matched against the name or
/// the bundle identifier, case-insensitively.
pub const BLOCKED_APPS: &[&str] = &[
    "Terminal",
    "com.apple.Terminal",
    "iTerm2",
    "com.googlecode.iterm2",
    "Warp",
    "dev.warp.Warp-Stable",
    "Ghostty",
    "com.mitchellh.ghostty",
    "Alacritty",
    "kitty",
    "Keychain Access",
    "com.apple.keychainaccess",
    "Passwords",
    "com.apple.Passwords",
    "1Password",
    "com.1password.1password",
    "System Settings",
    "com.apple.systempreferences",
    "Script Editor",
    "com.apple.ScriptEditor2",
    "Automator",
    "com.apple.Automator",
    "Claude",
    "com.anthropic.claudefordesktop",
    "ChatGPT",
    "com.openai.chat",
    "Code",
    "com.microsoft.VSCode",
    "Cursor",
    "com.todesktop.230313mzl4w4u92",
];

/// Key combinations that leave the application, affect the session or the
/// whole system (switching apps and spaces, logging out, force quit,
/// screenshots, Spotlight).
const BLOCKED_KEYS: &[&[&str]] = &[
    &["cmd", "tab"],
    &["cmd", "`"],
    &["cmd", "space"],
    &["ctrl", "space"],
    &["cmd", "alt", "space"],
    &["cmd", "shift", "q"],
    &["cmd", "ctrl", "q"],
    &["cmd", "alt", "escape"],
    &["cmd", "h"],
    &["cmd", "alt", "h"],
    &["cmd", "m"],
    &["cmd", "alt", "d"],
    &["ctrl", "up"],
    &["ctrl", "down"],
    &["ctrl", "left"],
    &["ctrl", "right"],
    &["cmd", "shift", "3"],
    &["cmd", "shift", "4"],
    &["cmd", "shift", "5"],
    &["cmd", "ctrl", "f"],
];

/// Longest text typed by one action.
pub const MAX_TEXT: usize = 5000;

/// The policy of one MCP server.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Policy {
    /// Observe only: every action is refused.
    pub read_only: bool,
    /// If not empty, only these applications (names or bundle IDs).
    pub allowed_apps: Vec<String>,
    /// Entries of [`BLOCKED_APPS`] lifted by the user.
    pub unblocked_apps: Vec<String>,
}

fn same(a: &str, b: &str) -> bool {
    a.trim().eq_ignore_ascii_case(b.trim())
}

impl Policy {
    /// Why `application` may not be used, if it may not.
    pub fn refuse_app(&self, application: &Application) -> Option<String> {
        let names: Vec<&str> = [application.name.as_deref(), application.bundle_id.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        let label = names.first().copied().unwrap_or("this application");
        let listed = |list: &[&str]| names.iter().any(|name| list.iter().any(|x| same(x, name)));
        let unblocked: Vec<&str> = self.unblocked_apps.iter().map(String::as_str).collect();
        if listed(BLOCKED_APPS) && !listed(&unblocked) {
            return Some(format!(
                "{label} is blocked for agents (shells, passwords, settings and chat clients \
                 are); the user can lift this with `argus mcp --unblock-app`"
            ));
        }
        let allowed: Vec<&str> = self.allowed_apps.iter().map(String::as_str).collect();
        if !allowed.is_empty() && !listed(&allowed) {
            return Some(format!(
                "{label} is not among the applications this server may use: {}",
                allowed.join(", ")
            ));
        }
        None
    }

    /// Why actions are refused, if they are.
    pub fn refuse_actions(&self) -> Option<String> {
        self.read_only.then(|| {
            "this Argus server is read-only (`--read-only`): it observes but does not act"
                .to_owned()
        })
    }

    /// Why `combo` may not be pressed, if it may not.
    pub fn refuse_keys(&self, combo: &Combo) -> Option<String> {
        let mut pressed = combo.modifiers.names();
        pressed.push(&combo.key);
        let blocked = BLOCKED_KEYS.iter().any(|blocked| {
            blocked.len() == pressed.len() && blocked.iter().all(|name| pressed.contains(name))
        });
        blocked.then(|| {
            format!(
                "`{}` is not allowed: it could leave the application or affect the whole system",
                combo.canonical()
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use argus_input::parse_combo;

    use super::*;

    fn app(name: &str, bundle: &str) -> Application {
        Application { name: Some(name.into()), bundle_id: Some(bundle.into()), pid: Some(1) }
    }

    #[test]
    fn sensitive_applications_are_blocked_unless_unblocked() {
        let policy = Policy::default();
        assert!(policy.refuse_app(&app("Terminal", "com.apple.Terminal")).is_some());
        assert!(policy.refuse_app(&app("Claude", "com.anthropic.claudefordesktop")).is_some());
        assert!(policy.refuse_app(&app("Code", "com.microsoft.VSCode")).is_some());
        assert!(policy.refuse_app(&app("Calculator", "com.apple.calculator")).is_none());
        let unblocked = Policy { unblocked_apps: vec!["terminal".into()], ..Policy::default() };
        assert!(unblocked.refuse_app(&app("Terminal", "com.apple.Terminal")).is_none());
    }

    #[test]
    fn an_allow_list_restricts_applications() {
        let policy = Policy { allowed_apps: vec!["TextEdit".into()], ..Policy::default() };
        assert!(policy.refuse_app(&app("TextEdit", "com.apple.TextEdit")).is_none());
        assert!(policy.refuse_app(&app("Calculator", "com.apple.calculator")).is_some());
    }

    #[test]
    fn system_key_combinations_are_blocked() {
        let policy = Policy::default();
        let refused = |keys: &str| policy.refuse_keys(&parse_combo(keys).unwrap()).is_some();
        assert!(refused("cmd+tab"));
        assert!(refused("Cmd+Shift+Q"));
        assert!(refused("command+space"));
        assert!(refused("cmd+option+esc"));
        assert!(!refused("cmd+s"));
        assert!(!refused("cmd+shift+s"));
        assert!(!refused("Return"));
        assert!(!refused("shift+tab"));
    }

    #[test]
    fn read_only_refuses_actions() {
        assert!(Policy { read_only: true, ..Policy::default() }.refuse_actions().is_some());
        assert!(Policy::default().refuse_actions().is_none());
    }
}
