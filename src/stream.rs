//! Byte-level tracker for the child's output stream.
//!
//! It knows whether the stream is between escape sequences / UTF-8 characters (the only places
//! where `w1` may splice its own OSC 7501 reports in), and it lifts OSC 7501 sequences emitted by
//! the child out of the stream so `w1` stays the single reporter towards the outer terminal.
//! Window titles (OSC 0/1/2) can be rewritten so animated spinner frames do not reach the outer
//! terminal as a stream of title changes; status is reported through OSC 7501 instead.

use crate::osc7501::MAX_SEQUENCE_BYTES;

const OSC_7501_HEAD: &[u8] = b"7501;";
const MAX_TITLE_BYTES: usize = 2048;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TitleFilter {
    Off,
    /// Drops braille spinner frames.
    Despin,
    /// omp: `π <status glyph> <label>`, where the glyph animates while working.
    Omp,
    /// Codex: its titles name the directory rather than the session, so none are forwarded.
    Drop,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Utf8(u8),
    Esc,
    EscIntermediate,
    Csi,
    OscHead,
    Osc7501,
    Osc7501Esc,
    Title(u8),
    TitleEsc(u8),
    Str,
    StrEsc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Terminator {
    Bel,
    St,
}

impl Terminator {
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::Bel => b"\x07",
            Self::St => b"\x1b\\",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Captured {
    Probe(Terminator),
    Report(String),
}

pub struct OutputStream {
    state: State,
    held: Vec<u8>,
    payload: Vec<u8>,
    overflow: bool,
    saw_reset: bool,
    titles: TitleFilter,
    last_title: Option<(u8, String)>,
    keep_reports: bool,
}

impl Default for OutputStream {
    fn default() -> Self {
        Self::new(TitleFilter::Off)
    }
}

impl OutputStream {
    pub fn new(titles: TitleFilter) -> Self {
        Self {
            state: State::Ground,
            held: Vec::new(),
            payload: Vec::new(),
            overflow: false,
            saw_reset: false,
            titles,
            last_title: None,
            keep_reports: false,
        }
    }

    /// Captures only the OSC 7501 probe; the child's own reports stay in the stream, in place.
    pub fn probe_only() -> Self {
        Self {
            keep_reports: true,
            ..Self::new(TitleFilter::Off)
        }
    }

    /// True when bytes may be inserted without splitting a sequence or a character.
    pub fn at_boundary(&self) -> bool {
        self.state == State::Ground && self.held.is_empty()
    }

    /// Releases an `ESC ] 7…` prefix held back because the child stalled before revealing
    /// whether it is an OSC 7501 sequence.
    pub fn flush_stalled_osc(&mut self, out: &mut Vec<u8>) {
        match self.state {
            State::OscHead => {
                out.append(&mut self.held);
                self.state = State::Str;
            }
            State::Title(_) | State::TitleEsc(_) if self.titles == TitleFilter::Drop => {}
            State::Title(_) => {
                self.release_title(out);
                self.state = State::Str;
            }
            State::TitleEsc(_) => {
                self.release_title(out);
                self.held.push(0x1b);
                self.state = State::StrEsc;
            }
            _ => {}
        }
    }

    pub fn stalled_in_osc_head(&self) -> bool {
        match self.state {
            State::OscHead => true,
            State::Title(_) | State::TitleEsc(_) => self.titles != TitleFilter::Drop,
            _ => false,
        }
    }

    /// Whether the child sent RIS (`ESC c`), which wipes the outer terminal's status table.
    pub fn take_reset(&mut self) -> bool {
        std::mem::take(&mut self.saw_reset)
    }

    pub fn process(&mut self, data: &[u8], out: &mut Vec<u8>, captured: &mut Vec<Captured>) {
        for &b in data {
            self.byte(b, out, captured);
        }
    }

    fn byte(&mut self, b: u8, out: &mut Vec<u8>, captured: &mut Vec<Captured>) {
        match self.state {
            State::Ground => match b {
                0x1b => {
                    self.held.push(b);
                    self.state = State::Esc;
                }
                0xC2..=0xDF => self.forward_to(b, out, State::Utf8(1)),
                0xE0..=0xEF => self.forward_to(b, out, State::Utf8(2)),
                0xF0..=0xF4 => self.forward_to(b, out, State::Utf8(3)),
                _ => out.push(b),
            },
            State::Utf8(n) => match b {
                0x80..=0xBF => {
                    out.push(b);
                    self.state = if n <= 1 { State::Ground } else { State::Utf8(n - 1) };
                }
                _ => {
                    self.state = State::Ground;
                    self.byte(b, out, captured);
                }
            },
            State::Esc => match b {
                b']' => {
                    self.held.push(b);
                    self.state = State::OscHead;
                }
                0x1b => {
                    out.append(&mut self.held);
                    self.held.push(b);
                }
                _ => {
                    out.append(&mut self.held);
                    out.push(b);
                    if b == b'c' {
                        self.saw_reset = true;
                    }
                    self.state = match b {
                        b'[' => State::Csi,
                        b'P' | b'X' | b'^' | b'_' => State::Str,
                        0x20..=0x2F => State::EscIntermediate,
                        _ => State::Ground,
                    };
                }
            },
            State::EscIntermediate => {
                out.push(b);
                if !(0x20..=0x2F).contains(&b) {
                    self.state = State::Ground;
                }
            }
            State::Csi => match b {
                0x1b => {
                    self.held.push(b);
                    self.state = State::Esc;
                }
                0x18 | 0x1a | 0x40..=0x7E => self.forward_to(b, out, State::Ground),
                _ => out.push(b),
            },
            State::OscHead => {
                self.held.push(b);
                let body = self.held.get(2..).unwrap_or_default();
                let filtering = self.titles != TitleFilter::Off;
                let title_kind = |c: u8| matches!(c, b'0' | b'1' | b'2');
                if body == OSC_7501_HEAD {
                    self.held.clear();
                    self.payload.clear();
                    self.overflow = false;
                    self.state = State::Osc7501;
                } else if let [kind, b';'] = body {
                    if filtering && title_kind(*kind) {
                        self.payload.clear();
                        self.state = State::Title(*kind);
                    } else {
                        let last = self.held.pop();
                        out.append(&mut self.held);
                        self.state = State::Str;
                        if let Some(last) = last {
                            self.byte(last, out, captured);
                        }
                    }
                } else {
                    let maybe_title = filtering && matches!(body, [k] if title_kind(*k));
                    if OSC_7501_HEAD.starts_with(body) || maybe_title {
                        return;
                    }
                    let last = self.held.pop();
                    out.append(&mut self.held);
                    self.state = State::Str;
                    if let Some(last) = last {
                        self.byte(last, out, captured);
                    }
                }
            }
            State::Osc7501 => match b {
                0x07 => self.finish_7501(Terminator::Bel, out, captured),
                0x1b => self.state = State::Osc7501Esc,
                0x18 | 0x1a => {
                    self.payload.clear();
                    self.state = State::Ground;
                }
                _ => {
                    if self.payload.len() < MAX_SEQUENCE_BYTES {
                        self.payload.push(b);
                    } else {
                        self.overflow = true;
                    }
                }
            },
            State::Osc7501Esc => {
                if b == b'\\' {
                    self.finish_7501(Terminator::St, out, captured);
                } else {
                    self.payload.clear();
                    self.held.push(0x1b);
                    self.state = State::Esc;
                    self.byte(b, out, captured);
                }
            }
            State::Title(kind) => match b {
                0x07 => self.finish_title(kind, Terminator::Bel, out),
                0x1b => self.state = State::TitleEsc(kind),
                0x18 | 0x1a => {
                    self.release_title(out);
                    self.forward_to(b, out, State::Ground);
                }
                _ if self.payload.len() < MAX_TITLE_BYTES => self.payload.push(b),
                _ if self.titles == TitleFilter::Drop => {}
                _ => {
                    self.release_title(out);
                    self.forward_to(b, out, State::Str);
                }
            },
            State::TitleEsc(kind) => {
                if b == b'\\' {
                    self.finish_title(kind, Terminator::St, out);
                } else {
                    self.release_title(out);
                    self.held.push(0x1b);
                    self.state = State::Esc;
                    self.byte(b, out, captured);
                }
            }
            State::Str => match b {
                0x07 | 0x18 | 0x1a => self.forward_to(b, out, State::Ground),
                0x1b => {
                    self.held.push(b);
                    self.state = State::StrEsc;
                }
                _ => out.push(b),
            },
            State::StrEsc => {
                if b == b'\\' {
                    out.append(&mut self.held);
                    self.forward_to(b, out, State::Ground);
                } else {
                    // ESC not followed by `\` aborts the string and starts a new sequence.
                    self.state = State::Esc;
                    self.byte(b, out, captured);
                }
            }
        }
    }

    fn forward_to(&mut self, b: u8, out: &mut Vec<u8>, next: State) {
        out.push(b);
        self.state = next;
    }

    /// Forwards a held title sequence unchanged.
    fn release_title(&mut self, out: &mut Vec<u8>) {
        if self.titles == TitleFilter::Drop {
            self.held.clear();
            self.payload.clear();
            return;
        }
        out.append(&mut self.held);
        out.append(&mut self.payload);
    }

    fn finish_title(&mut self, kind: u8, term: Terminator, out: &mut Vec<u8>) {
        self.state = State::Ground;
        if self.titles == TitleFilter::Drop {
            self.release_title(out);
            return;
        }
        let Some(title) = std::str::from_utf8(&self.payload)
            .ok()
            .map(|t| normalize_title(self.titles, t))
        else {
            self.release_title(out);
            out.extend_from_slice(term.bytes());
            return;
        };
        self.held.clear();
        self.payload.clear();
        if self.last_title.as_ref().is_some_and(|(k, t)| *k == kind && *t == title) {
            return;
        }
        out.extend_from_slice(b"\x1b]");
        out.push(kind);
        out.push(b';');
        out.extend_from_slice(title.as_bytes());
        out.extend_from_slice(term.bytes());
        self.last_title = Some((kind, title));
    }

    fn finish_7501(&mut self, term: Terminator, out: &mut Vec<u8>, captured: &mut Vec<Captured>) {
        self.state = State::Ground;
        let payload = std::mem::take(&mut self.payload);
        if self.overflow {
            self.overflow = false;
            return;
        }
        if payload.first() == Some(&b'?') {
            captured.push(Captured::Probe(term));
        } else if self.keep_reports {
            out.extend_from_slice(b"\x1b]");
            out.extend_from_slice(OSC_7501_HEAD);
            out.extend_from_slice(&payload);
            out.extend_from_slice(term.bytes());
        } else if let Ok(text) = String::from_utf8(payload) {
            captured.push(Captured::Report(text));
        }
    }
}

fn is_spinner_token(token: &str) -> bool {
    !token.is_empty() && token.chars().all(|c| matches!(c as u32, 0x2800..=0x28FF))
}

fn normalize_title(filter: TitleFilter, title: &str) -> String {
    let mut tokens: Vec<&str> = title.split(' ').collect();
    if filter == TitleFilter::Omp && tokens.first() == Some(&"π") && tokens.len() >= 2 {
        let glyph = tokens.get(1).copied().unwrap_or_default();
        if glyph.chars().count() == 1 {
            tokens.remove(1);
        }
    }
    let kept: Vec<&str> = tokens.into_iter().filter(|t| !is_spinner_token(t)).collect();
    let joined = kept.join(" ");
    if joined.len() == title.len() {
        joined
    } else {
        joined.split_whitespace().collect::<Vec<_>>().join(" ")
    }
}
