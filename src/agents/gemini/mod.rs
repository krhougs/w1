mod tracker;

use super::profile::{AgentProfile, Hooks, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "gemini",
    display_name: "Gemini CLI",
    commands: &["gemini", "gemini-cli"],
    install_paths: &["@google/gemini-cli", "/gemini-cli/"],
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--resume", "-r"]),
    tracker: Some(|| Box::new(tracker::GeminiTracker::default())),
    hooks: Hooks {
        working_detail: Some(working_detail),
        ..Hooks::NONE
    },
    summary: "Screen/OSC title + $GEMINI_CLI_HOME/.gemini/tmp/<project>/chats JSONL",
    ..AgentProfile::DEFAULTS
};

/// The activity in a `✦ <activity> (<dir>)` title.
fn working_detail(osc_title: &str) -> Option<String> {
    let s = osc_title.trim().strip_prefix('✦')?.trim();
    let s = s.rfind(" (").map_or(s, |idx| s[..idx].trim());
    if s.is_empty() || s == "Working…" || s == "Working..." {
        return None;
    }
    crate::osc7501::sanitize_single_line(s, 240)
}
