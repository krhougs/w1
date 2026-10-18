use std::sync::LazyLock;

use regex::Regex;

use super::regions::bottom_non_empty_lines;
use super::rules::{DetectedState, DetectionInput};
use crate::agents::AgentKind;

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
    agent: Option<AgentKind>,
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

    if let Some(detail) = agent
        .and_then(|a| a.profile().hooks.blocked_detail)
        .and_then(|hook| hook(rule_id, &lines))
    {
        return Some(detail);
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
    if let Some(detail) = agent
        .and_then(|a| a.profile().hooks.working_detail)
        .and_then(|hook| hook(osc_title))
    {
        return Some(detail);
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
