use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

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
