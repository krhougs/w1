use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "hermes",
    display_name: "Hermes Agent",
    commands: &["hermes", "hermes-agent"],
    rules: include_str!("rules.toml"),
    ..AgentProfile::DEFAULTS
};
