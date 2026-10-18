mod tracker;

use super::profile::{AgentProfile, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "devin",
    display_name: "Devin CLI",
    commands: &["devin", "devin-cli"],
    install_paths: &["/devin/cli/"],
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--resume", "-r", "--continue", "-c"]),
    tracker: Some(|| Box::new(tracker::DevinTracker::default())),
    summary: "Screen + ~/.local/share/devin/cli sessions.db (session_locks by pid)",
    ..AgentProfile::DEFAULTS
};
