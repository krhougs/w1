mod tracker;

use super::profile::{AgentProfile, NativeQuirks, NativeSupport, Resume};
use crate::stream::TitleFilter;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "codex",
    display_name: "OpenAI Codex CLI",
    commands: &["codex", "codex-cli"],
    install_paths: &["@openai/codex"],
    // Codex >= 0.163 reports idle/working/blocked only, without probing.
    native: NativeSupport::Partial,
    native_quirks: NativeQuirks {
        idle_for_done: true,
        ..NativeQuirks::NONE
    },
    rules: include_str!("rules.toml"),
    // Codex's titles name the directory rather than the session, so none are forwarded.
    titles: TitleFilter::Drop,
    resume: Resume {
        flags: &[],
        subcommands: &["resume", "fork"],
    },
    tracker: Some(|| Box::new(tracker::CodexTracker::default())),
    summary: "Native (>= 0.163) + w1: ~/.codex/state_5.sqlite threads (claimed by prompt time) + rollout JSONL",
    ..AgentProfile::DEFAULTS
};
