//! What session-store trackers share: the launch context that ties an on-disk session to this
//! `w1` instance, the insight they report, session claims, and file/SQLite/JSONL helpers.

use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::agents::AgentKind;
use crate::osc7501::{BlockedKind, OscRecord, OscState};

pub const POLL_INTERVAL: Duration = Duration::from_millis(500);
const SQLITE_TIMEOUT: Duration = Duration::from_millis(800);
const JSONL_READ_BUDGET: usize = 4 * 1024 * 1024;
const MAX_JSONL_LINE: usize = 8 * 1024 * 1024;
/// Sessions created slightly before `launch_ms` (clock skew, agent start-up racing w1) still count.
const CREATED_SLACK_MS: u64 = 3_000;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExternalInsight {
    pub state: Option<OscState>,
    pub blocked_kind: Option<BlockedKind>,
    pub progress: Option<u8>,
    pub title: Option<String>,
    pub working_msg: Option<String>,
    pub done_msg: Option<String>,
    pub error_msg: Option<String>,
    pub blocked_msg: Option<String>,
    pub children: Vec<OscRecord>,
}

impl ExternalInsight {
    pub(crate) fn begin_turn(&mut self) {
        self.state = Some(OscState::Working);
        self.working_msg = None;
        self.done_msg = None;
        self.error_msg = None;
        self.blocked_msg = None;
        self.blocked_kind = None;
        self.progress = None;
    }

    pub(crate) fn finish_turn(&mut self, msg: Option<String>) {
        self.state = Some(OscState::Done);
        self.working_msg = None;
        self.blocked_msg = None;
        self.blocked_kind = None;
        if msg.is_some() {
            self.done_msg = msg;
        }
    }

    pub(crate) fn fail_turn(&mut self, msg: Option<String>) {
        self.state = Some(OscState::Error);
        self.working_msg = None;
        self.error_msg = msg;
    }

    pub(crate) fn abort_turn(&mut self) {
        self.state = Some(OscState::Idle);
        self.working_msg = None;
        self.blocked_msg = None;
        self.blocked_kind = None;
    }
}

/// Everything a tracker needs to decide which on-disk session belongs to this `w1` instance.
#[derive(Clone, Debug)]
pub struct LaunchContext {
    /// The working directory in every spelling an agent may record it with
    /// (logical `$PWD`, `getcwd`, canonical path).
    pub cwd_variants: Vec<String>,
    pub launch_ms: u64,
    /// The user asked the agent to continue an existing session, so a session created before
    /// launch may legitimately be ours.
    pub resume: bool,
    /// The agent's argv (program first).
    pub args: Vec<String>,
    /// Pid of the process `w1` spawned; sessions recorded by one of its descendants are ours.
    pub child_pid: Option<i32>,
    pub submits: SubmitLog,
}

/// When the user submitted input in this terminal, shared between the PTY loop and the tracker.
#[derive(Clone, Debug, Default)]
pub struct SubmitLog(Arc<Mutex<Vec<u64>>>);

const SUBMIT_LOG_LEN: usize = 64;
/// A session row may be stamped slightly before the keypress reaches `w1`, and an agent can take
/// a while (model warm-up, plugins) before it persists a session for a submitted prompt.
const SUBMIT_LEAD_MS: u64 = 1500;
const SUBMIT_LAG_MS: u64 = 15_000;

impl SubmitLog {
    pub fn record(&self) {
        if let Ok(mut log) = self.0.lock() {
            if log.len() >= SUBMIT_LOG_LEN {
                log.remove(0);
            }
            log.push(now_ms());
        }
    }

    fn any(&self, f: impl Fn(u64) -> bool) -> bool {
        self.0.lock().is_ok_and(|log| log.iter().any(|s| f(*s)))
    }
}

impl LaunchContext {
    pub fn new(agent: Option<AgentKind>, args: &[String], child_pid: Option<i32>) -> Self {
        let mut cwd_variants: Vec<String> = Vec::new();
        let mut push = |p: &Path| {
            let s = p.to_string_lossy().trim_end_matches('/').to_owned();
            if !s.is_empty() && !cwd_variants.contains(&s) {
                cwd_variants.push(s);
            }
        };
        let physical = std::env::current_dir().ok();
        if let Some(pwd) = std::env::var_os("PWD").map(PathBuf::from) {
            let same_dir = match (fs::canonicalize(&pwd), physical.as_deref().map(fs::canonicalize)) {
                (Ok(a), Some(Ok(b))) => a == b,
                _ => false,
            };
            if same_dir {
                push(&pwd);
            }
        }
        if let Some(dir) = physical.as_deref() {
            push(dir);
            if let Ok(canonical) = fs::canonicalize(dir) {
                push(&canonical);
            }
        }
        let resume = agent.is_some_and(|a| is_resume_invocation(a, args));
        Self {
            cwd_variants,
            launch_ms: now_ms(),
            resume,
            args: args.to_vec(),
            child_pid,
            submits: SubmitLog::default(),
        }
    }

    /// Whether a session created at `created_ms` was started by input typed into this `w1`
    /// (a first prompt, `/new`, `/clear`), rather than by another instance in the same directory.
    pub fn created_by_our_submit(&self, created_ms: u64) -> bool {
        self.submits
            .any(|s| created_ms + SUBMIT_LEAD_MS >= s && created_ms <= s + SUBMIT_LAG_MS)
    }

    /// Whether a prompt the agent timestamped at `prompt_ms` is one typed into this `w1`.
    pub fn prompted_here(&self, prompt_ms: u64) -> bool {
        self.submits.any(|s| prompt_ms.abs_diff(s) <= SUBMIT_LEAD_MS)
    }

    pub fn matches_cwd(&self, recorded: &str) -> bool {
        let recorded = recorded.trim().trim_end_matches('/');
        let recorded = recorded.strip_prefix("file://").unwrap_or(recorded);
        if self.cwd_variants.iter().any(|v| v == recorded) {
            return true;
        }
        fs::canonicalize(recorded)
            .ok()
            .is_some_and(|c| self.cwd_variants.iter().any(|v| Path::new(v) == c))
    }

    pub fn min_created_ms(&self) -> u64 {
        self.launch_ms.saturating_sub(CREATED_SLACK_MS)
    }

    /// Whether a session with the given creation / last-update times may belong to this launch.
    pub fn session_time_ok(&self, created_ms: u64, updated_ms: u64) -> bool {
        if created_ms >= self.min_created_ms() {
            return true;
        }
        self.resume && updated_ms >= self.min_created_ms()
    }

    /// Value of `--name value` / `--name=value` in the agent's argv.
    pub fn arg_value(&self, names: &[&str]) -> Option<String> {
        let mut iter = self.args.iter().skip(1);
        while let Some(arg) = iter.next() {
            if arg == "--" {
                break;
            }
            if let Some((key, value)) = arg.split_once('=') {
                if names.contains(&key) {
                    return Some(value.to_owned());
                }
            } else if names.contains(&arg.as_str()) {
                return iter.next().cloned();
            }
        }
        None
    }

    pub fn sql_cwd_list(&self) -> String {
        self.cwd_variants
            .iter()
            .map(|v| format!("'{}'", sql_escape_literal(v)))
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn is_resume_invocation(agent: AgentKind, args: &[String]) -> bool {
    let resume = agent.profile().resume;
    let has_flag = args.iter().skip(1).any(|a| {
        let key = a.split('=').next().unwrap_or(a);
        resume.flags.contains(&key)
    });
    let first_positional = args
        .iter()
        .skip(1)
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
    has_flag || first_positional.is_some_and(|p| resume.subcommands.contains(&p))
}

pub(crate) trait Tracker: Send {
    fn poll(&mut self, ctx: &LaunchContext, insight: &mut ExternalInsight);
}

pub struct SessionInspector {
    ctx: LaunchContext,
    tracker: Option<Box<dyn Tracker>>,
    insight: ExternalInsight,
}

impl SessionInspector {
    pub fn new(agent: Option<AgentKind>, ctx: LaunchContext) -> Self {
        let tracker = agent.and_then(|a| a.profile().tracker).map(|new| new());
        Self {
            ctx,
            tracker,
            insight: ExternalInsight::default(),
        }
    }

    pub fn has_tracker(&self) -> bool {
        self.tracker.is_some()
    }

    pub fn poll(&mut self) -> ExternalInsight {
        if let Some(tracker) = self.tracker.as_mut() {
            tracker.poll(&self.ctx, &mut self.insight);
        }
        self.insight.clone()
    }
}

// ============================================================================
// Session claims: several `w1` instances in the same directory must not adopt the same session.
// ============================================================================

pub struct Claim {
    _file: Option<fs::File>,
}

pub fn try_claim(agent: &str, session_key: &str) -> Option<Claim> {
    let Some(dir) = claims_dir() else {
        return Some(Claim { _file: None });
    };
    let mut hasher = Sha256::new();
    hasher.update(agent.as_bytes());
    hasher.update([0]);
    hasher.update(session_key.as_bytes());
    let digest = hasher.finalize();
    let name: String = digest.iter().take(16).map(|b| format!("{b:02x}")).collect();
    let Ok(file) = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(name))
    else {
        return Some(Claim { _file: None });
    };
    // SAFETY: flock on a descriptor we own; the lock lives as long as `file`.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Some(Claim { _file: Some(file) });
    }
    match std::io::Error::last_os_error().kind() {
        std::io::ErrorKind::WouldBlock => None,
        _ => Some(Claim { _file: None }),
    }
}

fn claims_dir() -> Option<PathBuf> {
    // SAFETY: getuid has no preconditions.
    let uid = unsafe { libc::getuid() };
    let dir = std::env::temp_dir().join(format!("wrap7501-{uid}"));
    if !dir.is_dir() {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .ok()?;
    }
    Some(dir)
}

/// Picks the first candidate (already ordered by preference) that no other `w1` holds.
pub fn claim_first<T>(
    agent: &str,
    candidates: impl IntoIterator<Item = T>,
    key: impl Fn(&T) -> String,
) -> Option<(T, Claim)> {
    for candidate in candidates {
        if let Some(claim) = try_claim(agent, &key(&candidate)) {
            return Some((candidate, claim));
        }
    }
    None
}

// ============================================================================
// Shared helpers
// ============================================================================

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

pub fn env_dir_or_home(var: &str, home_rel: &str) -> Option<PathBuf> {
    std::env::var_os(var)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|h| h.join(home_rel)))
}

fn system_time_ms(t: SystemTime) -> Option<u64> {
    t.duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis() as u64)
}

pub fn file_mtime_ms(path: &Path) -> Option<u64> {
    system_time_ms(fs::metadata(path).ok()?.modified().ok()?)
}

/// Birth time where the filesystem records it, otherwise the modification time.
pub fn file_created_ms(path: &Path) -> Option<u64> {
    let meta = fs::metadata(path).ok()?;
    meta.created()
        .ok()
        .and_then(system_time_ms)
        .or_else(|| meta.modified().ok().and_then(system_time_ms))
}

/// Parses RFC 3339 / ISO 8601 UTC timestamps such as `2026-10-13T20:16:18.549Z`.
pub fn parse_iso_ms(raw: &str) -> Option<u64> {
    let s = raw.trim();
    let (date, rest) = s.split_once('T')?;
    let mut dp = date.split('-');
    let year: i64 = dp.next()?.parse().ok()?;
    let month: i64 = dp.next()?.parse().ok()?;
    let day: i64 = dp.next()?.parse().ok()?;
    let rest = rest.strip_suffix('Z').unwrap_or(rest);
    let (time, offset_secs) = split_offset(rest)?;
    let mut tp = time.split(':');
    let hour: i64 = tp.next()?.parse().ok()?;
    let minute: i64 = tp.next()?.parse().ok()?;
    let sec_part = tp.next().unwrap_or("0");
    let (sec_str, frac_str) = sec_part.split_once('.').unwrap_or((sec_part, ""));
    let second: i64 = sec_str.parse().ok()?;
    let millis: i64 = if frac_str.is_empty() {
        0
    } else {
        let digits: String = frac_str.chars().take(3).collect();
        let padded = format!("{digits:0<3}");
        padded.parse().ok()?
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let days = days_from_civil(year, month, day);
    let total = ((days * 24 + hour) * 60 + minute) * 60 + second - offset_secs;
    u64::try_from(total * 1000 + millis).ok()
}

fn split_offset(time: &str) -> Option<(&str, i64)> {
    for (idx, ch) in time.char_indices().skip(1) {
        if ch == '+' || ch == '-' {
            let (t, off) = time.split_at(idx);
            let sign = if ch == '-' { -1 } else { 1 };
            let off = off.get(1..)?;
            let (h, m) = off.split_once(':').unwrap_or((off.get(..2)?, off.get(2..).unwrap_or("0")));
            let secs = h.parse::<i64>().ok()? * 3600 + m.parse::<i64>().unwrap_or(0) * 60;
            return Some((t, sign * secs));
        }
    }
    Some((time, 0))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// First meaningful line of a (markdown) message, single-lined and bounded.
pub fn clean_text_summary(raw: &str) -> Option<String> {
    let mut in_fence = false;
    for l in raw.lines() {
        let t = l.trim();
        if t.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let t = t.trim_start_matches(['#', '*', '-', '>', '•', '`', ' ']).trim();
        if t.is_empty() || t.starts_with('<') || t.chars().all(|c| matches!(c, '-' | '=' | '|' | ':' | ' ')) {
            continue;
        }
        let t = t.replace("**", "").replace('`', "");
        return crate::osc7501::sanitize_single_line(&t, 280);
    }
    None
}

/// Strips private-use glyphs (Nerd Font icons) and stray symbols that agents prefix titles with.
pub fn clean_title(raw: &str) -> Option<String> {
    let filtered: String = raw
        .chars()
        .filter(|c| !matches!(*c as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD | 0x100000..=0x10FFFD))
        .collect();
    let trimmed = filtered.trim_start_matches(|c: char| c.is_whitespace() || c == '·');
    clean_text_summary(trimmed)
}

pub fn sql_escape_literal(s: &str) -> String {
    s.replace('\'', "''")
}

/// Runs a read-only query through the `sqlite3` CLI and returns its rows as JSON objects.
pub fn sqlite_rows(db_path: &Path, sql: &str) -> Option<Vec<serde_json::Map<String, Value>>> {
    let uri = format!("file:{}?mode=ro", db_path.display());
    let mut child = Command::new("sqlite3")
        .args(["-readonly", "-batch", "-json", "-cmd", ".timeout 200", &uri, sql])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::Builder::new()
        .name("w1-sqlite".into())
        .spawn(move || {
            let mut out = Vec::new();
            let _ = stdout.read_to_end(&mut out);
            out
        })
        .ok();
    let deadline = Instant::now() + SQLITE_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break None,
        }
    };
    let out = reader.and_then(|h| h.join().ok()).unwrap_or_default();
    if !status?.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Some(Vec::new());
    }
    let parsed: Value = serde_json::from_str(trimmed).ok()?;
    Some(
        parsed
            .as_array()?
            .iter()
            .filter_map(|row| row.as_object().cloned())
            .collect(),
    )
}

pub fn row_str<'a>(row: &'a serde_json::Map<String, Value>, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

pub fn row_u64(row: &serde_json::Map<String, Value>, key: &str) -> Option<u64> {
    let v = row.get(key)?;
    v.as_u64()
        .or_else(|| v.as_i64().and_then(|i| u64::try_from(i).ok()))
        .or_else(|| v.as_f64().filter(|f| *f >= 0.0).map(|f| f as u64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Incrementally tails an append-only JSONL file, yielding complete lines only.
#[derive(Default)]
pub struct JsonlTail {
    path: Option<PathBuf>,
    offset: u64,
    partial: Vec<u8>,
    skipping_oversized: bool,
}

impl JsonlTail {
    pub fn open(&mut self, path: PathBuf) {
        if self.path.as_deref() != Some(path.as_path()) {
            self.path = Some(path);
            self.offset = 0;
            self.partial.clear();
            self.skipping_oversized = false;
        }
    }

    /// Follows only what is appended from now on (resumed sessions replay nothing).
    pub fn open_at_end(&mut self, path: PathBuf) {
        let len = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        self.path = Some(path);
        self.offset = len;
        self.partial.clear();
        self.skipping_oversized = false;
    }

    pub fn read_lines(&mut self) -> Vec<Value> {
        let Some(path) = self.path.as_deref() else {
            return Vec::new();
        };
        let Ok(mut file) = fs::File::open(path) else {
            return Vec::new();
        };
        let Ok(meta) = file.metadata() else {
            return Vec::new();
        };
        if meta.len() < self.offset {
            self.offset = 0;
            self.partial.clear();
            self.skipping_oversized = false;
        }
        if meta.len() == self.offset || file.seek(SeekFrom::Start(self.offset)).is_err() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut chunk = vec![0u8; 64 * 1024];
        let mut budget = JSONL_READ_BUDGET;
        while budget > 0 {
            let n = match file.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            budget = budget.saturating_sub(n);
            self.offset += n as u64;
            let mut rest = chunk.get(..n).unwrap_or_default();
            while let Some(pos) = rest.iter().position(|b| *b == b'\n') {
                let (head, tail) = rest.split_at(pos);
                if !self.skipping_oversized {
                    self.partial.extend_from_slice(head);
                    if let Ok(v) = serde_json::from_slice::<Value>(&self.partial) {
                        out.push(v);
                    }
                }
                self.partial.clear();
                self.skipping_oversized = false;
                rest = tail.get(1..).unwrap_or_default();
            }
            if !self.skipping_oversized {
                self.partial.extend_from_slice(rest);
                if self.partial.len() > MAX_JSONL_LINE {
                    self.partial.clear();
                    self.skipping_oversized = true;
                }
            }
        }
        out
    }
}

pub fn read_first_line(path: &Path, max: usize) -> Option<String> {
    let mut file = fs::File::open(path).ok()?;
    let mut buf = vec![0u8; max];
    let mut filled = 0;
    while filled < max {
        match file.read(buf.get_mut(filled..)?) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                filled += n;
                if buf.get(..filled)?.contains(&b'\n') {
                    break;
                }
            }
        }
    }
    let data = buf.get(..filled)?;
    let end = data.iter().position(|b| *b == b'\n').unwrap_or(data.len());
    Some(String::from_utf8_lossy(data.get(..end)?).into_owned())
}

pub fn list_dir(path: &Path) -> Vec<PathBuf> {
    fs::read_dir(path)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default()
}

pub fn json_str<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str()
}

/// Concatenates `text` fields of a content array (or returns a plain string content).
pub fn content_text(content: &Value, text_types: &[&str]) -> Option<String> {
    if let Some(s) = content.as_str() {
        return (!s.trim().is_empty()).then(|| s.to_owned());
    }
    let mut out = String::new();
    for block in content.as_array()? {
        let ty = block.get("type").and_then(Value::as_str).unwrap_or("text");
        if !text_types.is_empty() && !text_types.contains(&ty) {
            continue;
        }
        if let Some(t) = block.get("text").and_then(Value::as_str) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(t);
        }
    }
    (!out.trim().is_empty()).then_some(out)
}

/// Progress percentage and active item from a list of `(status, text)` todo entries.
pub fn todo_progress<'a>(
    items: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<(u8, Option<String>)> {
    let mut total = 0usize;
    let mut finished = 0usize;
    let mut active = None;
    for (status, text) in items {
        total += 1;
        match status {
            "completed" | "done" | "cancelled" | "canceled" | "skipped" => finished += 1,
            "in_progress" | "in-progress" | "active" | "running" if active.is_none() => {
                active = clean_text_summary(text);
            }
            _ => {}
        }
    }
    (total > 0).then(|| (((finished * 100) / total).min(100) as u8, active))
}

pub fn short_id(raw: &str, max_chars: usize) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .take(max_chars)
        .collect()
}
