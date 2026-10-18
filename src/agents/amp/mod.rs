use super::profile::{AgentProfile, NativeSupport};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "amp",
    display_name: "Sourcegraph Amp",
    commands: &["amp", "amp-local"],
    install_paths: &["@sourcegraph/amp"],
    // Amp reports state/kind only.
    native: NativeSupport::Partial,
    rules: include_str!("rules.toml"),
    summary: "Native + w1: screen only (threads live server-side)",
    ..AgentProfile::DEFAULTS
};
