mod tracker;

use super::profile::{AgentProfile, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "cursor",
    display_name: "Cursor Agent",
    commands: &["cursor", "cursor-agent", "agent"],
    install_paths: &["cursor-agent"],
    rules: include_str!("rules.toml"),
    resume: Resume {
        flags: &["--resume", "--continue"],
        subcommands: &["resume"],
    },
    tracker: Some(|| Box::new(tracker::CursorTracker::default())),
    summary: "Screen + ~/.cursor chats meta.json + agent-transcripts (turns untested)",
    ..AgentProfile::DEFAULTS
};
