//! The `org.callaudio.CallAudio1` interface and the profile switching.

use zbus::fdo;

use crate::native::{self, Card};

/// The UCM verb a card plays calls with.
pub const VERB: &str = "Voice Call";

pub struct CallAudio {
    card: Card,
    /// The profile to go back to; set while call audio runs.
    previous: Option<String>,
}

impl CallAudio {
    pub fn new(card: Card) -> Self {
        CallAudio {
            card,
            previous: None,
        }
    }

    pub fn active(&self) -> bool {
        self.previous.is_some()
    }

    /// Starts or stops call audio; true when it changed.
    pub fn follow(&mut self, call: bool) -> bool {
        match (call, self.previous.take()) {
            (true, None) => match native::set_profile(self.card.index, VERB) {
                Ok(previous) => {
                    eprintln!(
                        "call audio on, card {} was on \"{previous}\"",
                        self.card.index
                    );
                    self.previous = Some(previous);
                    true
                }
                Err(e) => {
                    eprintln!("cannot switch card {} to \"{VERB}\": {e}", self.card.index);
                    false
                }
            },
            (false, Some(previous)) => {
                // The next recording starts with the microphone on.
                if !self.card.mute.is_empty()
                    && let Err(e) = native::set(self.card.index, &self.card.mute, 1)
                {
                    eprintln!("cannot unmute: {e}");
                }
                if previous == VERB {
                    // Left over from an earlier run: WirePlumber's choice stays.
                    eprintln!("call audio off, card was already on \"{VERB}\"");
                } else if let Err(e) = native::set_profile(self.card.index, &previous) {
                    eprintln!("cannot switch card back to \"{previous}\": {e}");
                } else {
                    eprintln!("call audio off, card back on \"{previous}\"");
                }
                true
            }
            (_, previous) => {
                self.previous = previous;
                false
            }
        }
    }

    fn control(&self, id: &str) -> fdo::Result<native::Control> {
        if id.is_empty() {
            return Err(fdo::Error::NotSupported(format!(
                "the \"{VERB}\" verb names no such control"
            )));
        }
        native::get(self.card.index, id).map_err(|e| fdo::Error::IOError(e.to_string()))
    }
}

#[zbus::interface(name = "org.callaudio.CallAudio1")]
impl CallAudio {
    /// The card is on its voice call profile.
    #[zbus(property, name = "Active")]
    fn active_property(&self) -> bool {
        self.active()
    }

    /// Call volume, 0 to VolumeMax; kept between calls.
    #[zbus(property)]
    fn volume(&self) -> fdo::Result<u32> {
        let control = self.control(&self.card.volume)?;
        Ok((control.value - control.min) as u32)
    }

    #[zbus(property)]
    fn set_volume(&mut self, level: u32) -> fdo::Result<()> {
        let control = self.control(&self.card.volume)?;
        let value = control.min + i64::from(level);
        if value > control.max {
            return Err(fdo::Error::InvalidArgs(format!(
                "volume {level} is above {}",
                control.max - control.min
            )));
        }
        native::set(self.card.index, &self.card.volume, value)
            .map_err(|e| fdo::Error::IOError(e.to_string()))
    }

    #[zbus(property)]
    fn volume_max(&self) -> fdo::Result<u32> {
        let control = self.control(&self.card.volume)?;
        Ok((control.max - control.min) as u32)
    }

    /// The microphone is off for the other side. Each call starts unmuted.
    #[zbus(property)]
    fn muted(&self) -> fdo::Result<bool> {
        if !self.active() {
            return Ok(false);
        }
        Ok(self.control(&self.card.mute)?.value == 0)
    }

    #[zbus(property)]
    fn set_muted(&mut self, muted: bool) -> fdo::Result<()> {
        if !self.active() {
            return Err(fdo::Error::Failed("no call".into()));
        }
        self.control(&self.card.mute)?;
        native::set(self.card.index, &self.card.mute, i64::from(!muted))
            .map_err(|e| fdo::Error::IOError(e.to_string()))
    }
}
