use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "goose",
    display_name: "Goose",
    commands: &["goose", "goose-cli"],
    rules: include_str!("rules.toml"),
    ..AgentProfile::DEFAULTS
};
