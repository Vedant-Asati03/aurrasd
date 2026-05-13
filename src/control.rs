use crate::{
    audio::{
        decode::decode_thread,
        output::{create_audio_buffer, play_audio},
    },
    command::Command,
    event::Event,
    session::PlaybackSession,
    state::{PlaybackState, PlaybackStatus},
};

use std::thread;

use anyhow::Result;
use cpal::traits::StreamTrait;
use crossbeam_channel::{Receiver, Select, Sender, bounded};

pub fn run_control_loop(cmd_rx: Receiver<Command>, event_tx: Sender<Event>) -> Result<()> {
    let mut current_session: Option<PlaybackSession> = None;
    let mut playback_state = PlaybackState::new();

    macro_rules! play_track_macro {
        ($path:expr) => {
            if let Some(session) = current_session.take() {
                PlaybackSession::stop(session);
            }

            let (producer, consumer) = create_audio_buffer();
            let (shutdown_tx, shutdown_rx) = bounded::<()>(1);
            let (finished_tx, finished_rx) = bounded::<()>(1);

            let path_clone = $path.clone();

            let decode_thread = thread::spawn(move || {
                if let Err(err) = decode_thread(&path_clone, producer, shutdown_rx, finished_tx) {
                    tracing::error!("Decode error: {err:#}");
                }
            });

            match play_audio(consumer) {
                Ok(stream) => {
                    playback_state.status = PlaybackStatus::Playing;
                    let _ = event_tx.send(Event::PlaybackStarted($path));
                    let _ = event_tx.send(Event::StateChanged(playback_state.clone()));
                    current_session = Some(PlaybackSession::new(
                        stream,
                        decode_thread,
                        shutdown_tx,
                        finished_rx,
                    ));
                }
                Err(err) => {
                    tracing::error!("Playback error: {err:#}");
                    let _ = event_tx.send(Event::Error(format!("Playback error: {}", err)));
                }
            }
        };
    }

    loop {
        let mut sel = Select::new();
        let cmd_idx = sel.recv(&cmd_rx);
        let finished_idx = current_session.as_ref().map(|s| sel.recv(&s.finished_rx));

        let oper = sel.select();

        if oper.index() == cmd_idx {
            let command = match oper.recv(&cmd_rx) {
                Ok(cmd) => cmd,
                Err(_) => {
                    if let Some(session) = current_session.take() {
                        PlaybackSession::stop(session);
                    }
                    return Ok(());
                }
            };

            match command {
                Command::Play(path) => {
                    play_track_macro!(path);
                }

                Command::Stop => {
                    if let Some(session) = current_session.take() {
                        PlaybackSession::stop(session);
                        playback_state.status = PlaybackStatus::Stopped;
                        let _ = event_tx.send(Event::PlaybackStopped);
                        let _ = event_tx.send(Event::StateChanged(playback_state.clone()));
                    }
                }

                Command::Pause => {
                    if let Some(session) = &current_session {
                        if let Err(err) = session.stream.pause() {
                            tracing::error!("Failed to pause stream: {err:#}");
                            let _ = event_tx
                                .send(Event::Error(format!("Failed to pause stream: {}", err)));
                        } else {
                            playback_state.status = PlaybackStatus::Paused;
                            let _ = event_tx.send(Event::PlaybackPaused);
                            let _ = event_tx.send(Event::StateChanged(playback_state.clone()));
                        }
                    }
                }

                Command::Resume => {
                    if let Some(session) = &current_session {
                        if let Err(err) = session.stream.play() {
                            tracing::error!("Failed to resume stream: {err:#}");
                            let _ = event_tx
                                .send(Event::Error(format!("Failed to resume stream: {}", err)));
                        } else {
                            playback_state.status = PlaybackStatus::Playing;
                            let _ = event_tx.send(Event::PlaybackResumed);
                            let _ = event_tx.send(Event::StateChanged(playback_state.clone()));
                        }
                    }
                }

                Command::SetVolume(_) => {}

                Command::Enqueue(path) => {
                    playback_state.queue.push_back(path);
                    let _ = event_tx.send(Event::QueueUpdated);
                    let _ = event_tx.send(Event::StateChanged(playback_state.clone()));
                }

                Command::Next => {
                    if let Some(path) = playback_state.queue.pop_front() {
                        let _ = event_tx.send(Event::QueueUpdated);
                        play_track_macro!(path);
                    } else {
                        if let Some(session) = current_session.take() {
                            PlaybackSession::stop(session);
                            playback_state.status = PlaybackStatus::Stopped;
                            let _ = event_tx.send(Event::PlaybackStopped);
                            let _ = event_tx.send(Event::StateChanged(playback_state.clone()));
                        }
                    }
                }

                Command::Previous => {
                    // Previous not fully implemented yet but skeleton added
                }

                Command::ClearQueue => {
                    playback_state.queue.clear();
                    let _ = event_tx.send(Event::QueueUpdated);
                    let _ = event_tx.send(Event::StateChanged(playback_state.clone()));
                }
            }
        } else if let Some(idx) = finished_idx {
            if oper.index() == idx {
                let _ = oper.recv(&current_session.as_ref().unwrap().finished_rx);

                if let Some(session) = current_session.take() {
                    playback_state.status = PlaybackStatus::Stopped;
                    PlaybackSession::stop(session);
                }
                let _ = event_tx.send(Event::PlaybackFinished);

                if let Some(path) = playback_state.queue.pop_front() {
                    let _ = event_tx.send(Event::QueueUpdated);
                    play_track_macro!(path);
                } else {
                    let _ = event_tx.send(Event::StateChanged(playback_state.clone()));
                }
            }
        }
    }
}
