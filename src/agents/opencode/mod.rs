pub(crate) mod tracker;

use super::profile::{AgentProfile, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "opencode",
    display_name: "OpenCode",
    commands: &["opencode", "opencode2", "open-code"],
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--continue", "-c", "--session", "-s"]),
    tracker: Some(|| Box::new(tracker::OpenCodeTracker::opencode())),
    summary: "Screen + ~/.local/share/opencode/opencode.db (sessions/messages/todos/subagents)",
    ..AgentProfile::DEFAULTS
};
