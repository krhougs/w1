use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "qodercli",
    display_name: "Qoder CLI",
    commands: &["qodercli", "qoderclicn", "qoder", "qodercn"],
    rules: include_str!("rules.toml"),
    ..AgentProfile::DEFAULTS
};
