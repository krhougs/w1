use super::profile::{AgentProfile, NativeSupport};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "claude-code",
    display_name: "Claude Code",
    commands: &["claude", "claude-code", "claudecode"],
    install_paths: &["@anthropic-ai/claude-code", "/claude/versions/"],
    // Claude Code >= 2.1.295 reports everything itself.
    native: NativeSupport::Full,
    rules: include_str!("rules.toml"),
    summary: "Claude Code >= 2.1.295 supports OSC 7501 natively",
    ..AgentProfile::DEFAULTS
};
