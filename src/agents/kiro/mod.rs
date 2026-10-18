use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "kiro",
    display_name: "Kiro CLI",
    commands: &["kiro", "kiro-cli"],
    rules: include_str!("rules.toml"),
    ..AgentProfile::DEFAULTS
};
