use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "letta",
    display_name: "Letta Code",
    commands: &["letta", "letta-code"],
    rules: include_str!("rules.toml"),
    ..AgentProfile::DEFAULTS
};
