use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "zcode",
    display_name: "ZCode",
    commands: &["zcode", "ztui", "zcode-app-cli"],
    rules: include_str!("rules.toml"),
    ..AgentProfile::DEFAULTS
};
