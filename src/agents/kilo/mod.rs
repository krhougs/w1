use super::profile::{AgentProfile, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "kilo",
    display_name: "Kilo Code",
    commands: &["kilo", "kilo-code", "kilocode"],
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--continue", "-c", "--session", "-s"]),
    tracker: Some(|| Box::new(super::opencode::tracker::OpenCodeTracker::kilo())),
    summary: "Screen + ~/.local/share/kilo (same store as OpenCode; untested)",
    ..AgentProfile::DEFAULTS
};
