mod tracker;

use std::time::Duration;

use super::profile::{AgentProfile, Hooks, NativeQuirks, NativeSupport, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "antigravity",
    display_name: "Antigravity CLI",
    commands: &["agy", "antigravity", "antigravity-cli"],
    // agy >= 1.4.0 reports no title.
    native: NativeSupport::Partial,
    native_quirks: NativeQuirks {
        // agy reports `done` when a permission is declined and replaces it with `idle` a frame
        // later.
        settle: Some(Duration::from_millis(300)),
        // agy reports `blocked` with no dialog on screen: `auth` while it signs in at startup,
        // and `Approve plan` from the moment it writes an artifact until the next prompt, even
        // after the turn has ended.
        blocked_needs_screen: true,
    },
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--continue", "-c", "--conversation"]),
    tracker: Some(|| Box::new(tracker::AgyTracker::default())),
    hooks: Hooks {
        blocked_detail: Some(blocked_detail),
        ..Hooks::NONE
    },
    summary: "Native (blocked only with a dialog on screen) + w1: agy log (by pid) + conversation databases",
    ..AgentProfile::DEFAULTS
};

fn blocked_detail(rule_id: &str, lines: &[&str]) -> Option<String> {
    if rule_id == "login_prompt" {
        return Some("Sign in required".to_owned());
    }
    // `Requesting permission for:` followed by the indented command.
    let cmd = lines
        .iter()
        .rposition(|l| l.eq_ignore_ascii_case("requesting permission for:"))
        .and_then(|i| lines.get(i + 1))?;
    crate::osc7501::sanitize_single_line(&format!("$ {cmd}"), 240)
}
