use super::rules::{DetectedState, ScreenDetection};
use crate::osc7501::OscState;

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
                if detection.turn_only && !self.armed && !self.turn_active {
                    return self.current;
                }
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
