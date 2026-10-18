use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "cline",
    display_name: "Cline",
    commands: &["cline", ".cline"],
    rules: include_str!("rules.toml"),
    ..AgentProfile::DEFAULTS
};
