mod tracker;

use super::profile::{AgentProfile, NativeSupport, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "grok-build",
    display_name: "Grok Build",
    commands: &["grok", "grok-build"],
    // Grok Build >= 1.0.52 reports natively but stays silent inside tmux.
    native: NativeSupport::Partial,
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--resume", "-r", "--continue", "-c"]),
    tracker: Some(|| Box::new(tracker::GrokTracker::default())),
    summary: "Native + w1: active_sessions.json (by pid) + events/updates JSONL",
    ..AgentProfile::DEFAULTS
};
