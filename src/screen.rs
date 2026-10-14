use alacritty_terminal::event::VoidListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi;

const AGENT_OSC_MAX_CHARS: usize = 256;

#[derive(Clone, Copy)]
struct TermDims {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermDims {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

pub struct VirtualScreen {
    term: Term<VoidListener>,
    processor: ansi::Processor,
    alt_normalizer: AltScreenNormalizer,
    osc_tracker: OscStateTracker,
    cols: usize,
    rows: usize,
}

impl VirtualScreen {
    pub fn new(cols: u16, rows: u16) -> Self {
        let cols = (cols as usize).max(20);
        let rows = (rows as usize).max(5);
        let dims = TermDims { cols, rows };
        let config = Config {
            scrolling_history: 200,
            ..Default::default()
        };
        Self {
            term: Term::new(config, &dims, VoidListener),
            processor: ansi::Processor::new(),
            alt_normalizer: AltScreenNormalizer::default(),
            osc_tracker: OscStateTracker::default(),
            cols,
            rows,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.osc_tracker.observe(bytes);
        let normalized = self.alt_normalizer.normalize(bytes);
        self.processor.advance(&mut self.term, &normalized);
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        let cols = (cols as usize).max(20);
        let rows = (rows as usize).max(5);
        if cols == self.cols && rows == self.rows {
            return;
        }
        if cols != self.cols && rows != self.rows {
            self.term.resize(TermDims {
                cols,
                rows: self.rows,
            });
        }
        self.term.resize(TermDims { cols, rows });
        self.cols = cols;
        self.rows = rows;
    }

    pub fn is_alternate_screen(&self) -> bool {
        self.term.mode().contains(TermMode::ALT_SCREEN)
    }

    pub fn osc_title(&self) -> &str {
        self.osc_tracker.latest_title()
    }

    pub fn osc_progress(&self) -> &str {
        self.osc_tracker.latest_progress()
    }

    /// Clean human-readable session title stripped of spinners, status prefixes, and padding.
    pub fn clean_title(&self, app_id: &str, display_name: &str) -> Option<String> {
        let raw = self.osc_tracker.latest_title().trim();
        if raw.is_empty() {
            return None;
        }
        clean_osc_title(raw, app_id, display_name)
    }

    pub fn take_prompt_marks(&mut self) -> Vec<PromptMark> {
        std::mem::take(&mut self.osc_tracker.marks)
    }

    pub fn parsed_osc_progress_percent(&self) -> Option<u8> {
        let raw = self.osc_tracker.latest_progress();
        let rest = raw.strip_prefix("4;")?;
        let mut parts = rest.split(';');
        let state = parts.next()?;
        let pct = parts.next()?;
        if matches!(state, "1" | "2" | "4") {
            pct.parse::<u8>().ok().filter(|v| *v <= 100)
        } else {
            None
        }
    }

    pub fn viewport_text(&self) -> String {
        let mut lines = Vec::with_capacity(self.term.screen_lines());
        let mut line = String::with_capacity(self.term.columns());
        let mut current_line = None;

        for indexed in self.term.renderable_content().display_iter {
            if current_line != Some(indexed.point.line) {
                if current_line.is_some() {
                    lines.push(trim_end_spaces(line));
                    line = String::with_capacity(self.term.columns());
                }
                current_line = Some(indexed.point.line);
            }

            let cell = indexed.cell;
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }
            line.push(if cell.c == '\t' { ' ' } else { cell.c });
            if let Some(zw) = cell.zerowidth() {
                line.extend(zw);
            }
        }

        if current_line.is_some() {
            lines.push(trim_end_spaces(line));
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }
}

fn trim_end_spaces(mut s: String) -> String {
    let trimmed_len = s.trim_end_matches(' ').len();
    s.truncate(trimmed_len);
    s
}

/// Strips leading spinner glyphs, status indicators (`Action Required`, `Ready`, `Working…`),
/// and trailing application name suffixes (e.g. ` - grok`, ` - amp - `) to recover a clean
/// session/task title when the agent sets one via OSC 0/2.
pub fn clean_osc_title(raw: &str, app_id: &str, display_name: &str) -> Option<String> {
    let mut s = raw.trim();
    if s.is_empty() {
        return None;
    }

    for prefix in [
        "ZC | ",
        "[ ! ] Action Required | ",
        "[ . ] Action Required | ",
        "[ ! ] Action Required",
        "[ . ] Action Required",
        "⚠ Action Required - ",
        "⚠ Action Required: ",
        "⚠ Action Required",
        "✋  Action Required - ",
        "✋  Action Required",
        "✋ Action Required",
        "Action Required - ",
        "Action Required: ",
        "Action Required | ",
        "⏲  Working…",
        "⏲ Working…",
        "◇  Ready",
        "◇ Ready",
    ] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.trim_start();
            break;
        }
    }

    loop {
        let mut chars = s.chars();
        let Some(first) = chars.next() else {
            break;
        };
        if is_title_spinner_or_badge(first) {
            s = chars.as_str().trim_start_matches(['\u{FE0E}', '\u{FE0F}', ' ', '-', '|', ':']);
        } else {
            break;
        }
    }

    for suffix in [" - grok", " - Claude Code", " - Codex", " - OpenCode"] {
        if let Some(stripped) = s.strip_suffix(suffix) {
            s = stripped.trim_end();
        }
    }
    if let Some(idx) = s.find(" - amp - ") {
        s = s[..idx].trim();
    }

    let s = s.trim();
    if s.is_empty()
        || s.eq_ignore_ascii_case(app_id)
        || s.eq_ignore_ascii_case(display_name)
        || s.eq_ignore_ascii_case("Action Required")
        || s.eq_ignore_ascii_case("Working…")
        || s.eq_ignore_ascii_case("Working...")
        || s.eq_ignore_ascii_case("Ready")
    {
        return None;
    }

    crate::osc7501::sanitize_single_line(s, crate::osc7501::MAX_TITLE_DECODED_BYTES)
}

fn is_title_spinner_or_badge(ch: char) -> bool {
    matches!(
        ch,
        '\u{2800}'..='\u{28FF}'
            | '\u{25D0}'..='\u{25D3}'
            | '✳'
            | '✦'
            | '⏳'
            | '⚠'
            | '✓'
            | '✋'
            | '⏲'
            | '◇'
    )
}

#[derive(Default)]
enum OscScanState {
    #[default]
    Ground,
    Escape,
    OscBody,
    OscEscape,
}

/// Semantic prompt marks (OSC 133) seen in the child's output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptMark {
    PromptStart,
    CommandStart,
    CommandFinished(Option<i32>),
}

const MAX_PENDING_MARKS: usize = 32;

#[derive(Default)]
struct OscStateTracker {
    state: OscScanState,
    body: Vec<u8>,
    latest_title: Option<String>,
    latest_progress: Option<String>,
    marks: Vec<PromptMark>,
}

impl OscStateTracker {
    fn observe(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            match self.state {
                OscScanState::Ground => {
                    if byte == 0x1b {
                        self.state = OscScanState::Escape;
                    }
                }
                OscScanState::Escape => {
                    if byte == b']' {
                        self.body.clear();
                        self.state = OscScanState::OscBody;
                    } else if byte == 0x1b {
                        self.state = OscScanState::Escape;
                    } else {
                        self.state = OscScanState::Ground;
                    }
                }
                OscScanState::OscBody => match byte {
                    0x07 => {
                        self.finalize();
                        self.state = OscScanState::Ground;
                    }
                    0x1b => self.state = OscScanState::OscEscape,
                    _ => self.body.push(byte),
                },
                OscScanState::OscEscape => {
                    if byte == b'\\' {
                        self.finalize();
                        self.state = OscScanState::Ground;
                    } else {
                        self.body.push(0x1b);
                        self.body.push(byte);
                        self.state = OscScanState::OscBody;
                    }
                }
            }

            if self.body.len() > 4096 {
                self.body.clear();
                self.state = OscScanState::Ground;
            }
        }
    }

    fn finalize(&mut self) {
        if let Some(sep) = self.body.iter().position(|&b| b == b';') {
            let cmd = &self.body[..sep];
            let payload = &self.body[sep + 1..];
            match cmd {
                b"0" | b"2" => {
                    if payload.is_empty() {
                        self.latest_title = None;
                    } else {
                        let s = sanitize_osc_str(payload, AGENT_OSC_MAX_CHARS);
                        self.latest_title = (!s.is_empty()).then_some(s);
                    }
                }
                b"9" if payload.starts_with(b"4;") => {
                    self.latest_progress = Some(sanitize_osc_str(payload, AGENT_OSC_MAX_CHARS));
                }
                b"133" => {
                    let text = String::from_utf8_lossy(payload);
                    let mut parts = text.split(';');
                    let mark = match parts.next() {
                        Some("A") => Some(PromptMark::PromptStart),
                        Some("C") => Some(PromptMark::CommandStart),
                        Some("D") => Some(PromptMark::CommandFinished(
                            parts.next().and_then(|c| c.trim().parse().ok()),
                        )),
                        _ => None,
                    };
                    if let Some(mark) = mark {
                        if self.marks.len() >= MAX_PENDING_MARKS {
                            self.marks.remove(0);
                        }
                        self.marks.push(mark);
                    }
                }
                _ => {}
            }
        }
        self.body.clear();
    }

    fn latest_title(&self) -> &str {
        self.latest_title.as_deref().unwrap_or("")
    }

    fn latest_progress(&self) -> &str {
        self.latest_progress.as_deref().unwrap_or("")
    }
}

fn sanitize_osc_str(payload: &[u8], max_chars: usize) -> String {
    let text = String::from_utf8_lossy(payload);
    text.chars()
        .filter(|ch| !ch.is_control())
        .take(max_chars)
        .collect()
}

#[derive(Default)]
enum AltScreenNormalizer {
    #[default]
    Ground,
    Escape,
    Csi(Vec<u8>),
}

impl AltScreenNormalizer {
    fn normalize(&mut self, bytes: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(bytes.len());
        for &b in bytes {
            self.advance(b, &mut out);
        }
        out
    }

    fn advance(&mut self, byte: u8, output: &mut Vec<u8>) {
        match self {
            Self::Ground if byte == 0x1b => *self = Self::Escape,
            Self::Ground => output.push(byte),
            Self::Escape if byte == b'[' => *self = Self::Csi(Vec::new()),
            Self::Escape => {
                output.push(0x1b);
                *self = Self::Ground;
                self.advance(byte, output);
            }
            Self::Csi(seq) if (0x40..=0x7e).contains(&byte) => {
                seq.push(byte);
                write_normalized_csi(seq, output);
                *self = Self::Ground;
            }
            Self::Csi(seq) if (0x20..=0x3f).contains(&byte) && seq.len() < 128 => {
                seq.push(byte);
            }
            Self::Csi(seq) => {
                output.extend_from_slice(b"\x1b[");
                output.append(seq);
                *self = Self::Ground;
                self.advance(byte, output);
            }
        }
    }
}

fn write_normalized_csi(sequence: &[u8], output: &mut Vec<u8>) {
    output.extend_from_slice(b"\x1b[");
    let Some((&final_byte, parameters)) = sequence.split_last() else {
        return;
    };
    if !matches!(final_byte, b'h' | b'l') || !parameters.starts_with(b"?") {
        output.extend_from_slice(sequence);
        return;
    }
    let private_parameters = &parameters[1..];
    if !private_parameters
        .iter()
        .all(|b| b.is_ascii_digit() || *b == b';')
    {
        output.extend_from_slice(sequence);
        return;
    }
    output.push(b'?');
    for (idx, param) in private_parameters.split(|b| *b == b';').enumerate() {
        if idx != 0 {
            output.push(b';');
        }
        match param {
            b"47" | b"1047" => output.extend_from_slice(b"1049"),
            _ => output.extend_from_slice(param),
        }
    }
    output.push(final_byte);
}
