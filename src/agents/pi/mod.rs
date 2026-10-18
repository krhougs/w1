use super::profile::{AgentProfile, NativeSupport, Resume};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "pi",
    display_name: "Pi Coding Agent",
    commands: &["pi", "pi-coding-agent"],
    install_paths: &["@earendil-works/pi-coding-agent"],
    // Pi >= 1.1.0 reports everything itself.
    native: NativeSupport::Full,
    rules: include_str!("rules.toml"),
    resume: Resume::flags(&["--continue", "-c", "--resume", "-r", "--session"]),
    summary: "Pi Coding Agent >= 1.1.0 supports OSC 7501 natively",
    ..AgentProfile::DEFAULTS
};
