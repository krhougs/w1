mod tracker;

use super::profile::{AgentProfile, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "copilot",
    display_name: "GitHub Copilot CLI",
    commands: &["copilot", "github-copilot", "ghcs"],
    install_paths: &["@github/copilot"],
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--resume", "--continue"]),
    tracker: Some(|| Box::new(tracker::CopilotTracker::default())),
    summary: "Screen + ~/.copilot/session-state events.jsonl (inuse.<pid>.lock)",
    ..AgentProfile::DEFAULTS
};
