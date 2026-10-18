use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use super::message::extract_screen_detail;
use super::regions::{bottom_non_empty_lines, select_region};
use crate::agents::{AgentKind, GENERIC_RULES};
use crate::osc7501::BlockedKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetectedState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenDetection {
    pub state: DetectedState,
    pub blocked_kind: Option<BlockedKind>,
    pub skip_state_update: bool,
    pub visible_idle: bool,
    pub visible_blocker: bool,
    pub visible_working: bool,
    pub matched_rule_id: Option<String>,
    pub detail: Option<String>,
}

impl Default for ScreenDetection {
    fn default() -> Self {
        Self {
            state: DetectedState::Unknown,
            blocked_kind: None,
            skip_state_update: false,
            visible_idle: false,
            visible_blocker: false,
            visible_working: false,
            matched_rule_id: None,
            detail: None,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct DetectionInput<'a> {
    pub screen: &'a str,
    pub osc_title: &'a str,
    pub osc_progress: &'a str,
}

#[derive(Debug, Deserialize, Clone)]
struct AgentManifest {
    #[serde(default)]
    rules: Vec<ManifestRule>,
}

#[derive(Debug, Deserialize, Clone)]
struct ManifestRule {
    id: String,
    #[serde(default)]
    state: Option<ManifestState>,
    priority: i32,
    #[serde(default = "default_region")]
    region: String,
    #[serde(default)]
    skip_state_update: bool,
    #[serde(default)]
    visible_idle: bool,
    #[serde(default)]
    visible_blocker: bool,
    #[serde(default)]
    visible_working: bool,
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
    #[serde(default)]
    line_regex: Vec<String>,
    #[serde(default)]
    all: Vec<ManifestGate>,
    #[serde(default)]
    any: Vec<ManifestGate>,
    #[serde(default, rename = "not")]
    not_gate: Vec<ManifestGate>,
}

#[derive(Debug, Deserialize, Clone)]
struct ManifestGate {
    #[serde(default)]
    contains: Vec<String>,
    #[serde(default)]
    regex: Vec<String>,
    #[serde(default)]
    line_regex: Vec<String>,
    #[serde(default)]
    all: Vec<ManifestGate>,
    #[serde(default)]
    any: Vec<ManifestGate>,
    #[serde(default, rename = "not")]
    not_gate: Vec<ManifestGate>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ManifestState {
    Idle,
    Working,
    Blocked,
    Unknown,
}

impl From<ManifestState> for DetectedState {
    fn from(s: ManifestState) -> Self {
        match s {
            ManifestState::Idle => Self::Idle,
            ManifestState::Working => Self::Working,
            ManifestState::Blocked => Self::Blocked,
            ManifestState::Unknown => Self::Unknown,
        }
    }
}

fn default_region() -> String {
    "whole_recent".to_owned()
}

struct CompiledManifest {
    rules: Vec<(ManifestRule, CompiledGate)>,
}

struct CompiledGate {
    contains: Vec<String>,
    regex: Vec<Regex>,
    line_regex: Vec<Regex>,
    all: Vec<CompiledGate>,
    any: Vec<CompiledGate>,
    not_gate: Vec<CompiledGate>,
}

static MANIFESTS: LazyLock<HashMap<AgentKind, CompiledManifest>> = LazyLock::new(|| {
    AgentKind::ALL
        .iter()
        .filter_map(|&kind| Some((kind, parse_manifest(kind.profile().rules)?)))
        .collect()
});

static GENERIC: LazyLock<CompiledManifest> = LazyLock::new(|| {
    parse_manifest(GENERIC_RULES).unwrap_or(CompiledManifest { rules: Vec::new() })
});

fn parse_manifest(raw_toml: &str) -> Option<CompiledManifest> {
    let parsed = toml::from_str::<AgentManifest>(raw_toml).ok()?;
    compile_manifest(&parsed).ok()
}

fn compile_manifest(manifest: &AgentManifest) -> Result<CompiledManifest, String> {
    let mut rules = Vec::with_capacity(manifest.rules.len());
    for rule in &manifest.rules {
        let gate = ManifestGate {
            contains: rule.contains.clone(),
            regex: rule.regex.clone(),
            line_regex: rule.line_regex.clone(),
            all: rule.all.clone(),
            any: rule.any.clone(),
            not_gate: rule.not_gate.clone(),
        };
        let compiled = compile_gate(&gate)?;
        rules.push((rule.clone(), compiled));
    }
    Ok(CompiledManifest { rules })
}

fn compile_gate(gate: &ManifestGate) -> Result<CompiledGate, String> {
    Ok(CompiledGate {
        contains: gate.contains.iter().map(|s| s.to_lowercase()).collect(),
        regex: gate
            .regex
            .iter()
            .map(|p| Regex::new(p).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?,
        line_regex: gate
            .line_regex
            .iter()
            .map(|p| Regex::new(p).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?,
        all: gate
            .all
            .iter()
            .map(compile_gate)
            .collect::<Result<_, _>>()?,
        any: gate
            .any
            .iter()
            .map(compile_gate)
            .collect::<Result<_, _>>()?,
        not_gate: gate
            .not_gate
            .iter()
            .map(compile_gate)
            .collect::<Result<_, _>>()?,
    })
}

fn gate_matches(gate: &CompiledGate, text: &str, lower_text: &str) -> bool {
    if !gate.contains.iter().all(|needle| lower_text.contains(needle)) {
        return false;
    }
    if !gate.regex.iter().all(|re| re.is_match(text)) {
        return false;
    }
    if !gate
        .line_regex
        .iter()
        .all(|re| text.lines().any(|line| re.is_match(line)))
    {
        return false;
    }
    if !gate
        .all
        .iter()
        .all(|nested| gate_matches(nested, text, lower_text))
    {
        return false;
    }
    if !gate.any.is_empty()
        && !gate
            .any
            .iter()
            .any(|nested| gate_matches(nested, text, lower_text))
    {
        return false;
    }
    if gate
        .not_gate
        .iter()
        .any(|nested| gate_matches(nested, text, lower_text))
    {
        return false;
    }
    true
}

pub fn detect(agent: Option<AgentKind>, input: DetectionInput<'_>) -> ScreenDetection {
    let compiled = agent.and_then(|a| MANIFESTS.get(&a)).unwrap_or(&GENERIC);
    let mut best: Option<&ManifestRule> = None;
    for (rule, gate) in &compiled.rules {
        let region_text = select_region(input, &rule.region);
        let lower = region_text.to_lowercase();
        if !gate_matches(gate, region_text, &lower) {
            continue;
        }
        match best {
            Some(prev) if prev.priority >= rule.priority => {}
            _ => best = Some(rule),
        }
    }

    if let Some(rule) = best {
        let state = rule
            .state
            .map(DetectedState::from)
            .unwrap_or(DetectedState::Unknown);
        let blocked_kind = (state == DetectedState::Blocked)
            .then(|| classify_blocked_kind(&rule.id, input.screen, input.osc_title));
        let detail = extract_screen_detail(agent, state, &rule.id, input);

        return ScreenDetection {
            state,
            blocked_kind,
            skip_state_update: rule.skip_state_update,
            visible_idle: rule.visible_idle && state == DetectedState::Idle,
            visible_blocker: rule.visible_blocker && state == DetectedState::Blocked,
            visible_working: rule.visible_working && state == DetectedState::Working,
            matched_rule_id: Some(rule.id.clone()),
            detail,
        };
    }

    fallback_detection(agent, input)
}

fn fallback_detection(agent: Option<AgentKind>, input: DetectionInput<'_>) -> ScreenDetection {
    // ConEmu progress: 1 = value, 3 = indeterminate (busy).
    if input.osc_progress.starts_with("4;1") || input.osc_progress.starts_with("4;3") {
        return ScreenDetection {
            state: DetectedState::Working,
            visible_working: true,
            detail: extract_screen_detail(agent, DetectedState::Working, "osc_progress", input),
            ..Default::default()
        };
    }
    if agent.is_some() {
        ScreenDetection {
            state: DetectedState::Idle,
            ..Default::default()
        }
    } else {
        ScreenDetection::default()
    }
}

pub fn classify_blocked_kind(rule_id: &str, screen: &str, osc_title: &str) -> BlockedKind {
    let rule_lower = rule_id.to_lowercase();
    if rule_lower.contains("auth") || rule_lower.contains("credential") || rule_lower.contains("login") {
        return BlockedKind::Auth;
    }
    if rule_lower.contains("question")
        || rule_lower.contains("clarification")
        || rule_lower.contains("pick_request")
        || rule_lower.contains("elicitation")
    {
        return BlockedKind::Question;
    }
    if rule_lower.contains("permission")
        || rule_lower.contains("approval")
        || rule_lower.contains("trust")
        || rule_lower.contains("allow")
    {
        return BlockedKind::Permission;
    }

    let bottom = bottom_non_empty_lines(screen, 16).to_lowercase();
    let title_lower = osc_title.to_lowercase();
    if bottom.contains("sign in")
        || bottom.contains("login")
        || bottom.contains("api key")
        || bottom.contains("oauth")
        || bottom.contains("authentication")
        || bottom.contains("sudo password")
        || title_lower.contains("login")
    {
        return BlockedKind::Auth;
    }
    if bottom.contains("answer questions")
        || bottom.contains("ready to submit your answers?")
        || (bottom.contains("enter submit") && bottom.contains("esc dismiss"))
        || bottom.contains("enter to submit answer")
        || bottom.contains("enter to submit all")
        || bottom.contains("type your answer")
        || bottom.contains("asking a question")
        || bottom.contains("requests your input")
        || bottom.contains("esc:unselect")
        || bottom.contains("esc skip")
    {
        return BlockedKind::Question;
    }
    BlockedKind::Permission
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_bundled_manifests_compile() {
        for &kind in AgentKind::ALL {
            assert!(
                MANIFESTS.contains_key(&kind),
                "rules of {kind:?} failed to compile"
            );
        }
        assert!(!GENERIC.rules.is_empty(), "generic rules failed to compile");
    }

    #[test]
    fn codex_detects_working_and_blocked() {
        let det = detect(
            Some(AgentKind::Codex),
            DetectionInput {
                screen: "• Running cargo test (4s • esc to interrupt)\n› ",
                osc_title: "⠋ codex",
                osc_progress: "",
            },
        );
        assert_eq!(det.state, DetectedState::Working);

        let det_blocked = detect(
            Some(AgentKind::Codex),
            DetectionInput {
                screen: "› \nAllow command? `rm -rf build`\nPress enter to confirm or esc to cancel",
                osc_title: "Action Required - codex",
                osc_progress: "",
            },
        );
        assert_eq!(det_blocked.state, DetectedState::Blocked);
        assert_eq!(det_blocked.blocked_kind, Some(BlockedKind::Permission));
    }

    #[test]
    fn gemini_detects_working_blocked_idle_from_title_and_screen() {
        let det_work = detect(
            Some(AgentKind::Gemini),
            DetectionInput {
                screen: "Analyzing files... (esc to cancel)",
                osc_title: "✦  Inspecting Cargo.toml (wrap7501)",
                osc_progress: "",
            },
        );
        assert_eq!(det_work.state, DetectedState::Working);
        assert_eq!(det_work.detail.as_deref(), Some("Inspecting Cargo.toml"));

        let det_blocked = detect(
            Some(AgentKind::Gemini),
            DetectionInput {
                screen: "│ Allow execution of `cargo test`?\n❯ Yes, allow",
                osc_title: "✋  Action Required (wrap7501)",
                osc_progress: "",
            },
        );
        assert_eq!(det_blocked.state, DetectedState::Blocked);
        assert_eq!(det_blocked.blocked_kind, Some(BlockedKind::Permission));

        let det_idle = detect(
            Some(AgentKind::Gemini),
            DetectionInput {
                screen: "> ",
                osc_title: "◇  Ready (wrap7501)",
                osc_progress: "",
            },
        );
        assert_eq!(det_idle.state, DetectedState::Idle);
    }

    #[test]
    fn detects_opencode_grok_qwen_muse_kiro_aider_goose() {
        // OpenCode permission blocker & working
        let oc_block = detect(
            Some(AgentKind::OpenCode),
            DetectionInput {
                screen: "△ Permission required\nRun `rm -rf dist`?\n↑↓ select  enter confirm  esc dismiss",
                osc_title: "",
                osc_progress: "",
            },
        );
        assert_eq!(oc_block.state, DetectedState::Blocked);
        assert_eq!(oc_block.blocked_kind, Some(BlockedKind::Permission));

        let oc_work = detect(
            Some(AgentKind::OpenCode),
            DetectionInput {
                screen: "Thinking... press esc to interrupt",
                osc_title: "",
                osc_progress: "",
            },
        );
        assert_eq!(oc_work.state, DetectedState::Working);

        // Grok Build working & question dialog
        let grok_work = detect(
            Some(AgentKind::Grok),
            DetectionInput {
                screen: "⠴ Explore /tmp/test… 5.6s   19s ⇣29.7k [stop]\nEsc:cancel │ Ctrl+.:shortcuts",
                osc_title: "⠴ grok",
                osc_progress: "4;1;-1",
            },
        );
        assert_eq!(grok_work.state, DetectedState::Working);

        let grok_q = detect(
            Some(AgentKind::Grok),
            DetectionInput {
                screen: "◆ Which database should we use?\n┃  1 (●) SQLite\n┃  2 (○) Postgres\nEsc:unselect │ Tab:scrollback │ Shift+x:dismiss",
                osc_title: "⚠ Action Required - grok",
                osc_progress: "",
            },
        );
        assert_eq!(grok_q.state, DetectedState::Blocked);
        assert_eq!(grok_q.blocked_kind, Some(BlockedKind::Question));

        // Qwen Code title & confirmation
        let qwen_block = detect(
            Some(AgentKind::Qwen),
            DetectionInput {
                screen: "⠏ Waiting for user confirmation...",
                osc_title: "✳ Qwen",
                osc_progress: "",
            },
        );
        assert_eq!(qwen_block.state, DetectedState::Blocked);

        // Muse Code pick request & working
        let muse_q = detect(
            Some(AgentKind::Muse),
            DetectionInput {
                screen: "Select target environment?\nEnter to select · ↑/↓ to move · Tab for an optional note · Esc to interrupt",
                osc_title: "",
                osc_progress: "",
            },
        );
        assert_eq!(muse_q.state, DetectedState::Blocked);
        assert_eq!(muse_q.blocked_kind, Some(BlockedKind::Question));

        // Kiro CLI working
        let kiro_work = detect(
            Some(AgentKind::Kiro),
            DetectionInput {
                screen: "kiro is working · type to steer · ctrl+s to queue",
                osc_title: "◐ kiro: building",
                osc_progress: "4;3;",
            },
        );
        assert_eq!(kiro_work.state, DetectedState::Working);

        // Aider confirmation
        let aider_block = detect(
            Some(AgentKind::Aider),
            DetectionInput {
                screen: "Run shell command? (Y)es/(N)o [Yes]:",
                osc_title: "",
                osc_progress: "",
            },
        );
        assert_eq!(aider_block.state, DetectedState::Blocked);

        // Goose working
        let goose_work = detect(
            Some(AgentKind::Goose),
            DetectionInput {
                screen: "⠋ Thinking about your request... (esc to interrupt)",
                osc_title: "",
                osc_progress: "",
            },
        );
        assert_eq!(goose_work.state, DetectedState::Working);
    }
}
