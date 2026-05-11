use crate::{
    audio::{
        decode::decode_thread,
        output::{create_audio_buffer, play_audio},
    },
    command::Command,
    session::PlaybackSession,
};

use std::thread;

use anyhow::Result;
use cpal::traits::StreamTrait;
use crossbeam_channel::{Receiver, bounded};

pub fn run_control_loop(cmd_rx: Receiver<Command>) -> Result<()> {
    let mut current_session: Option<PlaybackSession> = None;

    loop {
        let command = match cmd_rx.recv() {
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
                if let Some(session) = current_session.take() {
                    PlaybackSession::stop(session);
                }

                let (producer, consumer) = create_audio_buffer();
                let (shutdown_tx, shutdown_rx) = bounded::<()>(1);
                let (finished_tx, finished_rx) = bounded::<()>(1);

                let path_str = path.to_string_lossy().to_string();

                let decode_thread = thread::spawn(move || {
                    if let Err(err) = decode_thread(&path_str, producer, shutdown_rx, finished_tx) {
                        tracing::error!("Decode error: {err:#}");
                    }
                });

                let stream = match play_audio(consumer) {
                    Ok(stream) => stream,
                    Err(err) => {
                        tracing::error!("Playback error: {err:#}");
                        continue;
                    }
                };

                current_session = Some(PlaybackSession::new(
                    stream,
                    decode_thread,
                    shutdown_tx,
                    finished_rx,
                ));
            }

            Command::Stop => {
                if let Some(session) = current_session.take() {
                    PlaybackSession::stop(session);
                }
            }

            Command::Pause => {
                if let Some(session) = &current_session
                    && let Err(err) = session.stream.pause()
                {
                    tracing::error!("Failed to pause stream: {err:#}");
                }
            }

            Command::Resume => {
                if let Some(session) = &current_session
                    && let Err(err) = session.stream.play()
                {
                    tracing::error!("Failed to resume stream: {err:#}");
                }
            }

            Command::SetVolume(_) => {}
        }
    }
}
