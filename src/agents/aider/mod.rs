use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "aider",
    display_name: "Aider",
    commands: &["aider", "aider-chat"],
    rules: include_str!("rules.toml"),
    ..AgentProfile::DEFAULTS
};
