//! When the listener listens, and why it does not.
//!
//! It runs only while everything it needs is there (`Need`): the switch,
//! the island on, the main synth linked, the microphone allowed, the
//! models. Then it pauses by itself (`Pause`) while the app is paused,
//! while a hold runs, while the screen is locked, and while another app
//! uses the microphone (a call). A pause stops the microphone; the island
//! and Settings say why.

use serde::Serialize;

/// Why it does not listen at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Need {
    /// The island is off, or no synth is linked to it.
    Island,
    /// The main synth is offline or too old for the island.
    Synth,
    /// The microphone was not allowed (yet).
    Mic,
    /// The models are not downloaded (or are downloading).
    Model,
    /// The models are loading, or did not load.
    Loading,
}

/// Why it stopped for now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Pause {
    /// «Остановить все».
    AppPaused,
    /// A hold key is down.
    Hold,
    /// The screen is locked (or the Mac sleeps).
    Locked,
    /// Another app records (a call).
    MicBusy,
}

/// What the island and Settings show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Status {
    Off,
    Waiting { need: Need },
    Listening,
    Paused { reason: Pause },
    /// The name was heard: the request is being recorded.
    Woke,
}

/// What the listener looks at, once a tick.
#[derive(Debug, Clone, Copy, Default)]
pub struct Inputs {
    pub on: bool,
    pub available: bool,
    pub island: bool,
    pub synth_up: bool,
    pub mic_granted: bool,
    pub model_ready: bool,
    pub ears_loaded: bool,
    pub app_paused: bool,
    /// A hold runs that is not a wake of ours.
    pub holding: bool,
    pub locked: bool,
    pub mic_busy: bool,
    /// A wake of ours is recording.
    pub woke: bool,
}

/// The status, in this order: what is missing, then why it pauses.
pub fn status(i: &Inputs) -> Status {
    if !i.on || !i.available {
        return Status::Off;
    }
    let need = if !i.island {
        Some(Need::Island)
    } else if !i.synth_up {
        Some(Need::Synth)
    } else if !i.mic_granted {
        Some(Need::Mic)
    } else if !i.model_ready {
        Some(Need::Model)
    } else if !i.ears_loaded {
        Some(Need::Loading)
    } else {
        None
    };
    if let Some(need) = need {
        return Status::Waiting { need };
    }
    if i.woke {
        return Status::Woke;
    }
    let pause = if i.app_paused {
        Some(Pause::AppPaused)
    } else if i.locked {
        Some(Pause::Locked)
    } else if i.mic_busy {
        Some(Pause::MicBusy)
    } else if i.holding {
        Some(Pause::Hold)
    } else {
        None
    };
    match pause {
        Some(reason) => Status::Paused { reason },
        None => Status::Listening,
    }
}

impl Status {
    /// The microphone is open for the listener.
    pub fn listens(self) -> bool {
        matches!(self, Status::Listening | Status::Woke)
    }

    /// The models should be in memory.
    pub fn wants_ears(i: &Inputs) -> bool {
        i.on && i.available && i.model_ready
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> Inputs {
        Inputs { on: true, available: true, island: true, synth_up: true, mic_granted: true, model_ready: true, ears_loaded: true, ..Inputs::default() }
    }

    #[test]
    fn it_listens_only_with_everything_there() {
        assert_eq!(status(&ready()), Status::Listening);
        assert_eq!(status(&Inputs { on: false, ..ready() }), Status::Off);
        assert_eq!(status(&Inputs { available: false, ..ready() }), Status::Off, "an Intel Mac, Windows");
        assert_eq!(status(&Inputs { island: false, ..ready() }), Status::Waiting { need: Need::Island });
        assert_eq!(status(&Inputs { synth_up: false, ..ready() }), Status::Waiting { need: Need::Synth });
        assert_eq!(status(&Inputs { mic_granted: false, ..ready() }), Status::Waiting { need: Need::Mic });
        assert_eq!(status(&Inputs { model_ready: false, ..ready() }), Status::Waiting { need: Need::Model });
        assert_eq!(status(&Inputs { ears_loaded: false, ..ready() }), Status::Waiting { need: Need::Loading });
        assert!(Status::wants_ears(&Inputs { ears_loaded: false, ..ready() }));
        assert!(!Status::wants_ears(&Inputs { on: false, ..ready() }), "off unloads whisper tiny");
    }

    #[test]
    fn it_pauses_by_itself_and_comes_back() {
        let paused = |i: Inputs| match status(&i) {
            Status::Paused { reason } => Some(reason),
            _ => None,
        };
        assert_eq!(paused(Inputs { app_paused: true, ..ready() }), Some(Pause::AppPaused));
        assert_eq!(paused(Inputs { holding: true, ..ready() }), Some(Pause::Hold));
        assert_eq!(paused(Inputs { locked: true, ..ready() }), Some(Pause::Locked));
        assert_eq!(paused(Inputs { mic_busy: true, ..ready() }), Some(Pause::MicBusy));
        // Several at once: the one that explains most.
        assert_eq!(paused(Inputs { mic_busy: true, holding: true, ..ready() }), Some(Pause::MicBusy));
        assert_eq!(paused(Inputs { app_paused: true, locked: true, ..ready() }), Some(Pause::AppPaused));
        assert!(!status(&Inputs { mic_busy: true, ..ready() }).listens(), "the microphone closes");
        // The call ended.
        assert_eq!(status(&ready()), Status::Listening);
        // Its own wake is not a pause.
        assert_eq!(status(&Inputs { woke: true, ..ready() }), Status::Woke);
        assert!(Status::Woke.listens());
    }
}
