use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "maki",
    display_name: "Maki",
    commands: &["maki"],
    rules: include_str!("rules.toml"),
    ..AgentProfile::DEFAULTS
};
