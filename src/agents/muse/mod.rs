use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "muse",
    display_name: "Muse Code",
    commands: &["muse", "muse-code", "muse-cli"],
    versioned_command: Some("muse-bin-"),
    rules: include_str!("rules.toml"),
    ..AgentProfile::DEFAULTS
};
