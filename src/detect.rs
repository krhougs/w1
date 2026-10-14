use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::agents::AgentKind;
use crate::osc7501::{BlockedKind, OscState};

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
    id: String,
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

static MANIFESTS: LazyLock<HashMap<String, CompiledManifest>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    for &(id, raw_toml) in BUNDLED_MANIFESTS {
        if let Ok(parsed) = toml::from_str::<AgentManifest>(raw_toml) {
            if let Ok(compiled) = compile_manifest(&parsed) {
                map.insert(id.to_owned(), compiled);
                if parsed.id != id {
                    map.insert(parsed.id, compiled_clone_from_raw(raw_toml));
                }
            }
        }
    }
    map
});

fn manifests() -> &'static HashMap<String, CompiledManifest> {
    &MANIFESTS
}

fn compiled_clone_from_raw(raw_toml: &str) -> CompiledManifest {
    toml::from_str::<AgentManifest>(raw_toml)
        .ok()
        .and_then(|m| compile_manifest(&m).ok())
        .unwrap_or(CompiledManifest { rules: Vec::new() })
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
    let manifest_key = agent.map(|a| a.manifest_id()).unwrap_or("generic");
    let table = manifests();
    let Some(compiled) = table.get(manifest_key).or_else(|| table.get("generic")) else {
        return fallback_detection(agent, input);
    };

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

/// Extracts a concise human-readable 1-line summary (`msg`) from screen / OSC title
/// when the agent is working or blocked.
pub fn extract_screen_detail(
    agent: Option<AgentKind>,
    state: DetectedState,
    rule_id: &str,
    input: DetectionInput<'_>,
) -> Option<String> {
    match state {
        DetectedState::Blocked => extract_blocked_message(agent, rule_id, input.screen, input.osc_title),
        DetectedState::Working => extract_working_message(agent, input.screen, input.osc_title),
        _ => None,
    }
}

fn extract_blocked_message(
    _agent: Option<AgentKind>,
    rule_id: &str,
    screen: &str,
    osc_title: &str,
) -> Option<String> {
    let lines: Vec<&str> = bottom_non_empty_lines(screen, 20)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();

    if rule_id.contains("trust") {
        return Some("Workspace trust confirmation required".to_owned());
    }
    if rule_id.contains("startup_update") {
        return Some("Update prompt waiting for confirmation".to_owned());
    }

    // agy: `Requesting permission for:` followed by the indented command.
    if let Some(cmd) = lines
        .iter()
        .rposition(|l| l.eq_ignore_ascii_case("requesting permission for:"))
        .and_then(|i| lines.get(i + 1))
    {
        return crate::osc7501::sanitize_single_line(&format!("$ {cmd}"), 240);
    }

    // A question dialog shows no command; `$`/`↳` lines above it are earlier tool output.
    let command_lines = if rule_id.contains("question") { &[][..] } else { &lines[..] };
    for &line in command_lines.iter().rev() {
        let clean = strip_box_borders(line);
        let clean = clean.trim_start_matches(['└', '│', ' ']);
        if let Some(cmd) = clean.strip_prefix("$ ").or_else(|| clean.strip_prefix("↳ ")) {
            let cmd = TRAILING_TIMER
                .as_ref()
                .map_or(cmd, |re| re.find(cmd).map_or(cmd, |m| &cmd[..m.start()]));
            return crate::osc7501::sanitize_single_line(&format!("$ {}", cmd.trim()), 240);
        }
    }

    for &line in &lines {
        if let Some(subject) = strip_box_borders(line).strip_prefix("? ") {
            let subject = subject.trim().trim_end_matches(['←', '→']).trim_end();
            if let Some(subject) = crate::osc7501::sanitize_single_line(subject, 240) {
                return Some(subject);
            }
        }
    }

    for &line in lines.iter().rev() {
        let clean = strip_box_borders(line);
        if clean.is_empty() || is_footer_hint_line(clean) || is_option_choice_line(clean) {
            continue;
        }
        let lower = clean.to_lowercase();
        if clean.ends_with('?')
            || lower.contains("allow ")
            || lower.contains("approve ")
            || lower.contains("permission")
            || lower.contains("run this command")
            || lower.contains("write to this file")
            || lower.contains("apply this change")
            || lower.contains("do you want to")
            || lower.contains("would you like to")
            || lower.contains("requires approval")
            || lower.contains("requests your input")
            || lower.contains("dangerous command")
        {
            return crate::osc7501::sanitize_single_line(clean, 240);
        }
    }

    for &line in lines.iter().rev() {
        let clean = strip_box_borders(line);
        if clean.is_empty() || is_footer_hint_line(clean) || is_option_choice_line(clean) {
            continue;
        }
        if clean.chars().count() >= 6 {
            return crate::osc7501::sanitize_single_line(clean, 240);
        }
    }

    if !osc_title.trim().is_empty() {
        return crate::osc7501::sanitize_single_line(osc_title.trim(), 240);
    }

    Some("Waiting for user confirmation".to_owned())
}

fn extract_working_message(
    agent: Option<AgentKind>,
    screen: &str,
    osc_title: &str,
) -> Option<String> {
    if matches!(agent, Some(AgentKind::Gemini)) {
        let t = osc_title.trim();
        if let Some(rest) = t.strip_prefix('✦') {
            let s = rest.trim();
            let s = if let Some(idx) = s.rfind(" (") {
                s[..idx].trim()
            } else {
                s
            };
            if !s.is_empty() && s != "Working…" && s != "Working..." {
                return crate::osc7501::sanitize_single_line(s, 240);
            }
        }
    }

    let bottom = bottom_non_empty_lines(screen, 14);
    for line in bottom.lines().rev() {
        let trimmed = line.trim();
        if trimmed.is_empty() || is_footer_hint_line(trimmed) {
            continue;
        }
        // Match Codex / Claude / Grok / Gemini / Qwen / Amp active status lines
        if let Some(activity) = parse_spinner_or_activity_line(trimmed) {
            return normalize_activity(&activity);
        }
    }

    None
}

static ACTIVITY_NOISE: LazyLock<Option<Regex>> = LazyLock::new(|| {
    Regex::new(
        r"(?ix)
        \s*\((?:[^)]*\b(?:esc|ctrl\+c|interrupt|cancel)\b[^)]*)\)   # (esc to interrupt), (esc to cancel, 3s)
        | \s*\(\d+(?:\.\d+)?k?c\)                                    # (128c)
        | \s*(?:·|⋯|\|)?\s*\d+(?:\.\d+)?\s*(?:ms|s|m|h)\b(?:\s+\d+(?:\.\d+)?\s*s\b)?  # timers
        | \s*⇣\s*[\d.]+k?                                              # token counters
        | \s*\[stop\]
        | \s+esc\s+(?:again\s+to\s+)?(?:edit\s+prompt|interrupt|cancel)\b.*$     # Copilot status hints
        | \s*·\s*[\d.]+\s*[KMG]?B\b                                    # streamed byte counters
        ",
    )
    .ok()
});

static TRAILING_TIMER: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"\s+\d+(?:\.\d+)?\s*(?:ms|s|m)\s*$").ok());

/// Status words that only restate "working" and would make the summary flicker.
fn is_generic_activity(caption: &str) -> bool {
    let word = caption
        .trim_end_matches(['.', '…'])
        .trim()
        .to_lowercase();
    matches!(
        word.as_str(),
        "thinking" | "streaming" | "working" | "invoking tools" | "processing" | "loading" | "generating"
    )
}

/// Strips spinner glyphs, timers, counters and key hints from a status caption.
pub fn normalize_activity(raw: &str) -> Option<String> {
    let trimmed = raw.trim_start_matches(|c: char| {
        c.is_whitespace()
            || matches!(c as u32, 0x2800..=0x28FF | 0xFE0E | 0xFE0F)
            || "•◦◆✦✳✻✽✶✢⬡⬢◔◑◕◐◓◒●◎○⏲⏳".contains(c)
    });
    let cleaned = match ACTIVITY_NOISE.as_ref() {
        Some(re) => re.replace_all(trimmed, "").into_owned(),
        None => trimmed.to_owned(),
    };
    let cleaned = cleaned
        .trim()
        .trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '·' | '⋯' | '-' | '|' | ','))
        .trim();
    let letters = cleaned
        .chars()
        .filter(|c| c.is_alphabetic() && !matches!(*c as u32, 0x2800..=0x28FF))
        .count();
    if letters < 2 || is_generic_activity(cleaned) {
        return None;
    }
    crate::osc7501::sanitize_single_line(cleaned, 240)
}

fn parse_spinner_or_activity_line(line: &str) -> Option<String> {
    let mut s = line.trim();
    if let Some(first) = s.chars().next() {
        let is_marker = matches!(
            first,
            '\u{2800}'..='\u{28FF}'
                | '•'
                | '◦'
                | '◆'
                | '✦'
                | '✳'
                | '✻'
                | '✽'
                | '✶'
                | '✢'
                | '⬡'
                | '⬢'
                | '◔'
                | '◑'
                | '◕'
                | '●'
                | '◎'
        );
        // Transcript bullets look like status lines; only a live one carries a key hint or timer.
        let bullet = matches!(first, '•' | '◦' | '●' | '◆' | '✦');
        if bullet && !has_live_status_hint(s) {
            return None;
        }
        if is_marker {
            s = s[first.len_utf8()..].trim_start();
        } else if s.starts_with("╰ ") {
            s = s["╰ ".len()..].trim_start().trim_end_matches('─').trim();
            if !s.is_empty() {
                return Some(s.to_owned());
            }
        } else {
            return None;
        }
    } else {
        return None;
    }

    // A floating panel drawn over the status line starts at its border.
    if let Some(idx) = s.find(['┃', '│']) {
        s = s[..idx].trim_end();
    }
    if let Some(stripped) = s.strip_suffix("[stop]") {
        s = stripped.trim_end();
    }
    if let Some(idx) = s.rfind(" (") {
        let tail = &s[idx..];
        if tail.contains("esc") || tail.contains("interrupt") || tail.contains("cancel") || tail.ends_with("s)") {
            s = s[..idx].trim_end();
        }
    }

    if s.is_empty() || s.starts_with('›') || s.starts_with('❯') || s.starts_with("─") {
        return None;
    }
    // Logos drawn with braille / block art are not activity captions.
    let letters = s.chars().filter(|c| c.is_alphabetic() && !matches!(*c as u32, 0x2800..=0x28FF)).count();
    if letters < 2 || is_footer_hint_line(s) {
        return None;
    }
    Some(s.to_owned())
}

fn has_live_status_hint(line: &str) -> bool {
    let lower = line.to_lowercase();
    lower.contains("esc to")
        || lower.contains("interrupt")
        || lower.contains("ctrl+c")
        || lower.trim_end().ends_with("s)")
}

fn strip_box_borders(line: &str) -> &str {
    line.trim()
        .trim_start_matches(['│', '┃', '╭', '╰', '├', '▸', '▶', '❯', '❭', '›', '•', '◆', '●', '○', ' '])
        .trim_end_matches(['│', '┃', '╮', '╯', '┤'])
        .trim()
}

fn is_footer_hint_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    if line.chars().all(|c| matches!(c, '─' | '-' | '═' | ' ')) || line.starts_with("───") {
        return true;
    }
    if lower.contains("for shortcuts")
        || lower.contains("to view")
        || lower.contains("for agents")
        || lower.starts_with("queued follow-up")
        || lower.starts_with("messages to be submitted")
        || lower.starts_with("tip:")
        || lower.starts_with("└ tip:")
        || lower.contains("enter to steer")
        || lower.contains("shift + tab to cycle")
        || lower.contains("shift+tab to cycle")
        || lower.starts_with("➜ ")
        || lower.starts_with("read context files")
        || lower.contains("type your message")
    {
        return true;
    }
    (lower.contains("esc") || lower.contains("enter") || lower.contains("navigate") || lower.contains("select"))
        && (lower.contains("cancel")
            || lower.contains("confirm")
            || lower.contains("dismiss")
            || lower.contains("submit")
            || lower.contains("shortcuts")
            || lower.contains("scrollback")
            || lower.contains("amend"))
}

fn is_option_choice_line(line: &str) -> bool {
    let lower = line.to_lowercase();
    if matches!(
        lower.as_str(),
        "yes" | "no" | "allow" | "deny" | "approve" | "reject" | "proceed (y)" | "keep (n)"
    ) {
        return true;
    }
    let mut chars = lower.chars();
    if chars.next().is_some_and(|c| c.is_ascii_digit())
        && chars.next().is_some_and(|c| c == '.' || c == ' ' || c == ')')
    {
        return true;
    }
    if lower.starts_with("1.")
        || lower.starts_with("2.")
        || lower.starts_with("3.")
        || lower.starts_with("(●)")
        || lower.starts_with("(○)")
        || lower.starts_with("[y]")
        || lower.starts_with("[n]")
        || lower.starts_with("> yes")
        || lower.starts_with("> no")
    {
        return true;
    }
    false
}

const TURN_START_GRACE: std::time::Duration = std::time::Duration::from_millis(1500);

/// Turns raw screen detections into report states.
///
/// Holds `Working` for up to 650ms (or 3 consecutive rechecks) unless explicit idle chrome is
/// visible, so frame redraws do not flicker. A turn only ends in `Done` when the user submitted
/// input (or the session store confirmed a turn); activity the user did not start, such as
/// start-up spinners or MCP reconnects, falls back to the previous resting state instead.
pub struct StateStabilizer {
    current: OscState,
    resting: OscState,
    armed: bool,
    turn_active: bool,
    turn_started: Option<std::time::Instant>,
    pending_idle_since: Option<std::time::Instant>,
    pending_idle_count: u8,
}

impl Default for StateStabilizer {
    fn default() -> Self {
        Self {
            current: OscState::Idle,
            resting: OscState::Idle,
            armed: false,
            turn_active: false,
            turn_started: None,
            pending_idle_since: None,
            pending_idle_count: 0,
        }
    }
}

impl StateStabilizer {
    pub fn current(&self) -> OscState {
        self.current
    }

    pub fn is_armed(&self) -> bool {
        self.armed
    }

    pub fn turn_active(&self) -> bool {
        self.turn_active
    }

    /// The user submitted input; the next working phase is a real turn.
    pub fn arm(&mut self) {
        if self.current != OscState::Blocked {
            self.armed = true;
        }
    }

    pub fn update(&mut self, detection: &ScreenDetection, now: std::time::Instant) -> OscState {
        if detection.skip_state_update {
            return self.current;
        }

        match detection.state {
            DetectedState::Blocked => {
                self.reset_pending();
                self.current = OscState::Blocked;
            }
            DetectedState::Working => {
                self.reset_pending();
                if self.armed {
                    self.armed = false;
                    self.turn_active = true;
                    self.turn_started = Some(now);
                }
                self.current = OscState::Working;
            }
            DetectedState::Idle => {
                // A turn confirmed by input marks or the session store starts before the screen
                // repaints; idle chrome still on screen right then is stale.
                if self.current == OscState::Working
                    && self.turn_started.is_some_and(|t| now.duration_since(t) < TURN_START_GRACE)
                {
                    return OscState::Working;
                }
                if self.current == OscState::Working && !detection.visible_idle {
                    let since = *self.pending_idle_since.get_or_insert(now);
                    self.pending_idle_count = self.pending_idle_count.saturating_add(1);
                    if now.duration_since(since) < std::time::Duration::from_millis(650)
                        && self.pending_idle_count < 3
                    {
                        return OscState::Working;
                    }
                }
                self.reset_pending();
                if matches!(self.current, OscState::Working | OscState::Blocked) {
                    self.current = if self.turn_active {
                        OscState::Done
                    } else {
                        self.resting
                    };
                    self.turn_active = false;
                    self.resting = self.current;
                }
            }
            DetectedState::Unknown => {}
        }

        self.current
    }

    /// Applies a state confirmed by the agent's own session store.
    pub fn force_state(&mut self, state: OscState) {
        self.reset_pending();
        match state {
            OscState::Working => {
                self.armed = false;
                if !self.turn_active || self.current != OscState::Working {
                    self.turn_started = Some(std::time::Instant::now());
                }
                self.turn_active = true;
            }
            OscState::Blocked => {}
            OscState::Idle | OscState::Done | OscState::Error => {
                self.turn_active = false;
                self.resting = state;
            }
        }
        self.current = state;
    }

    fn reset_pending(&mut self) {
        self.pending_idle_since = None;
        self.pending_idle_count = 0;
    }
}

fn select_region<'a>(input: DetectionInput<'a>, spec: &str) -> &'a str {
    let trimmed = spec.trim();
    match trimmed {
        "osc_title" => return input.osc_title,
        "osc_progress" => return input.osc_progress,
        _ => {}
    }
    let content = input.screen;
    match trimmed {
        "whole_recent" => content,
        "after_last_prompt_marker" => after_last_prompt_marker(content),
        "before_current_prompt_marker" => before_current_prompt_marker(content),
        "whole_recent_without_current_prompt_marker" => {
            whole_recent_without_current_prompt_marker(content)
        }
        "current_prompt_block_marker" => current_prompt_block_marker(content).unwrap_or(""),
        "after_current_prompt_block_marker" => {
            after_current_prompt_block_marker(content).unwrap_or("")
        }
        "prompt_box_body" => prompt_box_body(content).unwrap_or(""),
        "above_prompt_box" => above_prompt_box(content),
        "last_non_empty_above_prompt_box" => last_non_empty_line(above_prompt_box(content)),
        "after_last_horizontal_rule" => after_last_horizontal_rule(content),
        _ => {
            if let Some(count) = region_count(trimmed, "bottom_lines") {
                return bottom_lines(content, count);
            }
            if let Some(count) = region_count(trimmed, "bottom_non_empty_lines") {
                return bottom_non_empty_lines(content, count);
            }
            if let Some(count) = region_count(trimmed, "top_non_empty_lines") {
                return top_non_empty_lines(content, count);
            }
            ""
        }
    }
}

fn region_count(spec: &str, name: &str) -> Option<usize> {
    spec.strip_prefix(name)
        .and_then(|r| r.strip_prefix('('))
        .and_then(|r| r.strip_suffix(')'))
        .and_then(|c| c.parse::<usize>().ok())
}

fn bottom_lines(content: &str, count: usize) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let start = lines.len().saturating_sub(count);
    slice_from_line_index(content, &lines, start)
}

fn bottom_non_empty_lines(content: &str, count: usize) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(start_index) = lines
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, line)| !line.trim().is_empty())
        .take(count)
        .last()
        .map(|(idx, _)| idx)
    else {
        return "";
    };
    slice_from_line_index(content, &lines, start_index)
}

fn top_non_empty_lines(content: &str, count: usize) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(end_index) = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .take(count)
        .last()
        .map(|(idx, _)| idx)
    else {
        return "";
    };
    let byte_offset = line_start_offset(content, &lines, end_index + 1);
    &content[..byte_offset]
}

fn after_last_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(index) = lines.iter().rposition(|line| codex_prompt_line(line)) else {
        return content;
    };
    slice_from_line_index(content, &lines, index + 1)
}

fn before_current_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(index) = current_codex_prompt_index(&lines) else {
        return content;
    };
    let byte_offset = lines[..index].iter().map(|l| l.len() + 1).sum::<usize>();
    &content[..byte_offset.min(content.len())]
}

fn whole_recent_without_current_prompt_marker(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    if current_codex_prompt_index(&lines).is_some() {
        ""
    } else {
        content
    }
}

fn current_prompt_block_marker(content: &str) -> Option<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let prompt_index = current_codex_prompt_index(&lines)?;
    lines[..prompt_index]
        .iter()
        .rev()
        .find(|line| codex_block_marker_line(line))
        .copied()
}

fn after_current_prompt_block_marker(content: &str) -> Option<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let prompt_index = current_codex_prompt_index(&lines)?;
    let block_index = lines[..prompt_index]
        .iter()
        .rposition(|line| codex_block_marker_line(line))?;
    Some(slice_from_line_index(content, &lines, block_index))
}

fn current_codex_prompt_index(lines: &[&str]) -> Option<usize> {
    let prompt_index = lines.iter().rposition(|line| codex_prompt_line(line))?;
    if lines[prompt_index + 1..]
        .iter()
        .any(|line| codex_block_marker_line(line))
    {
        return None;
    }
    Some(prompt_index)
}

fn codex_prompt_line(line: &str) -> bool {
    line == "›" || line.starts_with("› ")
}

fn codex_block_marker_line(line: &str) -> bool {
    line.starts_with('•') || line.starts_with('■') || line.starts_with('✗') || line.starts_with('✓')
}

fn prompt_box_body(content: &str) -> Option<&str> {
    let lines: Vec<&str> = content.lines().collect();
    let top = prompt_box_top_border_index(&lines)?;
    let start = line_start_offset(content, &lines, top + 1);
    let end_index = lines[top + 1..]
        .iter()
        .position(|line| is_horizontal_rule(line))
        .map(|rel| top + 1 + rel)
        .unwrap_or(lines.len());
    let end = line_start_offset(content, &lines, end_index);
    Some(&content[start.min(content.len())..end.min(content.len())])
}

fn above_prompt_box(content: &str) -> &str {
    let lines: Vec<&str> = content.lines().collect();
    let Some(top) = prompt_box_top_border_index(&lines) else {
        return content;
    };
    let end = line_start_offset(content, &lines, top);
    &content[..end.min(content.len())]
}

fn after_last_horizontal_rule(content: &str) -> &str {
    let mut last_rule_end = 0usize;
    let mut offset = 0usize;
    for line in content.lines() {
        let next_offset = offset + line.len() + 1;
        if is_horizontal_rule(line) {
            last_rule_end = next_offset.min(content.len());
        }
        offset = next_offset;
    }
    &content[last_rule_end..]
}

fn last_non_empty_line(content: &str) -> &str {
    content
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
}

fn prompt_box_top_border_index(lines: &[&str]) -> Option<usize> {
    let mut border_count = 0;
    for index in (0..lines.len()).rev() {
        if is_horizontal_rule(lines[index]) {
            border_count += 1;
            if border_count == 2 {
                return Some(index);
            }
        }
    }
    None
}

fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }
    let rule_chars = trimmed.chars().take_while(|&ch| ch == '─').count();
    if rule_chars == 0 {
        return false;
    }
    let rule_bytes = trimmed
        .char_indices()
        .nth(rule_chars)
        .map(|(idx, _)| idx)
        .unwrap_or(trimmed.len());
    let suffix = trimmed[rule_bytes..].trim_start();
    suffix.is_empty() || rule_chars >= 3
}

fn slice_from_line_index<'a>(content: &'a str, lines: &[&str], index: usize) -> &'a str {
    let byte_offset = line_start_offset(content, lines, index);
    &content[byte_offset.min(content.len())..]
}

fn line_start_offset(content: &str, lines: &[&str], index: usize) -> usize {
    lines[..index.min(lines.len())]
        .iter()
        .map(|l| l.len() + 1)
        .sum::<usize>()
        .min(content.len())
}

const BUNDLED_MANIFESTS: &[(&str, &str)] = &[
    ("codex", MANIFEST_CODEX),
    ("gemini", MANIFEST_GEMINI),
    ("opencode", MANIFEST_OPENCODE),
    ("cursor", MANIFEST_CURSOR),
    ("devin", MANIFEST_DEVIN),
    ("agy", MANIFEST_ANTIGRAVITY),
    ("cline", MANIFEST_CLINE),
    ("copilot", MANIFEST_COPILOT),
    ("kimi", MANIFEST_KIMI),
    ("kiro", MANIFEST_KIRO),
    ("droid", MANIFEST_DROID),
    ("amp", MANIFEST_AMP),
    ("grok", MANIFEST_GROK),
    ("hermes", MANIFEST_HERMES),
    ("kilo", MANIFEST_KILO),
    ("qodercli", MANIFEST_QODERCLI),
    ("qwen", MANIFEST_QWEN),
    ("letta", MANIFEST_LETTA),
    ("maki", MANIFEST_MAKI),
    ("muse", MANIFEST_MUSE),
    ("aider", MANIFEST_AIDER),
    ("goose", MANIFEST_GOOSE),
    ("crush", MANIFEST_CRUSH),
    ("zcode", MANIFEST_ZCODE),
    ("mastracode", MANIFEST_GENERIC),
    ("claude", MANIFEST_CLAUDE),
    ("omp", MANIFEST_OMP),
    ("pi", MANIFEST_PI),
    ("generic", MANIFEST_GENERIC),
];

const MANIFEST_CODEX: &str = r#"
id = "codex"

[[rules]]
id = "osc_title_blocked"
state = "blocked"
priority = 1100
region = "osc_title"
visible_blocker = true
contains = ["Action Required"]

[[rules]]
id = "osc_title_working"
state = "working"
priority = 1050
region = "osc_title"
visible_working = true
regex = ['(?:^| )[\x{2800}-\x{28FF}](?: |$)']

[[rules]]
id = "transcript_viewer"
state = "unknown"
priority = 1000
region = "after_last_prompt_marker"
skip_state_update = true
contains = ["↑/↓ to scroll", "pgup/pgdn to", "home/end to jump", "q to quit"]
any = [
  { contains = ["esc to edit prev"] },
  { contains = ["esc/← to edit prev"] },
]

[[rules]]
id = "trust_directory"
state = "blocked"
priority = 950
region = "top_non_empty_lines(20)"
visible_blocker = true
all = [
  { any = [
    { regex = ['\A> You are in [^\r\n]+(?:\r?\n|$)'] },
    { contains = ["Folder access"] },
  ] },
  { any = [
    { regex = ['(?s)Do\s+you\s+trust\s+the\s+contents\s+of\s+this\s+directory\?'] },
    { all = [
      { contains = ["Trust this folder?", "Codex can read, edit, and run files here"] },
      { any = [
        { contains = ["Trust and continue"] },
        { contains = ["enter continue"] },
      ] },
    ] },
  ] },
]

[[rules]]
id = "startup_update"
state = "blocked"
priority = 950
region = "bottom_non_empty_lines(20)"
visible_blocker = true
any = [
  { all = [
    { contains = ["Update available!", "Update now"] },
    { regex = ['Skip\s+until\s+next\s+version', 'Press enter to continue\s*\z'] },
  ] },
  { all = [
    { regex = ['(?m)^\s*Update\s+available\s+·'] },
    { regex = ['enter\s+continue\s+·\s+esc\s+skip\s*\z'] },
  ] },
]

[[rules]]
id = "live_strong_blocker"
state = "blocked"
priority = 900
region = "after_last_prompt_marker"
visible_blocker = true
any = [
  { contains = ["press enter to confirm or esc to cancel"] },
  { contains = ["enter to submit answer"] },
  { contains = ["enter to submit all"] },
  { contains = ["allow command?"] },
  { contains = ["All Results", "Filesystem Only", "Plugins"] },
]

[[rules]]
id = "weak_blocker"
state = "blocked"
priority = 600
region = "whole_recent_without_current_prompt_marker"
not = [{ regex = ['(?m)^›[⠁⠂⠄⠈⠐⠠⡀⢀][^\n]*(?:\n(?:[^•■✗✓\n][^\n]*)?)*\z'] }]
any = [
  { contains = ["[y/n]"] },
  { contains = ["yes (y)"] },
  { contains = ["do you want to"], any = [{ contains = ["yes"] }, { contains = ["❯"] }] },
  { contains = ["would you like to"], any = [{ contains = ["yes"] }, { contains = ["❯"] }] },
]

[[rules]]
id = "screen_working_fallback"
state = "working"
priority = 500
region = "before_current_prompt_marker"
visible_working = true
any = [{ contains = [" to interrupt)"] }, { contains = ["s)"] }]
regex = ['(?m)^(?:[•◦][ \t]+)?[^\s›•◦■✗✓─][^\r\n]* \((?:[0-9]+[hm] )*[0-9]+s(?: • [^\r\n]+? to interrupt)?\)(?: · [^\r\n]*)?(?:\r?\n(?:[^•◦›■✗✓─\r\n][^\r\n]*|•[ \t]+(?:Queued\s+follow-up\s+inputs|Messages\s+to\s+be\s+submitted\s+after\s+next\s+tool\s+call(?:\s+\(press\s+[^\r\n]+?\s+to\s+interrupt\s+and\s+send\s+immediately\))?|Messages\s+to\s+be\s+submitted\s+at\s+end\s+of\s+turn)|›[⠁⠂⠄⠈⠐⠠⡀⢀][^\r\n]*)?)*\s*\z']
not = [{ line_regex = ['^(?:[•◦][ \t]+)?Reconnect failed — check the endpoint, then relaunch \([0-9hms ]+\)$'] }]

[[rules]]
id = "osc_title_idle"
state = "idle"
priority = 100
region = "osc_title"
visible_idle = true
regex = ['\S']
not = [
  { regex = ['(?:^| )[\x{2800}-\x{28FF}](?: |$)'] },
  { contains = ["Action Required"] },
]
"#;

const MANIFEST_GEMINI: &str = r#"
id = "gemini"

[[rules]]
id = "osc_title_blocked"
state = "blocked"
priority = 1100
region = "osc_title"
visible_blocker = true
contains = ["Action Required"]

[[rules]]
id = "osc_title_working"
state = "working"
priority = 1050
region = "osc_title"
visible_working = true
any = [
  { regex = ['^[✦⏲]'] },
  { contains = ["Working…"] },
]

[[rules]]
id = "apply_or_allow_change"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(20)"
visible_blocker = true
any = [
  { contains = ["│ Apply this change"] },
  { contains = ["│ Allow execution"] },
  { all = [{ contains = ["yes"] }, { any = [{ contains = ["waiting for user confirmation"] }, { contains = ["│ Do you want to proceed"] }, { contains = ["do you want to proceed?"] }] }] },
  { line_regex = ['(?i)^\s*❯.*(yes|allow)'] },
]

[[rules]]
id = "answer_questions"
state = "blocked"
priority = 310
region = "bottom_non_empty_lines(24)"
visible_blocker = true
contains = ["Answer Questions"]
any = [{ contains = ["Enter to select"] }, { contains = ["Esc to cancel"] }]

[[rules]]
id = "esc_cancel_working"
state = "working"
priority = 100
region = "bottom_non_empty_lines(12)"
visible_working = true
contains = ["esc to cancel"]

[[rules]]
id = "osc_title_idle"
state = "idle"
priority = 80
region = "osc_title"
visible_idle = true
regex = ['^◇\s+Ready']
"#;

const MANIFEST_OPENCODE: &str = r#"
id = "opencode"

[[rules]]
id = "permission_required"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(24)"
visible_blocker = true
contains = ["Permission required"]
any = [
  { contains = ["enter confirm"] },
  { contains = ["Allow once"] },
]

[[rules]]
id = "question_dialog"
state = "blocked"
priority = 290
region = "bottom_non_empty_lines(24)"
visible_blocker = true
contains = ["esc dismiss"]
any = [{ contains = ["enter submit"] }, { contains = ["enter toggle"] }]

[[rules]]
id = "interrupt_hint_working"
state = "working"
priority = 110
region = "bottom_non_empty_lines(8)"
visible_working = true
any = [
  { contains = ["esc interrupt"] },
  { contains = ["esc again to interrupt"] },
  { contains = ["esc to interrupt"] },
]

[[rules]]
id = "progress_bar_working"
state = "working"
priority = 100
region = "bottom_non_empty_lines(8)"
visible_working = true
regex = ['(■|⬝){4,}']
"#;

const MANIFEST_CURSOR: &str = r#"
id = "cursor"

[[rules]]
id = "login_required"
state = "blocked"
priority = 400
region = "whole_recent"
visible_blocker = true
contains = ["press any key to log in"]

[[rules]]
id = "trust_prompt"
state = "blocked"
priority = 390
region = "whole_recent"
visible_blocker = true
contains = ["workspace trust required"]

[[rules]]
id = "write_file_approval"
state = "blocked"
priority = 320
region = "bottom_non_empty_lines(8)"
visible_blocker = true
contains = ["write to this file?", "proceed (y)"]
any = [
  { contains = ["reject & propose changes"] },
  { contains = ["esc or n or p"] },
  { contains = ["add write("] },
]

[[rules]]
id = "approval_prompt"
state = "blocked"
priority = 300
region = "whole_recent"
visible_blocker = true
any = [
  { contains = ["waiting for approval", "run this command?"], any = [{ contains = ["run (once) (y)"] }, { contains = ["skip (esc or n)"] }] },
  { contains = ["(y) (enter)"] },
  { line_regex = ['(?i)^\s*allow .*\(y\)'] },
  { contains = ["keep (n)"] },
  { contains = ["skip (esc or n)"] },
  { line_regex = ['(?i)^\s*(?:→\s*)?run .*\(y\)'] },
]

[[rules]]
id = "stop_hint_working"
state = "working"
priority = 100
region = "bottom_non_empty_lines(6)"
visible_working = true
contains = ["ctrl+c to stop"]

[[rules]]
id = "background_task_status_working"
state = "working"
priority = 95
region = "bottom_non_empty_lines(5)"
visible_working = true
line_regex = ['(?i)\b[1-9][0-9]*\s+background\s+tasks?\b']

[[rules]]
id = "spinner_working"
state = "working"
priority = 90
region = "bottom_non_empty_lines(8)"
visible_working = true
line_regex = ['^\s*(⬡|⬢|[\u2800-\u28FF]+)\s+\p{Alphabetic}+\w*ing\b']
"#;

/// omp (oh-my-pi) shows activity only as a braille spinner in its OSC 0 title (`π ⠋ proj`).
const MANIFEST_OMP: &str = r#"
id = "omp"

[[rules]]
id = "osc_title_working"
state = "working"
priority = 200
region = "osc_title"
visible_working = true
regex = ['(?:^|\s)[\x{2801}-\x{28FF}](?:\s|$)']

[[rules]]
id = "osc_title_idle"
state = "idle"
priority = 100
region = "osc_title"
visible_idle = true
regex = ['\S']
not = [{ regex = ['[\x{2801}-\x{28FF}]'] }]
"#;

const MANIFEST_DEVIN: &str = r#"
id = "devin"

[[rules]]
id = "workspace_trust_prompt"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(10)"
visible_blocker = true
contains = ["do you trust the authors of this directory?"]

[[rules]]
id = "permission_prompt"
state = "blocked"
priority = 290
region = "bottom_non_empty_lines(12)"
visible_blocker = true
contains = ["approve once", "confirm", "esc cancel"]

[[rules]]
id = "question_prompt"
state = "blocked"
priority = 280
region = "bottom_non_empty_lines(4)"
visible_blocker = true
contains = ["select", "confirm", "esc cancel"]
not = [{ contains = ["approve once"] }]

[[rules]]
id = "interruptible_working"
state = "working"
priority = 200
region = "bottom_non_empty_lines(10)"
visible_working = true
any = [
  { contains = ["esc twice to interrupt"] },
  { contains = ["esc to interrupt"] },
  { contains = ["guide devin while it works"] },
]

[[rules]]
id = "prompt_idle"
state = "idle"
priority = 100
region = "bottom_non_empty_lines(8)"
visible_idle = true
line_regex = ['^\s*❭ ']
not = [
  { contains = ["esc twice to interrupt"] },
  { contains = ["guide devin while it works"] },
  { contains = ["esc cancel"] },
  { contains = ["esc to quit"] },
]
"#;

const MANIFEST_ANTIGRAVITY: &str = r#"
id = "agy"

[[rules]]
id = "permission_prompt"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(3)"
visible_blocker = true
contains = ["navigate", "tab amend"]

[[rules]]
id = "question_prompt"
state = "blocked"
priority = 290
region = "bottom_non_empty_lines(3)"
visible_blocker = true
contains = ["navigate", "esc skip"]

[[rules]]
id = "trust_prompt"
state = "blocked"
priority = 280
region = "bottom_non_empty_lines(8)"
visible_blocker = true
contains = ["do you trust the contents of this project?", "yes, i trust this folder"]

[[rules]]
id = "legacy_permission_prompt"
state = "blocked"
priority = 270
region = "whole_recent"
visible_blocker = true
contains = ["requesting permission for:"]
any = [
  { contains = ["do you want to proceed?"] },
  { contains = ["tab amend", "edit command"] },
]

[[rules]]
id = "esc_cancel_footer_working"
state = "working"
priority = 110
region = "bottom_non_empty_lines(2)"
visible_working = true
regex = ['(?m)^\s*(?:─{3,}\s*)?esc to cancel\b']

[[rules]]
id = "spinner_working"
state = "working"
priority = 100
region = "bottom_non_empty_lines(12)"
visible_working = true
line_regex = ['^\s*[\u2801-\u28FF]+\s+\S']
not = [{ contains = ["signing in"] }]

[[rules]]
id = "prompt_idle"
state = "idle"
priority = 50
region = "bottom_non_empty_lines(2)"
visible_idle = true
contains = ["? for shortcuts"]
"#;

const MANIFEST_CLINE: &str = r#"
id = "cline"

[[rules]]
id = "tool_permission"
state = "blocked"
priority = 300
region = "whole_recent"
visible_blocker = true
any = [
  { contains = ["let cline use this tool"] },
  { contains = ["[act mode]", "execute command?", "yes"] },
  { contains = ["[act mode]", "use this tool?", "yes"] },
  { contains = ["[plan mode]", "execute command?", "yes"] },
  { contains = ["[plan mode]", "use this tool?", "yes"] },
]

[[rules]]
id = "inline_tool_permission"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(16)"
visible_blocker = true
contains = ["Cline needs permission", "Approve tool call?", "[y] Approve", "[n] Deny"]

[[rules]]
id = "inline_question"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(24)"
visible_blocker = true
line_regex = ['^\s*Cline is asking a question\s*$', '^\s*>\s+\S']
contains = ["(Tab)", "Shift+Tab"]

[[rules]]
id = "active_turn"
state = "working"
priority = 200
region = "bottom_non_empty_lines(20)"
visible_working = true
any = [
  { line_regex = ['^\s*[\x{2801}-\x{28FF}]\s+\S'] },
  { contains = ["Thinking... (esc to cancel)"] },
]

[[rules]]
id = "composer_idle"
state = "idle"
priority = 100
region = "bottom_non_empty_lines(12)"
visible_idle = true
line_regex = ['^\s*❯(?:\s.*)?$']
contains = ["─", "(Tab)", "Shift+Tab"]
"#;

const MANIFEST_COPILOT: &str = r#"
id = "copilot"

[[rules]]
id = "login_required"
state = "blocked"
priority = 400
region = "bottom_non_empty_lines(16)"
visible_blocker = true
any = [
  { contains = ["please use /login to sign in"] },
  { contains = ["you must be logged in to send messages"] },
  { contains = ["what account do you want to log into?"] },
  { contains = ["enter one-time code"] },
]

[[rules]]
id = "ask_user_question"
state = "blocked"
priority = 310
region = "bottom_non_empty_lines(16)"
visible_blocker = true
contains = ["enter accept", "esc cancel"]
any = [{ contains = ["ctrl+d decline"] }, { contains = ["copilot needs information"] }]

[[rules]]
id = "selection_blocker"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(16)"
visible_blocker = true
all = [
  { any = [
    { contains = ["esc to cancel"] },
    { contains = ["esc cancel"] },
  ] },
  { any = [
    { contains = ["enter to select"] },
    { contains = ["enter to confirm"] },
    { contains = ["enter to submit"] },
    { contains = ["enter accept"] },
  ] },
]

[[rules]]
id = "background_agents_working"
state = "working"
priority = 110
region = "bottom_non_empty_lines(6)"
visible_working = true
line_regex = ['^\s*◎\s+Waiting for background agents(?:\s|·|$)']

[[rules]]
id = "working_cancel_hint"
state = "working"
priority = 100
region = "bottom_non_empty_lines(8)"
visible_working = true
any = [
  { contains = ["esc to cancel"] },
  { contains = ["esc again to cancel"] },
  { contains = ["esc interrupt"] },
  { contains = ["esc edit prompt"] },
  { contains = ["esc again to edit prompt"] },
  { contains = ["transient api error. retrying"] },
]
"#;

const MANIFEST_KIMI: &str = r#"
id = "kimi"

[[rules]]
id = "current_approval_panel"
state = "blocked"
priority = 400
region = "whole_recent"
visible_blocker = true
contains = ["↵ confirm"]
any = [
  { contains = ["run this command?"] },
  { contains = ["write this file?"] },
  { contains = ["apply these edits?"] },
  { contains = ["stop this task?"] },
  { contains = ["ready to build with this plan?"] },
  { line_regex = ['(?i)^\s*▶?\s*approve .*\?$'] },
]
all = [
  { contains = [" choose"] },
  { any = [{ contains = ["approve"] }, { contains = ["reject"] }, { contains = ["revise"] }] },
]

[[rules]]
id = "question_panel"
state = "blocked"
priority = 390
region = "whole_recent"
visible_blocker = true
contains = ["↑↓ select", "esc cancel"]
line_regex = ['^\s*question\s*$', '^\s*\? ']
any = [
  { contains = ["↵ choose"] },
  { contains = ["↵ toggle"] },
  { contains = ["↵ save"] },
]

[[rules]]
id = "legacy_approval_panel"
state = "blocked"
priority = 300
region = "whole_recent"
contains = ["requesting approval", "reject"]
any = [
  { contains = ["approve once"] },
  { contains = ["approve for this session"] },
]
all = [
  { any = [{ contains = ["1/2/3/4 choose"] }, { contains = ["↵ confirm"] }] },
]

[[rules]]
id = "question_confirm_panel"
state = "blocked"
priority = 380
region = "bottom_non_empty_lines(16)"
visible_blocker = true
contains = ["ready to submit your answers?", "↵ confirm"]

[[rules]]
id = "background_agent_status_working"
state = "working"
priority = 120
region = "bottom_non_empty_lines(3)"
visible_working = true
line_regex = ['(?i)\bkimi[-\w.]*\s+thinking\b.*\[[1-9][0-9]*\s+agents?\s+running\]']

[[rules]]
id = "moon_spinner_working"
state = "working"
priority = 100
region = "whole_recent"
visible_working = true
line_regex = ['^\s*(🌕|🌖|🌗|🌘|🌑|🌒|🌓|🌔)(\s|$)']

[[rules]]
id = "braille_spinner_working"
state = "working"
priority = 90
region = "whole_recent"
visible_working = true
line_regex = ['(?i)^\s*[\u2800-\u28FF]+\s*(thinking|working)(\.\.\.|…)|^\s*[\u2800-\u28FF]+\s*using ']

[[rules]]
id = "running_tool_working"
state = "working"
priority = 95
region = "bottom_non_empty_lines(12)"
visible_working = true
line_regex = ['^\s*● Running ']
"#;

const MANIFEST_KIRO: &str = r#"
id = "kiro"

[[rules]]
id = "live_prompt_idle"
state = "idle"
priority = 1100
region = "bottom_non_empty_lines(4)"
visible_idle = true
line_regex = ['(?i)^\s*[>›]\s*ask a question or describe a task(?:\s+(?:enter|↵))?\s*$']

[[rules]]
id = "tool_approval"
state = "blocked"
priority = 1050
region = "bottom_non_empty_lines(8)"
visible_blocker = true
any = [
  { all = [
    { regex = ['(?is)(?:^|\n)\s*esc\s+to\s+close\b(?:[^\n]*\bto\s+navigate\b[^\n]*|.*\bto\s+navigate\b.*\bto\s+select\b.*\btab\s+to\s+edit|[^\n]*\benter\s+to\s+see\s+more\s+options)\s*\z'] },
    { any = [
      { line_regex = [
        '(?i)^\s*[>❯]\s*(?:allow|always allow|deny|always deny)\s*$',
        '(?i)^\s*(?:[>❯]\s*)?allow\s*$',
        '(?i)^\s*(?:[>❯]\s*)?always allow\s*$',
        '(?i)^\s*(?:[>❯]\s*)?deny\s*$',
        '(?i)^\s*(?:[>❯]\s*)?always deny\s*$',
      ] },
      { line_regex = [
        '(?i)^\s*[>❯]\s*(?:yes, single permission|trust, always allow in this session|no \(tab to edit\))\s*$',
        '(?i)^\s*(?:[>❯]\s*)?yes, single permission\s*$',
        '(?i)^\s*(?:[>❯]\s*)?trust, always allow in this session\s*$',
        '(?i)^\s*(?:[>❯]\s*)?no \(tab to edit\)\s*$',
      ] },
      { line_regex = [
        '(?i)^\s*[>❯]\s*(?:trust\b.*|entire tool)\s*$',
        '(?i)^\s*(?:[>❯]\s*)?(?:trust )?entire tool(?:\s+\([^\n]*\))?(?:\s+(?:session|workspace|always))?\s*$',
      ] },
    ] },
  ] },
]

[[rules]]
id = "tool_approval_edit"
state = "blocked"
priority = 1045
region = "whole_recent"
visible_blocker = true
regex = ['(?im)(?:^|\n)[ \t]*[-─]+[ \t]*\n[^\n]*requires\s+approval\s*[·.]\s*modify\s+request[ \t]*\n(?:[ \t]*\n|[^\n]*[^\s─-][^\n]*\n)*?[ \t]*[>›][ \t]*[^\n]*\n(?:[ \t]*\n|[^\n]*[^\s─-][^\n]*\n)*[ \t]*[-─]+[ \t]*\n[ \t]*esc[ \t]+to[ \t]+close[ \t]*\n?\z']

[[rules]]
id = "crew_approval"
state = "blocked"
priority = 1040
region = "bottom_non_empty_lines(8)"
visible_blocker = true
contains = [
  "tool approval",
  "approve all pending",
  "configure individually (agent monitor)",
  "exit (cancel subagents)",
]

[[rules]]
id = "question_panel"
state = "blocked"
priority = 1030
region = "bottom_non_empty_lines(8)"
visible_blocker = true
contains = ["to navigate", "to submit", "esc to cancel"]

[[rules]]
id = "live_working_footer"
state = "working"
priority = 950
region = "bottom_non_empty_lines(4)"
visible_working = true
contains = ["kiro is working", "type to steer", "ctrl+s to queue"]

[[rules]]
id = "osc_title_working"
state = "working"
priority = 900
region = "osc_title"
visible_working = true
regex = ['(?i)^[◐◓◑◒/|\\-]\s+kiro:']

[[rules]]
id = "osc_progress_working"
state = "working"
priority = 890
region = "osc_progress"
visible_working = true
regex = ['^4;3;?$']
"#;

const MANIFEST_DROID: &str = r#"
id = "droid"

[[rules]]
id = "ask_user_question"
state = "blocked"
priority = 320
region = "bottom_non_empty_lines(16)"
visible_blocker = true
contains = ["esc stop agent"]
any = [{ contains = ["enter/1"] }, { contains = ["select"] }]

[[rules]]
id = "folder_trust"
state = "blocked"
priority = 310
region = "bottom_non_empty_lines(20)"
visible_blocker = true
contains = ["trust this folder"]

[[rules]]
id = "execute_selection_blocker"
state = "blocked"
priority = 300
region = "whole_recent"
visible_blocker = true
contains = ["enter to select", "esc to cancel"]
any = [
  { contains = ["↑↓ to navigate"] },
  { contains = ["use ↑↓ to navigate"] },
]
all = [
  { any = [{ contains = ["> yes, allow"] }, { contains = ["> no, cancel"] }] },
]

[[rules]]
id = "selection_menu_blocker"
state = "blocked"
priority = 290
region = "bottom_non_empty_lines(8)"
visible_blocker = true
contains = ["enter select", "esc cancel"]
any = [
  { contains = ["↑/↓ navigate"] },
  { contains = ["↑↓ navigate"] },
]

[[rules]]
id = "spinner_stop_working"
state = "working"
priority = 110
region = "whole_recent"
visible_working = true
contains = ["esc to stop"]
line_regex = ['^\s*[\u2800-\u28FF]']

[[rules]]
id = "stop_hint_working"
state = "working"
priority = 100
region = "whole_recent"
visible_working = true
contains = ["esc to stop"]
"#;

const MANIFEST_AMP: &str = r#"
id = "amp"

[[rules]]
id = "osc_title_plugin_confirmation_blocked"
state = "blocked"
priority = 1100
region = "osc_title"
visible_blocker = true
contains = ["Plugin confirmation needed"]

[[rules]]
id = "osc_title_working"
state = "working"
priority = 1050
region = "osc_title"
visible_working = true
regex = ['^[\x{2800}-\x{28FF}] ']

[[rules]]
id = "login_required"
state = "blocked"
priority = 400
region = "bottom_non_empty_lines(12)"
visible_blocker = true
any = [
  { contains = ["would you like to log in to amp?"] },
  { contains = ["paste the code from the browser"] },
]

[[rules]]
id = "approval_required"
state = "blocked"
priority = 310
region = "bottom_non_empty_lines(24)"
visible_blocker = true
contains = ["approval required"]
any = [{ contains = ["allow once"] }, { contains = ["reject"] }]

[[rules]]
id = "approval_footer"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(24)"
visible_blocker = true
any = [
  { contains = ["waiting for approval"] },
  { contains = ["invoke tool"] },
  { contains = ["run this command?"] },
  { contains = ["allow editing file:"] },
  { contains = ["allow creating file:"] },
  { contains = ["confirm tool call"] },
  { contains = ["approve"], any = [{ contains = ["allow all for this session"] }, { contains = ["allow all for every session"] }, { contains = ["allow file for every session"] }, { contains = ["deny with feedback"] }] },
]

[[rules]]
id = "status_footer_working"
state = "working"
priority = 200
region = "bottom_non_empty_lines(5)"
visible_working = true
line_regex = ['(?i)^\s*╰\s+\S+\s+(thinking|streaming|running tools|waiting)\s+─']

[[rules]]
id = "esc_cancel_working"
state = "working"
priority = 100
region = "whole_recent"
visible_working = true
contains = ["esc to cancel"]

[[rules]]
id = "osc_title_idle"
state = "idle"
priority = 50
region = "osc_title"
visible_idle = true
contains = [" - amp - "]
not = [
  { regex = ['^[\x{2800}-\x{28FF}] '] },
  { contains = ["Plugin confirmation needed"] },
]
"#;

const MANIFEST_GROK: &str = r#"
id = "grok"

[[rules]]
id = "auth_required"
state = "blocked"
priority = 1400
region = "bottom_non_empty_lines(20)"
visible_blocker = true
contains = ["authentication required"]
any = [{ contains = ["/login"] }, { contains = ["sign in"] }]

[[rules]]
id = "osc_title_blocked"
state = "blocked"
priority = 1300
region = "osc_title"
visible_blocker = true
contains = ["Action Required"]

[[rules]]
id = "option_dialog_blocked"
state = "blocked"
priority = 1200
region = "whole_recent"
visible_blocker = true
line_regex = ['^\s*┃\s+[0-9a-z]+\s+\([●○]\)\s']

[[rules]]
id = "permission_hints_blocked"
state = "blocked"
priority = 1190
region = "bottom_non_empty_lines(2)"
visible_blocker = true
contains = [":select", "ctrl+o:yolo", "ctrl+c:cancel"]

[[rules]]
id = "question_dialog_hints_blocked"
state = "blocked"
priority = 1185
region = "bottom_non_empty_lines(2)"
visible_blocker = true
contains = ["tab:scrollback", "shift+x:dismiss"]

[[rules]]
id = "permission_scope_selector"
state = "blocked"
priority = 1180
region = "whole_recent"
visible_blocker = true
contains = ["yes, proceed", "no, reject"]
any = [
  { contains = ["use ← → to choose permission whitelist scope"] },
  { contains = ["←/→:scope"] },
]

[[rules]]
id = "spinner_status_working"
state = "working"
priority = 1160
region = "whole_recent"
visible_working = true
line_regex = ['^\s*[\x{2801}-\x{28FF}]\s.*\[stop\]\s*$']

[[rules]]
id = "esc_cancel_hints_working"
state = "working"
priority = 1155
region = "bottom_non_empty_lines(2)"
visible_working = true
any = [{ contains = ["esc:cancel"] }, { contains = ["ctrl+c:cancel"] }]

[[rules]]
id = "osc_title_idle"
state = "idle"
priority = 1100
region = "osc_title"
visible_idle = true
regex = ['(?:^| - )grok$']
not = [
  { regex = ['[\x{2800}-\x{28FF}]'] },
]

[[rules]]
id = "osc_title_working"
state = "working"
priority = 1000
region = "osc_title"
visible_working = true
regex = ['(?:^|\s)[\x{2801}-\x{28FF}](?:\s|$)']

[[rules]]
id = "prompt_hints_idle"
state = "idle"
priority = 100
region = "bottom_non_empty_lines(2)"
visible_idle = true
any = [{ contains = ["ctrl+x:shortcuts"] }, { contains = ["ctrl+.:shortcuts"] }]
not = [
  { contains = ["esc:cancel"] },
  { contains = ["ctrl+c:cancel"] },
]
"#;

const MANIFEST_HERMES: &str = r#"
id = "hermes"

[[rules]]
id = "osc_title_blocked"
state = "blocked"
priority = 1100
region = "osc_title"
visible_blocker = true
regex = ['^⚠[\u{fe0e}\u{fe0f}]?(?:\s|$)']

[[rules]]
id = "osc_title_working"
state = "working"
priority = 1050
region = "osc_title"
visible_working = true
regex = ['^⏳[\u{fe0e}\u{fe0f}]?(?:\s|$)']

[[rules]]
id = "interrupt_status_working"
state = "working"
priority = 950
region = "bottom_non_empty_lines(5)"
visible_working = true
any = [
  { contains = ["msg=interrupt"] },
  { contains = ["ctrl+c to interrupt"] },
]

[[rules]]
id = "dangerous_command_approval"
state = "blocked"
priority = 900
region = "bottom_non_empty_lines(14)"
visible_blocker = true
any = [
  { contains = ["dangerous"] },
  { contains = ["approval"] },
  { contains = ["allow once", "deny"] },
  { line_regex = ['(?i)^\s*[▸>]?\s*1\.\s*allow'] },
]
all = [
  { any = [{ contains = ["enter confirm"] }, { contains = ["enter to confirm"] }, { contains = ["↑/↓ to select"] }, { contains = ["show full command"] }] },
]

[[rules]]
id = "clarification_prompt"
state = "blocked"
priority = 900
region = "bottom_non_empty_lines(14)"
visible_blocker = true
any = [
  { contains = ["hermes needs your"] },
  { line_regex = ['^\s*ask\s+\S'] },
  { contains = ["type your answer"] },
]
all = [
  { any = [{ contains = ["enter confirm"] }, { contains = ["enter to confirm"] }, { contains = ["enter send"] }, { contains = ["press enter"] }, { contains = ["↑/↓ select"] }, { contains = ["↑/↓ to select"] }, { contains = ["other (type"] }] },
]

[[rules]]
id = "credential_prompt"
state = "blocked"
priority = 900
region = "bottom_non_empty_lines(14)"
visible_blocker = true
any = [
  { contains = ["sudo password"] },
  { contains = ["skill setup"] },
  { contains = ["🔑", "for "] },
]

[[rules]]
id = "confirmation_prompt"
state = "blocked"
priority = 900
region = "bottom_non_empty_lines(14)"
visible_blocker = true
all = [
  { any = [{ contains = ["approve once", "cancel"] }, { contains = ["start a new session", "keep going"] }] },
  { any = [{ contains = ["enter to confirm"] }, { contains = ["enter confirm"] }, { contains = ["type 1/2/3"] }, { contains = ["y/n quick"] }] },
]

[[rules]]
id = "classic_cancel_working"
state = "working"
priority = 500
region = "bottom_non_empty_lines(5)"
visible_working = true
contains = ["ctrl+c cancel"]

[[rules]]
id = "osc_title_idle"
state = "idle"
priority = 100
region = "osc_title"
visible_idle = true
regex = ['^✓[\u{fe0e}\u{fe0f}]?(?:\s|$)']
"#;

const MANIFEST_KILO: &str = r#"
id = "kilo"

[[rules]]
id = "permission_required"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(24)"
visible_blocker = true
contains = ["Permission required"]
any = [
  { contains = ["enter confirm"] },
  { contains = ["Allow once"] },
]

[[rules]]
id = "question_dialog"
state = "blocked"
priority = 290
region = "bottom_non_empty_lines(24)"
visible_blocker = true
contains = ["esc dismiss"]
any = [{ contains = ["enter submit"] }, { contains = ["enter toggle"] }]

[[rules]]
id = "interrupt_hint_working"
state = "working"
priority = 110
region = "bottom_non_empty_lines(8)"
visible_working = true
any = [
  { contains = ["esc interrupt"] },
  { contains = ["esc again to interrupt"] },
  { contains = ["esc to interrupt"] },
]

[[rules]]
id = "progress_bar_working"
state = "working"
priority = 100
region = "bottom_non_empty_lines(8)"
visible_working = true
regex = ['(■|⬝){4,}']
"#;

const MANIFEST_QODERCLI: &str = r#"
id = "qodercli"

[[rules]]
id = "confirmation_or_input_blocker"
state = "blocked"
priority = 300
region = "whole_recent"
visible_blocker = true
any = [
  { contains = ["waiting for user confirmation"], any = [{ contains = ["yes"] }, { contains = ["no"] }, { contains = ["allow"] }, { contains = ["reject"] }] },
  { contains = ["awaiting approval"], any = [{ contains = ["allow"] }, { contains = ["reject"] }] },
  { contains = ["permission required"] },
  { contains = ["allow once or always?"] },
  { contains = ["asking user"] },
  { contains = ["enter your response"] },
  { contains = ["review your answers:"] },
  { contains = ["shell awaiting input"] },
]

[[rules]]
id = "cancel_hint_working"
state = "working"
priority = 100
region = "whole_recent"
visible_working = true
contains = ["(esc to cancel,"]

[[rules]]
id = "spinner_working"
state = "working"
priority = 90
region = "whole_recent"
visible_working = true
line_regex = ['^\s*[\u2800-\u28FF]\s+.*\p{Alphabetic}']
"#;

const MANIFEST_QWEN: &str = r#"
id = "qwen"

[[rules]]
id = "ask_user_question"
state = "blocked"
priority = 1300
region = "bottom_non_empty_lines(20)"
visible_blocker = true
any = [
  { contains = ["Type something..."] },
  { line_regex = ['^\s*↑/↓\s*:\s*Navigate\s*\|\s*Enter\s*:\s*Select'] },
]

[[rules]]
id = "osc_title_permission"
state = "blocked"
priority = 1200
region = "osc_title"
visible_blocker = true
regex = ['^\x{2733}\x{FE0E}? ']

[[rules]]
id = "osc_title_working"
state = "working"
priority = 1100
region = "osc_title"
visible_working = true
regex = ['^\x{25D0}\x{FE0E}? ']

[[rules]]
id = "waiting_for_permission_confirmation"
state = "blocked"
priority = 1000
region = "bottom_non_empty_lines(20)"
visible_blocker = true
line_regex = ['^\s*⠏\s+.*\.\.\.\s*$']
any = [
  { contains = ["Waiting for user confirmation..."] },
  { contains = ["等待用户确认..."] },
  { contains = ["等待用戶確認..."] },
  { contains = ["Warten auf Benutzerbestätigung..."] },
  { contains = ["En attente de la confirmation de l'utilisateur..."] },
  { contains = ["ユーザーの確認を待っています..."] },
  { contains = ["Aguardando confirmação do usuário..."] },
  { contains = ["Ожидание подтверждения от пользователя..."] },
  { contains = ["Esperant la confirmació de l'usuari..."] },
]

[[rules]]
id = "tool_confirmation"
state = "blocked"
priority = 990
region = "bottom_non_empty_lines(20)"
visible_blocker = true
contains = ["yes, allow once"]
any = [
  { contains = ["apply this change?"] },
  { contains = ["allow execution of:"] },
  { contains = ["allow execution of mcp tool"] },
  { contains = ["do you want to proceed?"] },
  { contains = ["shell command execution"] },
]

[[rules]]
id = "question_dialog"
state = "blocked"
priority = 980
region = "bottom_non_empty_lines(20)"
visible_blocker = true
line_regex = [
  '^\s*[❯›]\s*(?:\[(?: |✓)\]\s*)?\d+\.\s+',
  '^\s*↑/↓\s*:.*(?:Enter|Return)\s*:',
]

[[rules]]
id = "folder_trust_dialog"
state = "blocked"
priority = 970
region = "bottom_non_empty_lines(20)"
visible_blocker = true
contains = ["do you trust this folder?", "trust folder (", "don't trust (esc)"]

[[rules]]
id = "cancel_hint_working"
state = "working"
priority = 900
region = "bottom_non_empty_lines(8)"
visible_working = true
line_regex = ['^\s*(?:[⠁-⣿]|\.{1,2})\s+.*\(\d+(?:m(?:\s+\d+s)?|s).*\s·\sesc to cancel\)\s*$']

[[rules]]
id = "narrow_cancel_hint_working"
state = "working"
priority = 890
region = "bottom_non_empty_lines(8)"
visible_working = true
line_regex = ['^\s*\(\d+(?:m(?:\s+\d+s)?|s)\s·\sesc to cancel\)\s*$']

[[rules]]
id = "osc_tool_progress_working"
state = "working"
priority = 850
region = "osc_progress"
visible_working = true
regex = ['^4;3(?:;|$)']

[[rules]]
id = "composer_idle"
state = "idle"
priority = 100
region = "bottom_non_empty_lines(30)"
visible_idle = true
line_regex = ['^\s*>\s*(?:type\s*)?.*$']
any = [
  { contains = ["type your message"] },
  { contains = ["your message or @path/to/file"] },
  { contains = ["@path/to/file"] },
  { contains = ["type", "mes", "sage", "@pat", "h/to", "/fil"] },
]
"#;

const MANIFEST_LETTA: &str = r#"
id = "letta"

[[rules]]
id = "osc_progress_blocked"
state = "blocked"
priority = 1400
region = "osc_progress"
visible_blocker = true
regex = ['^4;3(?:;|$)']

[[rules]]
id = "osc_title_blocked"
state = "blocked"
priority = 1300
region = "osc_title"
visible_blocker = true
regex = ['^\[ [!.] \] Action Required(?: \| |$)']

[[rules]]
id = "command_approval"
state = "blocked"
priority = 1200
region = "bottom_non_empty_lines(20)"
visible_blocker = true
contains = ["Run this command?", "Enter to select · Esc to cancel"]

[[rules]]
id = "osc_title_working"
state = "working"
priority = 900
region = "osc_title"
visible_working = true
regex = ['(?:^| )[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏](?: |$)']

[[rules]]
id = "active_status"
state = "working"
priority = 850
region = "bottom_non_empty_lines(8)"
visible_working = true
line_regex = ['^\s*(?:\S+\s+)+is(?: \S+)*… \((?:esc to interrupt(?: · .*)?|interrupting)\)\s*$']

[[rules]]
id = "running_tool"
state = "working"
priority = 800
region = "bottom_non_empty_lines(8)"
visible_working = true
line_regex = ['^\s*(?:└\s*)?Running\.\.\.\s*(?:\(.*\))?$']

[[rules]]
id = "composer_idle"
state = "idle"
priority = 100
region = "bottom_non_empty_lines(8)"
visible_idle = true
any = [
  { line_regex = ['^\s*›\s*$'] },
  { line_regex = ['^\s*›\s+Try\s+"'] },
]
not = [
  { line_regex = ['^\s*(?:\S+\s+)+is(?: \S+)*… \((?:esc to interrupt(?: · .*)?|interrupting)\)\s*$'] },
  { line_regex = ['^\s*(?:└\s*)?Running\.\.\.\s*(?:\(.*\))?$'] },
]
"#;

const MANIFEST_MAKI: &str = r#"
id = "maki"

[[rules]]
id = "permission_prompt"
state = "blocked"
priority = 980
region = "whole_recent"
visible_blocker = true
contains = ["permission required"]
any = [
  { contains = ["y allow", "n deny"] },
  { contains = ["confirm allow"] },
  { contains = ["confirm deny"] },
  { contains = ["enter deny", "esc cancel"] },
]

[[rules]]
id = "plan_complete_form"
state = "blocked"
priority = 970
region = "whole_recent"
visible_blocker = true
contains = ["plan complete", "enter confirm"]
any = [
  { contains = ["space toggle parallel"] },
  { contains = ["edit plan"] },
]

[[rules]]
id = "status_bar_spinner_working"
state = "working"
priority = 900
region = "bottom_non_empty_lines(1)"
visible_working = true
line_regex = ['^( [\x{2800}-\x{28FF}]){1,2} \[(BUILD|PLAN|BASH)\]']

[[rules]]
id = "status_bar_idle"
state = "idle"
priority = 850
region = "bottom_non_empty_lines(1)"
visible_idle = true
line_regex = ['^ \[(BUILD|PLAN|BASH)\]']

[[rules]]
id = "prompt_box_idle"
state = "idle"
priority = 840
region = "bottom_non_empty_lines(3)"
visible_idle = true
line_regex = ['^❯ ']
not = [
  { contains = ["queue another prompt"] },
  { line_regex = ['^( [\x{2800}-\x{28FF}]){1,2} '] },
]
"#;

const MANIFEST_MUSE: &str = r#"
id = "muse"

[[rules]]
id = "workspace_trust_blocked"
state = "blocked"
priority = 970
region = "bottom_non_empty_lines(12)"
visible_blocker = true
contains = ["Do you trust this workspace?"]
any = [
  { contains = ["Trust and continue"] },
  { contains = ["Use Up/Down"] },
]

[[rules]]
id = "pick_request_blocked"
state = "blocked"
priority = 950
region = "bottom_non_empty_lines(8)"
visible_blocker = true
any = [
  { contains = ["Enter to select", "Tab for an optional note"] },
  { contains = ["Enter to toggle", "Esc to interrupt"] },
]

[[rules]]
id = "menu_overlay"
state = "unknown"
priority = 940
region = "bottom_non_empty_lines(8)"
skip_state_update = true
any = [
  { contains = ["enter confirm", "esc go back"] },
  { contains = ["enter save", "esc go back"] },
  { contains = ["space toggle", "esc close", "type filter"] },
]

[[rules]]
id = "working_esc_interrupt"
state = "working"
priority = 900
region = "bottom_non_empty_lines(8)"
visible_working = true
contains = ["esc to interrupt"]
not = [
  { contains = ["Enter to select", "Tab for an optional note"] },
  { contains = ["Enter to toggle", "Esc to interrupt"] },
]

[[rules]]
id = "blocked_approval"
state = "blocked"
priority = 850
region = "bottom_non_empty_lines(8)"
visible_blocker = true
any = [
  { contains = ["Allow this stage once", "Always allow in this workspace"] },
  { contains = ["Allow once", "Allow for this session"] },
  { contains = ["Yes, proceed", "Yes, don't ask again this session"] },
]

[[rules]]
id = "idle_prompt"
state = "idle"
priority = 700
region = "bottom_non_empty_lines(5)"
visible_idle = true
any = [
  { line_regex = ['^\s*⟩\s*$'] },
  { line_regex = ['^\s*⟩\s+\S'] },
]
not = [
  { contains = ["esc to interrupt"] },
  { contains = ["Enter to select", "Tab for an optional note"] },
  { contains = ["Enter to toggle", "Esc to interrupt"] },
]

[[rules]]
id = "idle_status_fallback"
state = "idle"
priority = 500
region = "bottom_non_empty_lines(3)"
visible_idle = true
line_regex = ['^\s*\S+ · (none|minimal|low|medium|high|xhigh|ultra) · ']
not = [{ contains = ["esc to interrupt"] }]
"#;

const MANIFEST_AIDER: &str = r#"
id = "aider"

[[rules]]
id = "confirmation_prompt"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(6)"
visible_blocker = true
any = [
  { line_regex = ['(?i)\((?:y\)es/\(n\)o|y/n)\)\s*\[[^\]]+\]:?\s*$'] },
  { contains = ["Run shell command?"] },
  { contains = ["Add ", "to the chat?"] },
  { contains = ["Create new file?"] },
  { contains = ["Allow creation of"] },
]

[[rules]]
id = "waiting_llm_working"
state = "working"
priority = 200
region = "bottom_non_empty_lines(6)"
visible_working = true
any = [
  { contains = ["Waiting for "] },
  { line_regex = ['^\s*[\u2800-\u28FF]+\s+\S'] },
]

[[rules]]
id = "prompt_idle"
state = "idle"
priority = 100
region = "bottom_non_empty_lines(3)"
visible_idle = true
line_regex = ['^(?:[a-zA-Z0-9_-]+\s+)?>\s*$']
"#;

const MANIFEST_GOOSE: &str = r#"
id = "goose"

[[rules]]
id = "tool_approval_blocked"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(10)"
visible_blocker = true
any = [
  { contains = ["Allow Goose to"] },
  { contains = ["Approve tool call"] },
  { all = [{ contains = ["Allow"] }, { contains = ["Deny"] }, { any = [{ contains = ["Always allow"] }, { contains = ["Allow once"] }] }] },
]

[[rules]]
id = "working_spinner"
state = "working"
priority = 200
region = "bottom_non_empty_lines(8)"
visible_working = true
any = [
  { line_regex = ['^\s*[\u2800-\u28FF]+\s+(?:Thinking|Working|Running|Generating|Calling)'] },
  { contains = ["esc to interrupt"] },
  { contains = ["ctrl+c to interrupt"] },
]

[[rules]]
id = "goose_prompt_idle"
state = "idle"
priority = 100
region = "bottom_non_empty_lines(4)"
visible_idle = true
line_regex = ['^\s*(?:O\)|\(>\)|goose>)\s*$']
"#;

const MANIFEST_CRUSH: &str = r#"
id = "crush"

[[rules]]
id = "permission_dialog"
state = "blocked"
priority = 300
region = "bottom_non_empty_lines(12)"
visible_blocker = true
any = [
  { contains = ["Permission required"] },
  { all = [{ contains = ["Allow"] }, { contains = ["Deny"] }, { contains = ["esc"] }] },
]

[[rules]]
id = "question_dialog"
state = "blocked"
priority = 290
region = "bottom_non_empty_lines(12)"
visible_blocker = true
contains = ["enter select", "quick select"]

[[rules]]
id = "working_hint"
state = "working"
priority = 200
region = "bottom_non_empty_lines(8)"
visible_working = true
any = [
  { contains = ["esc cancel"] },
  { contains = ["press again to cancel"] },
  { contains = ["esc to cancel"] },
  { contains = ["esc to interrupt"] },
  { line_regex = ['^\s*::: Working'] },
  { line_regex = ['^\s*[\u2801-\u28FF]+\s+\S'] },
]
"#;

const MANIFEST_ZCODE: &str = r#"
id = "zcode"

[[rules]]
id = "osc_progress_blocked"
state = "blocked"
priority = 1300
region = "osc_progress"
visible_blocker = true
regex = ['^4;3(?:;|$)']

[[rules]]
id = "osc_title_working"
state = "working"
priority = 1100
region = "osc_title"
visible_working = true
regex = ['^ZC \| [\x{2800}-\x{28FF}] ']

[[rules]]
id = "permission_blocked"
state = "blocked"
priority = 900
region = "bottom_non_empty_lines(10)"
visible_blocker = true
contains = ["waiting for permission"]

[[rules]]
id = "composer_idle"
state = "idle"
priority = 100
region = "bottom_non_empty_lines(6)"
visible_idle = true
contains = ["? for shortcuts"]
"#;

const MANIFEST_CLAUDE: &str = r#"
id = "claude"

[[rules]]
id = "osc_title_working"
state = "working"
priority = 1100
region = "osc_title"
visible_working = true
regex = ['^[\x{2800}-\x{28FF}\x{25D0}-\x{25D3}] ']

[[rules]]
id = "live_blocked_form"
state = "blocked"
priority = 980
region = "after_last_horizontal_rule"
visible_blocker = true
contains = ["esc to cancel"]
any = [
  { contains = ["enter to confirm"] },
  { contains = ["enter to select"] },
  { contains = ["do you want to proceed?"] },
]

[[rules]]
id = "live_turn_working"
state = "working"
priority = 970
region = "bottom_non_empty_lines(12)"
visible_working = true
any = [
  { line_regex = ['^\s*[⏸⏵].*esc to interrupt(?:\s|·|$)'] },
  { line_regex = ['^\s*[\x{002A}\x{00B7}\x{2722}\x{2733}\x{2736}\x{273B}\x{273D}]\s+\S.*…'] },
]

[[rules]]
id = "live_prompt_box"
state = "idle"
priority = 950
region = "prompt_box_body"
visible_idle = true
line_regex = ['^\s*❯']
not = [
  { contains = ["enter to select"] },
  { contains = ["esc to cancel"] },
]
"#;

const MANIFEST_PI: &str = r#"
id = "pi"

[[rules]]
id = "working_literal"
state = "working"
priority = 100
region = "whole_recent"
visible_working = true
any = [
  { contains = ["Working..."] },
  { line_regex = ['^[⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏] Working'] },
]
"#;

const MANIFEST_GENERIC: &str = r#"
id = "generic"

[[rules]]
id = "osc_title_blocked"
state = "blocked"
priority = 1100
region = "osc_title"
visible_blocker = true
contains = ["Action Required"]

[[rules]]
id = "osc_title_working"
state = "working"
priority = 1000
region = "osc_title"
visible_working = true
regex = ['(?:^|\s)[\x{2801}-\x{28FF}](?:\s|$)']

[[rules]]
id = "generic_permission"
state = "blocked"
priority = 400
region = "bottom_non_empty_lines(12)"
visible_blocker = true
any = [
  { contains = ["do you want to proceed?"] },
  { contains = ["allow execution"] },
  { contains = ["permission required"] },
  { contains = ["waiting for approval"] },
  { contains = ["press enter to confirm or esc to cancel"] },
]

[[rules]]
id = "generic_working"
state = "working"
priority = 200
region = "bottom_non_empty_lines(10)"
visible_working = true
any = [
  { contains = ["esc to interrupt"] },
  { contains = ["esc to cancel"] },
  { contains = ["ctrl+c to stop"] },
  { contains = ["ctrl+c to interrupt"] },
]
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_bundled_manifests_compile() {
        let map = manifests();
        for &(id, _) in BUNDLED_MANIFESTS {
            assert!(map.contains_key(id), "manifest {id} failed to compile");
        }
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
