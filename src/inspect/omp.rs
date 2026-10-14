//! oh-my-pi (`omp`): `$PI_CODING_AGENT_DIR/sessions/<cwd-slug>/<ts>_<id>.jsonl` (default agent dir
//! `~/.omp/agent`). The first line is a padded `title` record rewritten in place; later renames are
//! appended as `title_change`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::{
    claim_first, clean_text_summary, clean_title, content_text, env_dir_or_home, file_created_ms,
    file_mtime_ms, home_dir, json_str, list_dir, parse_iso_ms, read_first_line, todo_progress, Claim,
    ExternalInsight, JsonlTail, LaunchContext, Tracker,
};

const SCAN_INTERVAL: Duration = Duration::from_millis(1500);
const HEADER_BYTES: usize = 64 * 1024;

#[derive(Default)]
pub struct OmpTracker {
    session: Option<(PathBuf, u64)>,
    claim: Option<Claim>,
    tail: JsonlTail,
    last_scan: Option<Instant>,
}

impl Tracker for OmpTracker {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight) {
        if self.last_scan.is_none_or(|t| t.elapsed() >= SCAN_INTERVAL) {
            self.last_scan = Some(Instant::now());
            self.scan(ctx);
        }
        let Some((path, _)) = self.session.clone() else {
            return;
        };
        if let Some(title) = read_first_line(&path, HEADER_BYTES)
            .and_then(|l| serde_json::from_str::<Value>(&l).ok())
            .filter(|v| json_str(v, &["type"]) == Some("title"))
            .and_then(|v| json_str(&v, &["title"]).and_then(clean_title))
        {
            insight.title = Some(title);
        }
        self.tail.open(path);
        for line in self.tail.read_lines() {
            apply_line(&line, ctx, insight);
        }
    }
}

impl OmpTracker {
    fn scan(&mut self, ctx: &LaunchContext) {
        let Some(root) = agent_dir(ctx) else {
            return;
        };
        let min = ctx.min_created_ms();
        let current_created = self.session.as_ref().map(|(_, c)| *c);
        let mut candidates: Vec<(u64, PathBuf)> = Vec::new();
        for dir in list_dir(&root.join("sessions")) {
            for file in list_dir(&dir) {
                if file.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if self.session.as_ref().is_some_and(|(p, _)| *p == file) {
                    continue;
                }
                let created = file_created_ms(&file).unwrap_or(0);
                let updated = file_mtime_ms(&file).unwrap_or(0);
                if !ctx.session_time_ok(created, updated) {
                    continue;
                }
                if current_created.is_some_and(|c| created <= c || !ctx.created_by_our_submit(created)) {
                    continue;
                }
                if session_cwd(&file).is_some_and(|cwd| ctx.matches_cwd(&cwd)) {
                    candidates.push((if created >= min { created } else { updated }, file));
                }
            }
        }
        candidates.sort();
        if let Some(((created, path), claim)) = claim_first("omp", candidates, |(_, p)| {
            p.to_string_lossy().into_owned()
        }) {
            self.claim = Some(claim);
            self.session = Some((path, created));
        }
    }
}

fn agent_dir(ctx: &LaunchContext) -> Option<PathBuf> {
    let profile = ctx
        .arg_value(&["--profile"])
        .or_else(|| std::env::var("OMP_PROFILE").ok())
        .or_else(|| std::env::var("PI_PROFILE").ok())
        .filter(|p| !p.is_empty() && !p.contains('/'));
    match profile {
        Some(p) => home_dir().map(|h| h.join(".omp/profiles").join(p).join("agent")),
        None => env_dir_or_home("PI_CODING_AGENT_DIR", ".omp/agent"),
    }
}

fn session_cwd(path: &Path) -> Option<String> {
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(std::io::Read::take(file, HEADER_BYTES as u64 * 2));
    for line in std::io::BufRead::lines(reader).take(4).map_while(Result::ok) {
        let Ok(v) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if json_str(&v, &["type"]) == Some("session") {
            return json_str(&v, &["cwd"]).map(str::to_owned);
        }
    }
    None
}

fn apply_line(line: &Value, ctx: &LaunchContext, insight: &mut ExternalInsight) {
    let live = json_str(line, &["timestamp"])
        .and_then(parse_iso_ms)
        .is_none_or(|ts| ts >= ctx.min_created_ms());
    match json_str(line, &["type"]).unwrap_or("") {
        "title_change" => {
            if let Some(t) = json_str(line, &["title"]).and_then(clean_title) {
                insight.title = Some(t);
            }
        }
        "message" if live => {
            let Some(msg) = line.get("message") else {
                return;
            };
            match json_str(msg, &["role"]).unwrap_or("") {
                "user" => {
                    if json_str(msg, &["attribution"]).is_none_or(|a| a == "user") {
                        insight.begin_turn();
                    }
                }
                "assistant" => {
                    let text = msg.get("content").and_then(|c| content_text(c, &["text"]));
                    let summary = text.as_deref().and_then(clean_text_summary);
                    match json_str(msg, &["stopReason"]).unwrap_or("") {
                        "stop" | "end_turn" => insight.finish_turn(summary),
                        "error" => {
                            let err = json_str(msg, &["errorMessage"])
                                .and_then(clean_text_summary)
                                .or(summary);
                            insight.fail_turn(err);
                        }
                        "aborted" => insight.abort_turn(),
                        _ => {
                            if summary.is_some() {
                                insight.working_msg = summary;
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        "custom" if live => match json_str(line, &["customType"]).unwrap_or("") {
            "tool_execution_start" => {
                if let Some(intent) = json_str(line, &["data", "intent"]).and_then(clean_text_summary) {
                    insight.working_msg = Some(intent);
                }
            }
            "todo_hud_state" => {
                let lists: Value = json_str(line, &["data", "fingerprint"])
                    .and_then(|f| serde_json::from_str(f).ok())
                    .unwrap_or(Value::Null);
                let tasks = lists
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|l| l.get("tasks").and_then(Value::as_array))
                    .flatten()
                    .map(|t| {
                        (
                            json_str(t, &["status"]).unwrap_or(""),
                            json_str(t, &["content"]).unwrap_or(""),
                        )
                    });
                if let Some((pct, active)) = todo_progress(tasks) {
                    insight.progress = Some(pct);
                    if active.is_some() {
                        insight.working_msg = active;
                    }
                }
            }
            _ => {}
        },
        _ => {}
    }
}
