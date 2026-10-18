mod tracker;

use super::profile::{AgentProfile, Resume};
use crate::stream::TitleFilter;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "codex",
    display_name: "OpenAI Codex CLI",
    commands: &["codex", "codex-cli"],
    install_paths: &["@openai/codex"],
    rules: include_str!("rules.toml"),
    // Codex's titles name the directory rather than the session, so none are forwarded.
    titles: TitleFilter::Drop,
    resume: Resume {
        flags: &[],
        subcommands: &["resume", "fork"],
    },
    tracker: Some(|| Box::new(tracker::CodexTracker::default())),
    summary: "Screen + ~/.codex/state_5.sqlite threads (claimed by prompt time) + rollout JSONL",
    ..AgentProfile::DEFAULTS
};
