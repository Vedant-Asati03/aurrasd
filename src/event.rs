use crate::state;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Event {
    PlaybackStarted(String),
    PlaybackStopped,
    PlaybackPaused,
    PlaybackResumed,
    PlaybackFinished,
    Error(String),
    QueueUpdated,
    StateChanged(state::PlaybackState),
}
