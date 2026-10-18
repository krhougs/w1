mod tracker;

use super::profile::{AgentProfile, NativeSupport, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "crush",
    display_name: "Crush",
    commands: &["crush"],
    // Crush >= 0.98 reports state/kind only, and errors as idle.
    native: NativeSupport::Partial,
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--continue", "-C", "--session", "-s"]),
    tracker: Some(|| Box::new(tracker::CrushTracker::default())),
    summary: "Native + w1: <data dir>/crush.db (title/summary/todos/errors)",
    ..AgentProfile::DEFAULTS
};
