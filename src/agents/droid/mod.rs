mod tracker;

use super::profile::{AgentProfile, NativeSupport, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "droid",
    display_name: "Factory Droid",
    commands: &["droid", "factory-droid"],
    // Droid >= 0.236 reports state/kind only.
    native: NativeSupport::Partial,
    rules: include_str!("rules.toml"),
    resume: Resume {
        flags: &["--resume", "-r", "--fork"],
        subcommands: &["resume"],
    },
    tracker: Some(|| Box::new(tracker::DroidTracker::default())),
    summary: "Native + w1: ~/.factory/sessions/<cwd> JSONL (title/summary/todos/errors)",
    ..AgentProfile::DEFAULTS
};
