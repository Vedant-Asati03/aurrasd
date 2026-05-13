use crate::{
    api::{Command, Event, PlaybackState, PlaybackStatus},
    audio::session::PlaybackSession,
};

use anyhow::Result;
use cpal::traits::StreamTrait;
use crossbeam_channel::{Receiver, Select, Sender};

fn play_track(
    path: String,
    current_session: &mut Option<PlaybackSession>,
    playback_state: &mut PlaybackState,
    event_tx: &Sender<Event>,
) {
    if let Some(session) = current_session.take() {
        PlaybackSession::stop(session);
    }

    match PlaybackSession::start(&path) {
        Ok(session) => {
            playback_state.status = PlaybackStatus::Playing;
            let _ = event_tx.send(Event::PlaybackStarted(path));
            let _ = event_tx.send(Event::StateChanged(playback_state.status));
            *current_session = Some(session);
        }
        Err(err) => {
            tracing::error!("Playback error: {err:#}");
            let _ = event_tx.send(Event::Error(format!("Playback error: {}", err)));
        }
    }
}

enum Selected {
    Command(Option<Command>),
    Drained,
    StreamError,
}

pub fn run_control_loop(cmd_rx: Receiver<Command>, event_tx: Sender<Event>) -> Result<()> {
    let mut current_session: Option<PlaybackSession> = None;
    let mut playback_state = PlaybackState::new();

    loop {
        let selected: Selected = {
            let mut sel = Select::new();
            let cmd_idx = sel.recv(&cmd_rx);

            let drained_idx = current_session.as_ref().map(|s| sel.recv(&s.drained_rx));

            let stream_error_idx = current_session
                .as_ref()
                .map(|s| sel.recv(&s.stream_error_rx));

            let oper = sel.select();
            let idx = oper.index();

            if idx == cmd_idx {
                let cmd = oper.recv(&cmd_rx).ok();
                Selected::Command(cmd)
            } else if drained_idx.map_or(false, |i| idx == i) {
                if let Some(s) = current_session.as_ref() {
                    let _ = oper.recv(&s.drained_rx);
                }
                Selected::Drained
            } else if stream_error_idx.map_or(false, |i| idx == i) {
                if let Some(s) = current_session.as_ref() {
                    let _ = oper.recv(&s.stream_error_rx);
                }
                Selected::StreamError
            } else {
                let _ = oper.recv(&cmd_rx);
                continue;
            }
        };

        match selected {
            Selected::Command(maybe_cmd) => {
                let command = match maybe_cmd {
                    Some(cmd) => cmd,
                    None => {
                        // cmd_rx closed — shut down gracefully.
                        if let Some(session) = current_session.take() {
                            PlaybackSession::stop(session);
                        }
                        return Ok(());
                    }
                };

                match command {
                    Command::Play(path) => {
                        play_track(path, &mut current_session, &mut playback_state, &event_tx);
                    }

                    Command::Stop => {
                        if let Some(session) = current_session.take() {
                            PlaybackSession::stop(session);
                            playback_state.status = PlaybackStatus::Stopped;
                            let _ = event_tx.send(Event::PlaybackStopped);
                            let _ = event_tx.send(Event::StateChanged(playback_state.status));
                        }
                    }

                    Command::Pause => {
                        if let Some(session) = &current_session {
                            if playback_state.status != PlaybackStatus::Playing {
                                let _ = event_tx.send(Event::Error(
                                    "Cannot pause: not currently playing".into(),
                                ));
                            } else if let Err(err) = session.stream.pause() {
                                tracing::error!("Failed to pause stream: {err:#}");
                                let _ = event_tx
                                    .send(Event::Error(format!("Failed to pause stream: {}", err)));
                            } else {
                                playback_state.status = PlaybackStatus::Paused;
                                let _ = event_tx.send(Event::PlaybackPaused);
                                let _ = event_tx.send(Event::StateChanged(playback_state.status));
                            }
                        } else {
                            let _ = event_tx
                                .send(Event::Error("Cannot pause: no active session".into()));
                        }
                    }

                    Command::Resume => {
                        if let Some(session) = &current_session {
                            if playback_state.status != PlaybackStatus::Paused {
                                let _ = event_tx.send(Event::Error(
                                    "Cannot resume: not currently paused".into(),
                                ));
                            } else if let Err(err) = session.stream.play() {
                                tracing::error!("Failed to resume stream: {err:#}");
                                let _ = event_tx.send(Event::Error(format!(
                                    "Failed to resume stream: {}",
                                    err
                                )));
                            } else {
                                playback_state.status = PlaybackStatus::Playing;
                                let _ = event_tx.send(Event::PlaybackResumed);
                                let _ = event_tx.send(Event::StateChanged(playback_state.status));
                            }
                        } else {
                            let _ = event_tx
                                .send(Event::Error("Cannot resume: no active session".into()));
                        }
                    }

                    Command::SetVolume(_) => {
                        let _ =
                            event_tx.send(Event::Error("SetVolume is not yet implemented".into()));
                    }

                    Command::Enqueue(path) => {
                        playback_state.queue.push_back(path);
                        let _ = event_tx.send(Event::QueueUpdated);
                    }

                    Command::Next => {
                        if let Some(path) = playback_state.queue.pop_front() {
                            let _ = event_tx.send(Event::QueueUpdated);
                            play_track(path, &mut current_session, &mut playback_state, &event_tx);
                        } else if let Some(session) = current_session.take() {
                            PlaybackSession::stop(session);
                            playback_state.status = PlaybackStatus::Stopped;
                            let _ = event_tx.send(Event::PlaybackStopped);
                            let _ = event_tx.send(Event::StateChanged(playback_state.status));
                        }
                    }

                    Command::Previous => {
                        let _ =
                            event_tx.send(Event::Error("Previous is not yet implemented".into()));
                    }

                    Command::ClearQueue => {
                        playback_state.queue.clear();
                        let _ = event_tx.send(Event::QueueUpdated);
                    }
                }
            }

            Selected::Drained => {
                if let Some(session) = current_session.take() {
                    playback_state.status = PlaybackStatus::Stopped;
                    PlaybackSession::stop(session);
                }
                let _ = event_tx.send(Event::PlaybackFinished);

                if let Some(path) = playback_state.queue.pop_front() {
                    let _ = event_tx.send(Event::QueueUpdated);
                    play_track(path, &mut current_session, &mut playback_state, &event_tx);
                } else {
                    let _ = event_tx.send(Event::StateChanged(playback_state.status));
                }
            }

            Selected::StreamError => {
                let msg = current_session
                    .as_ref()
                    .and_then(|s| s.stream_error_rx.try_recv().ok())
                    .unwrap_or_else(|| "unknown stream error".into());

                tracing::error!("Stream error received by control loop: {}", msg);

                if let Some(session) = current_session.take() {
                    PlaybackSession::stop(session);
                }
                playback_state.status = PlaybackStatus::Stopped;
                let _ = event_tx.send(Event::Error(msg));
                let _ = event_tx.send(Event::StateChanged(playback_state.status));
            }
        }
    }
}
