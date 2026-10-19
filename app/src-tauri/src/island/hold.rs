//! The hold keys as a small state machine, apart from the OS and the
//! microphone so it can be tested.
//!
//! - Voice and screenshot + voice: press starts a hold, release ends it.
//!   A hold shorter than `SHORT_PRESS` is a slip and sends nothing. A hold
//!   that reaches `MAX_HOLD` ends there (the protocol takes two minutes of
//!   voice at most); the release that follows is ignored.
//! - While one hold runs, every other press is ignored (the OS repeats a
//!   held key, and a second key mid-sentence is a slip too).
//! - Text: a press opens the line of text; its release means nothing.

use std::time::{Duration, Instant};

use connect_core::island::HotkeyKind;

/// Shorter than this is an accidental press: nothing is sent.
pub const SHORT_PRESS: Duration = Duration::from_millis(300);
/// The longest voice the protocol takes.
pub const MAX_HOLD: Duration = Duration::from_millis(connect_core::island::MAX_VOICE_MS);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Nothing,
    /// Start recording (and, for screen_voice, take the screenshot).
    Start(HotkeyKind),
    /// The hold was too short: stop recording, send nothing.
    Discard,
    /// The hold ended: stop recording and send what was recorded.
    Send(HotkeyKind, Duration),
    /// Open the line of text.
    OpenText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum State {
    #[default]
    Idle,
    Holding { kind: HotkeyKind, since: Instant },
    /// The hold reached its limit and was sent: waiting for the key to
    /// come up, so the release does not count as anything.
    Spent { kind: HotkeyKind },
}

#[derive(Debug, Default)]
pub struct Hold {
    state: State,
}

impl Hold {
    pub fn down(&mut self, kind: HotkeyKind, now: Instant) -> Action {
        match (self.state, kind) {
            (State::Idle, HotkeyKind::Text) => Action::OpenText,
            (State::Idle, _) => {
                self.state = State::Holding { kind, since: now };
                Action::Start(kind)
            }
            // A repeat of the held key, or another key during a hold.
            _ => Action::Nothing,
        }
    }

    pub fn up(&mut self, kind: HotkeyKind, now: Instant) -> Action {
        match self.state {
            State::Holding { kind: held, since } if held == kind => {
                self.state = State::Idle;
                let held_for = now.saturating_duration_since(since);
                if held_for < SHORT_PRESS {
                    Action::Discard
                } else {
                    Action::Send(kind, held_for.min(MAX_HOLD))
                }
            }
            State::Spent { kind: held } if held == kind => {
                self.state = State::Idle;
                Action::Nothing
            }
            _ => Action::Nothing,
        }
    }

    /// Called on a timer while a hold runs: ends it at `MAX_HOLD`.
    pub fn tick(&mut self, now: Instant) -> Action {
        match self.state {
            State::Holding { kind, since } if now.saturating_duration_since(since) >= MAX_HOLD => {
                self.state = State::Spent { kind };
                Action::Send(kind, MAX_HOLD)
            }
            _ => Action::Nothing,
        }
    }

    /// Stop whatever runs (the island was switched off, the app paused, a
    /// recording failed): nothing is sent.
    pub fn cancel(&mut self) -> Action {
        let was = std::mem::take(&mut self.state);
        match was {
            State::Holding { .. } => Action::Discard,
            _ => Action::Nothing,
        }
    }

    pub fn holding(&self) -> Option<HotkeyKind> {
        match self.state {
            State::Holding { kind, .. } => Some(kind),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V: HotkeyKind = HotkeyKind::Voice;
    const S: HotkeyKind = HotkeyKind::ScreenVoice;
    const T: HotkeyKind = HotkeyKind::Text;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_hold_sends_and_a_tap_does_not() {
        let t0 = Instant::now();
        let mut h = Hold::default();
        assert_eq!(h.down(V, t0), Action::Start(V));
        assert_eq!(h.up(V, t0 + ms(299)), Action::Discard, "299 ms is a slip");
        assert_eq!(h.down(V, t0 + ms(1000)), Action::Start(V));
        assert_eq!(h.up(V, t0 + ms(1300)), Action::Send(V, ms(300)), "300 ms counts");
        assert_eq!(h.holding(), None);
    }

    #[test]
    fn repeats_and_other_keys_during_a_hold_are_ignored() {
        let t0 = Instant::now();
        let mut h = Hold::default();
        assert_eq!(h.down(S, t0), Action::Start(S));
        assert_eq!(h.down(S, t0 + ms(30)), Action::Nothing, "the OS repeats a held key");
        assert_eq!(h.down(V, t0 + ms(40)), Action::Nothing);
        assert_eq!(h.down(T, t0 + ms(50)), Action::Nothing);
        assert_eq!(h.up(V, t0 + ms(60)), Action::Nothing, "not the key that is held");
        assert_eq!(h.holding(), Some(S));
        assert_eq!(h.up(S, t0 + ms(2000)), Action::Send(S, ms(2000)));
    }

    #[test]
    fn a_hold_ends_at_two_minutes_and_its_late_release_means_nothing() {
        let t0 = Instant::now();
        let mut h = Hold::default();
        h.down(V, t0);
        assert_eq!(h.tick(t0 + ms(119_000)), Action::Nothing);
        assert_eq!(h.tick(t0 + MAX_HOLD), Action::Send(V, MAX_HOLD));
        assert_eq!(h.tick(t0 + MAX_HOLD + ms(10)), Action::Nothing);
        assert_eq!(h.down(V, t0 + MAX_HOLD + ms(20)), Action::Nothing, "still held");
        assert_eq!(h.up(V, t0 + MAX_HOLD + ms(5000)), Action::Nothing);
        assert_eq!(h.down(V, t0 + MAX_HOLD + ms(6000)), Action::Start(V), "a new hold after that");
    }

    #[test]
    fn text_opens_on_press_only() {
        let t0 = Instant::now();
        let mut h = Hold::default();
        assert_eq!(h.down(T, t0), Action::OpenText);
        assert_eq!(h.up(T, t0 + ms(10)), Action::Nothing);
        assert_eq!(h.holding(), None);
    }

    #[test]
    fn a_cancel_drops_the_hold() {
        let t0 = Instant::now();
        let mut h = Hold::default();
        h.down(V, t0);
        assert_eq!(h.cancel(), Action::Discard);
        assert_eq!(h.up(V, t0 + ms(5000)), Action::Nothing);
        assert_eq!(h.cancel(), Action::Nothing);
    }
}
