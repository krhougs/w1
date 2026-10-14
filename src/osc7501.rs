use std::collections::BTreeMap;

use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};
use base64::Engine as _;

pub const MAX_SEQUENCE_BYTES: usize = 4096;
pub const MAX_PAYLOAD_BYTES: usize = 4096 - 8;
pub const MAX_TITLE_DECODED_BYTES: usize = 192;
pub const MAX_TITLE_ENCODED_BYTES: usize = 256;
pub const MAX_MSG_DECODED_BYTES: usize = 2048;
pub const MAX_MSG_ENCODED_BYTES: usize = 2732;
pub const MAX_APP_BYTES: usize = 32;
pub const MAX_ID_BYTES: usize = 128;
pub const MAX_ID_SEGMENT_BYTES: usize = 32;
pub const MAX_ID_DEPTH: usize = 8;
pub const MAX_RECORDS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum OscState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl OscState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Error => "error",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "idle" => Some(Self::Idle),
            "working" => Some(Self::Working),
            "done" => Some(Self::Done),
            "blocked" => Some(Self::Blocked),
            "error" => Some(Self::Error),
            _ => None,
        }
    }

    pub fn priority(self) -> u8 {
        match self {
            Self::Blocked => 4,
            Self::Error => 3,
            Self::Working => 2,
            Self::Done => 1,
            Self::Idle => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}

impl BlockedKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Auth => "auth",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "permission" => Some(Self::Permission),
            "question" => Some(Self::Question),
            "auth" => Some(Self::Auth),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OscRecord {
    pub state: OscState,
    pub id: Option<String>,
    pub kind: Option<BlockedKind>,
    pub progress: Option<u8>,
    pub app: Option<String>,
    pub title: Option<String>,
    pub msg: Option<String>,
}

impl OscRecord {
    pub fn new(state: OscState) -> Self {
        Self {
            state,
            id: None,
            kind: None,
            progress: None,
            app: None,
            title: None,
            msg: None,
        }
    }

    pub fn with_app(mut self, app: impl Into<String>) -> Self {
        self.app = sanitize_app(&app.into());
        self
    }

    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = sanitize_id(&id.into());
        self
    }

    pub fn with_kind(mut self, kind: Option<BlockedKind>) -> Self {
        self.kind = kind;
        self
    }

    pub fn with_progress(mut self, progress: Option<u8>) -> Self {
        self.progress = progress.map(|p| p.min(100));
        self
    }

    pub fn with_title(mut self, title: Option<String>) -> Self {
        self.title = title.and_then(|t| sanitize_single_line(&t, MAX_TITLE_DECODED_BYTES));
        self
    }

    pub fn with_msg(mut self, msg: Option<String>) -> Self {
        self.msg = msg.and_then(|m| sanitize_single_line(&m, MAX_MSG_DECODED_BYTES));
        self
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut pairs = Vec::with_capacity(7);
        pairs.push(format!("state={}", self.state.as_str()));

        if let Some(id) = self.id.as_deref().and_then(sanitize_id) {
            pairs.push(format!("id={id}"));
        }

        if self.state == OscState::Blocked {
            if let Some(kind) = self.kind {
                pairs.push(format!("kind={}", kind.as_str()));
            }
        }

        if matches!(self.state, OscState::Working | OscState::Blocked) {
            if let Some(progress) = self.progress {
                pairs.push(format!("progress={}", progress.min(100)));
            }
        }

        if let Some(app) = self.app.as_deref().and_then(sanitize_app) {
            pairs.push(format!("app={app}"));
        }

        if let Some(title) = self
            .title
            .as_deref()
            .and_then(|t| sanitize_single_line(t, MAX_TITLE_DECODED_BYTES))
        {
            let encoded = STANDARD.encode(title.as_bytes());
            if encoded.len() <= MAX_TITLE_ENCODED_BYTES {
                pairs.push(format!("title={encoded}"));
            }
        }

        if let Some(msg) = self
            .msg
            .as_deref()
            .and_then(|m| sanitize_single_line(m, MAX_MSG_DECODED_BYTES))
        {
            let encoded = STANDARD.encode(msg.as_bytes());
            if encoded.len() <= MAX_MSG_ENCODED_BYTES {
                pairs.push(format!("msg={encoded}"));
            }
        }

        let body = pairs.join(":");
        let mut out = Vec::with_capacity(body.len() + 9);
        out.extend_from_slice(b"\x1b]7501;");
        out.extend_from_slice(body.as_bytes());
        out.extend_from_slice(b"\x1b\\");
        out
    }
}

pub fn encode_clear(id: Option<&str>, app: Option<&str>) -> Vec<u8> {
    let mut pairs = vec!["state=clear".to_owned()];
    if let Some(app) = app.and_then(sanitize_app) {
        pairs.push(format!("app={app}"));
    }
    if let Some(id) = id.and_then(sanitize_id) {
        pairs.push(format!("id={id}"));
    }
    let body = pairs.join(":");
    let mut out = Vec::with_capacity(body.len() + 9);
    out.extend_from_slice(b"\x1b]7501;");
    out.extend_from_slice(body.as_bytes());
    out.extend_from_slice(b"\x1b\\");
    out
}

pub fn sanitize_app(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(trimmed.len().min(MAX_APP_BYTES));
    for ch in trimmed.chars() {
        if out.len() >= MAX_APP_BYTES {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-') {
            out.push(ch);
        } else if ch == ' ' {
            out.push('-');
        }
    }
    (!out.is_empty()).then_some(out)
}

pub fn sanitize_id_segment(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(trimmed.len().min(MAX_ID_SEGMENT_BYTES));
    for ch in trimmed.chars() {
        if out.len() >= MAX_ID_SEGMENT_BYTES {
            break;
        }
        if ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '+' | '-') {
            out.push(ch);
        } else if ch == ' ' || ch == ':' {
            out.push('-');
        }
    }
    (!out.is_empty()).then_some(out)
}

pub fn sanitize_id(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('/');
    if trimmed.is_empty() {
        return None;
    }
    let mut segments = Vec::new();
    for part in trimmed.split('/') {
        let seg = sanitize_id_segment(part)?;
        segments.push(seg);
        if segments.len() >= MAX_ID_DEPTH {
            break;
        }
    }
    if segments.is_empty() {
        return None;
    }
    let joined = segments.join("/");
    if joined.len() <= MAX_ID_BYTES {
        return Some(joined);
    }
    while segments.len() > 1 {
        segments.pop();
        let candidate = segments.join("/");
        if candidate.len() <= MAX_ID_BYTES {
            return Some(candidate);
        }
    }
    None
}

pub fn sanitize_single_line(raw: &str, max_bytes: usize) -> Option<String> {
    let mut normalized = String::with_capacity(raw.len().min(max_bytes));
    let mut last_was_space = false;

    for ch in raw.chars() {
        if ch.is_control() {
            if matches!(ch, '\n' | '\r' | '\t') && !last_was_space && !normalized.is_empty() {
                normalized.push(' ');
                last_was_space = true;
            }
            continue;
        }
        if is_invisible_or_bidi(ch) {
            continue;
        }
        if ch.is_whitespace() {
            if !last_was_space && !normalized.is_empty() {
                normalized.push(' ');
                last_was_space = true;
            }
        } else {
            normalized.push(ch);
            last_was_space = false;
        }
    }

    let trimmed = normalized.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_utf8(trimmed, max_bytes))
}

fn is_invisible_or_bidi(ch: char) -> bool {
    matches!(
        ch,
        '\u{061C}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    if max_bytes <= 3 {
        let mut end = max_bytes;
        while end > 0 && !text.is_char_boundary(end) {
            end -= 1;
        }
        return text[..end].to_owned();
    }
    let ellipsis = "…";
    let budget = max_bytes.saturating_sub(ellipsis.len());
    let mut end = budget.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].trim_end().to_owned();
    out.push_str(ellipsis);
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParsedReport {
    Clear { id: String },
    Record { id: String, record: OscRecord },
}

pub fn parse_report(payload: &str) -> Option<ParsedReport> {
    if payload.len() > MAX_PAYLOAD_BYTES || payload == "?" || payload.starts_with("?:") {
        return None;
    }
    let mut state = None;
    let mut id = String::new();
    let mut kind = None;
    let mut progress = None;
    let mut app = None;
    let mut title = None;
    let mut msg = None;

    for pair in payload.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if key.is_empty()
            || key.len() > 16
            || !key.bytes().all(|b| b.is_ascii_lowercase())
            || !value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b',' | b'+' | b'/' | b'=' | b'-'))
        {
            continue;
        }
        match key {
            "state" => state = Some(value),
            "id" => {
                if !valid_id_strict(value) {
                    return None;
                }
                id = value.to_owned();
            }
            "kind" => {
                if let Some(parsed) = BlockedKind::parse(value) {
                    kind = Some(parsed);
                } else {
                    kind = None;
                }
            }
            "progress" => {
                progress = value.parse::<u8>().ok().filter(|v| *v <= 100);
            }
            "app" => {
                if value.len() > MAX_APP_BYTES {
                    return None;
                }
                if valid_app_strict(value) {
                    app = Some(value.to_owned());
                } else {
                    app = None;
                }
            }
            "title" => {
                if value.len() > MAX_TITLE_ENCODED_BYTES {
                    return None;
                }
                title = Some(decode_base64_text(value, MAX_TITLE_DECODED_BYTES)?);
            }
            "msg" => {
                if value.len() > MAX_MSG_ENCODED_BYTES {
                    return None;
                }
                msg = Some(decode_base64_text(value, MAX_MSG_DECODED_BYTES)?);
            }
            _ => {}
        }
    }

    let state_str = state?;
    if state_str == "clear" {
        return Some(ParsedReport::Clear { id });
    }
    let state = OscState::parse(state_str)?;
    let record = OscRecord {
        state,
        id: (!id.is_empty()).then(|| id.clone()),
        kind: kind.filter(|_| state == OscState::Blocked),
        progress: progress.filter(|_| matches!(state, OscState::Working | OscState::Blocked)),
        app,
        title: title.filter(|t| !t.is_empty()),
        msg: msg.filter(|m| !m.is_empty()),
    };
    Some(ParsedReport::Record { id, record })
}

fn valid_app_strict(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_APP_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
}

fn valid_id_strict(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ID_BYTES
        && value.split('/').count() <= MAX_ID_DEPTH
        && value.split('/').all(|seg| {
            !seg.is_empty()
                && seg.len() <= MAX_ID_SEGMENT_BYTES
                && seg
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'+' | b'-'))
        })
}

fn decode_base64_text(encoded: &str, max_decoded: usize) -> Option<String> {
    let bytes = STANDARD
        .decode(encoded)
        .or_else(|_| STANDARD_NO_PAD.decode(encoded))
        .ok()?;
    if bytes.len() > max_decoded {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    (!text.chars().any(char::is_control)).then_some(text)
}

#[derive(Default)]
pub struct OscEmitter {
    app: String,
    emitted: BTreeMap<String, Vec<u8>>,
}

impl OscEmitter {
    pub fn new(app: impl Into<String>) -> Self {
        let app = sanitize_app(&app.into()).unwrap_or_else(|| "agent".to_owned());
        Self {
            app,
            emitted: BTreeMap::new(),
        }
    }

    pub fn app(&self) -> &str {
        &self.app
    }

    pub fn sync(&mut self, root: OscRecord, children: &[OscRecord], out: &mut Vec<u8>) {
        let mut desired: BTreeMap<String, Vec<u8>> = BTreeMap::new();

        let mut root_rec = root;
        if root_rec.app.is_none() {
            root_rec.app = Some(self.app.clone());
        }
        root_rec.id = None;
        desired.insert(String::new(), root_rec.encode());

        for child in children.iter().take(MAX_RECORDS.saturating_sub(1)) {
            let Some(id) = child.id.as_deref().and_then(sanitize_id) else {
                continue;
            };
            let mut child_rec = child.clone();
            child_rec.id = Some(id.clone());
            if child_rec.app.is_none() {
                child_rec.app = Some(self.app.clone());
            }
            desired.insert(id, child_rec.encode());
        }

        let removed: Vec<String> = self
            .emitted
            .keys()
            .filter(|id| !id.is_empty() && !desired.contains_key(*id))
            .cloned()
            .collect();

        for id in removed {
            out.extend_from_slice(&encode_clear(Some(&id), Some(&self.app)));
            self.emitted.remove(&id);
            let prefix = format!("{id}/");
            let descendants: Vec<String> = self
                .emitted
                .keys()
                .filter(|k| k.starts_with(&prefix))
                .cloned()
                .collect();
            for desc in descendants {
                self.emitted.remove(&desc);
            }
        }

        for (id, seq) in desired {
            if self.emitted.get(&id) != Some(&seq) {
                out.extend_from_slice(&seq);
                self.emitted.insert(id, seq);
            }
        }
    }

    /// Forget what was sent so the next sync re-reports everything (the outer terminal dropped
    /// its table, e.g. on RIS or a new shell prompt mark).
    pub fn invalidate(&mut self) {
        self.emitted.clear();
    }

    pub fn emit_exit(&mut self, final_record: Option<OscRecord>, out: &mut Vec<u8>) {
        let child_ids: Vec<String> = self
            .emitted
            .keys()
            .filter(|id| !id.is_empty())
            .cloned()
            .collect();
        for id in child_ids {
            out.extend_from_slice(&encode_clear(Some(&id), Some(&self.app)));
            self.emitted.remove(&id);
        }

        if let Some(mut rec) = final_record {
            rec.id = None;
            if rec.app.is_none() {
                rec.app = Some(self.app.clone());
            }
            if matches!(rec.state, OscState::Done | OscState::Error) {
                let seq = rec.encode();
                if self.emitted.get("") != Some(&seq) {
                    out.extend_from_slice(&seq);
                    self.emitted.insert(String::new(), seq);
                }
                return;
            }
        }

        if !self.emitted.is_empty() {
            out.extend_from_slice(&encode_clear(None, None));
            self.emitted.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_and_parse_roundtrip() {
        let rec = OscRecord::new(OscState::Blocked)
            .with_app("codex")
            .with_id("codex/sub-1")
            .with_kind(Some(BlockedKind::Permission))
            .with_progress(Some(40))
            .with_title(Some("Fix login bug\nline2".to_owned()))
            .with_msg(Some("Approve command: cargo test?".to_owned()));

        let encoded = String::from_utf8(rec.encode()).expect("valid utf8");
        assert!(encoded.starts_with("\x1b]7501;"));
        assert!(encoded.ends_with("\x1b\\"));
        let payload = &encoded[7..encoded.len() - 2];
        let parsed = parse_report(payload).expect("valid report");
        match parsed {
            ParsedReport::Record { id, record } => {
                assert_eq!(id, "codex/sub-1");
                assert_eq!(record.state, OscState::Blocked);
                assert_eq!(record.kind, Some(BlockedKind::Permission));
                assert_eq!(record.progress, Some(40));
                assert_eq!(record.app.as_deref(), Some("codex"));
                assert_eq!(record.title.as_deref(), Some("Fix login bug line2"));
                assert_eq!(record.msg.as_deref(), Some("Approve command: cargo test?"));
            }
            ParsedReport::Clear { .. } => panic!("expected record"),
        }
    }

    #[test]
    fn enforces_spec_limits_and_emitter_dedup() {
        let long_title = "🦀".repeat(100);
        let long_msg = "x".repeat(4000);
        let rec = OscRecord::new(OscState::Working)
            .with_app("codex")
            .with_progress(Some(150))
            .with_title(Some(long_title))
            .with_msg(Some(long_msg));
        let encoded = String::from_utf8(rec.encode()).expect("utf8");
        assert!(encoded.len() <= MAX_SEQUENCE_BYTES);
        let payload = &encoded[7..encoded.len() - 2];
        let parsed = parse_report(payload).expect("valid bounded report");
        match parsed {
            ParsedReport::Record { record, .. } => {
                assert_eq!(record.progress, Some(100));
                assert!(record.title.as_ref().map_or(0, String::len) <= MAX_TITLE_DECODED_BYTES);
                assert!(record.msg.as_ref().map_or(0, String::len) <= MAX_MSG_DECODED_BYTES);
            }
            ParsedReport::Clear { .. } => panic!("expected record"),
        }

        let mut emitter = OscEmitter::new("codex");
        let mut out = Vec::new();
        let root = OscRecord::new(OscState::Working).with_app("codex");
        let child = OscRecord::new(OscState::Working)
            .with_app("codex")
            .with_id("codex/sub1");
        emitter.sync(root.clone(), std::slice::from_ref(&child), &mut out);
        assert!(!out.is_empty());

        out.clear();
        emitter.sync(root.clone(), &[child], &mut out);
        assert!(out.is_empty(), "duplicate sync should not re-emit identical records");

        emitter.sync(root, &[], &mut out);
        let clear_str = String::from_utf8(out).expect("utf8");
        assert!(clear_str.contains("state=clear:app=codex:id=codex/sub1"));
    }
}
