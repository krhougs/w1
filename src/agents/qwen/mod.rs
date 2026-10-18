mod tracker;

use super::profile::{AgentProfile, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "qwen",
    display_name: "Qwen Code",
    commands: &["qwen", "qwen-code"],
    install_paths: &["qwen-code"],
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--continue", "-c", "--resume", "-r"]),
    tracker: Some(|| Box::new(tracker::QwenTracker::default())),
    summary: "Screen/OSC title + ~/.qwen/sessions/<pid>.json + projects/*/chats JSONL",
    ..AgentProfile::DEFAULTS
};
