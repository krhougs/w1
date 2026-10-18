use super::profile::AgentProfile;

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "mastracode",
    display_name: "Mastra Code",
    commands: &["mastracode", "mastra-code"],
    rules: super::GENERIC_RULES,
    ..AgentProfile::DEFAULTS
};
