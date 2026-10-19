use super::profile::{AgentProfile, Hooks};

pub(crate) const PROFILE: AgentProfile = AgentProfile {
    app_id: "zcode",
    display_name: "ZCode",
    commands: &["zcode", "ztui", "zcode-app-cli"],
    rules: include_str!("rules.toml"),
    hooks: Hooks {
        blocked_detail: Some(blocked_detail),
        ..Hooks::NONE
    },
    ..AgentProfile::DEFAULTS
};

/// `Permission · <tool>` heads the panel; the request follows its `Risk:` line.
fn blocked_detail(rule_id: &str, lines: &[&str]) -> Option<String> {
    if rule_id != "permission_panel" {
        return None;
    }
    let mut rest = lines.iter().skip_while(|l| !l.starts_with("Permission · "));
    let tool = rest.next()?.trim_start_matches("Permission · ").trim();
    let target = rest
        .skip_while(|l| !l.starts_with("Risk:"))
        .nth(1)
        .filter(|l| !l.starts_with("Filter:"))
        .map_or("", |l| l.trim());
    crate::osc7501::sanitize_single_line(&format!("{tool} {target}"), 240)
}
