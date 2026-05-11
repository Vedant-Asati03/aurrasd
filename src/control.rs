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
        let command = cmd_rx.recv()?;

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
                        eprintln!("Decode error: {err:#}");
                    }
                });

                let stream = match play_audio(consumer) {
                    Ok(stream) => stream,
                    Err(err) => {
                        eprintln!("Playback error: {err:#}");
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
                if let Some(session) = &current_session {
                    let _ = session.stream.pause();
                }
            }

            Command::Resume => {
                if let Some(session) = &current_session {
                    let _ = session.stream.play();
                }
            }

            Command::SetVolume(_) => {}
        }
    }
}
