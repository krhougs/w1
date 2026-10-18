mod tracker;

use super::profile::{AgentProfile, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "kimi",
    display_name: "Kimi Code",
    commands: &["kimi", "kimi-code", "kimi-cli", "kimi-legacy"],
    install_paths: &["kimi-cli", "kimi-code"],
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--continue", "-c", "--resume", "-r", "--session", "-S"]),
    tracker: Some(|| Box::new(tracker::KimiTracker::default())),
    summary: "Screen + $KIMI_CODE_HOME/sessions/wd_*/ state.json & wire.jsonl",
    ..AgentProfile::DEFAULTS
};
