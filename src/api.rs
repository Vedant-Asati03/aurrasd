use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Debug, Serialize, Deserialize)]
pub enum Command {
    Play(String),
    Stop,
    Pause,
    Resume,
    SetVolume(f32),
    Enqueue(String),
    Next,
    Previous,
    ClearQueue,
    GetState,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum Event {
    PlaybackStarted(String),
    PlaybackStopped,
    PlaybackPaused,
    PlaybackResumed,
    PlaybackFinished,
    Error(String),
    QueueUpdated,
    StateChanged(PlaybackStatus),
    FullState(PlaybackState),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PlaybackStatus {
    Playing,
    Paused,
    #[default]
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PlaybackState {
    pub status: PlaybackStatus,
    pub queue: VecDeque<String>,
}

impl PlaybackState {
    pub fn new() -> Self {
        Self {
            status: PlaybackStatus::Stopped,
            queue: VecDeque::new(),
        }
    }
}
