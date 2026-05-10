use crate::{
    audio::{
        decode::decode_thread,
        output::{create_audio_buffer, play_audio},
    },
    command::Command,
};

use std::thread;

use anyhow::Result;
use cpal::traits::StreamTrait;
use crossbeam_channel::Receiver;

pub fn run_control_loop(cmd_rx: Receiver<Command>) -> Result<()> {
    let mut current_decode_thread: Option<thread::JoinHandle<()>> = None;

    let mut current_stream = None;

    loop {
        let command = cmd_rx.recv()?;

        match command {
            Command::Play(path) => {
                current_stream = None;

                if let Some(handle) = current_decode_thread.take() {
                    let _ = handle.join();
                }

                let (producer, consumer) = create_audio_buffer();

                let path_str = path.to_string_lossy().to_string();

                current_decode_thread = Some(thread::spawn(move || {
                    if let Err(err) = decode_thread(&path_str, producer) {
                        eprintln!("Decode error: {err:#}");
                    }
                }));

                match play_audio(consumer) {
                    Ok(stream) => {
                        current_stream = Some(stream);
                    }

                    Err(err) => {
                        eprintln!("Playback error: {err:#}");
                    }
                }
            }

            Command::Pause => {
                if let Some(stream) = &current_stream {
                    let _ = stream.pause();
                }
            }

            Command::Resume => {
                if let Some(stream) = &current_stream {
                    let _ = stream.play();
                }
            }

            Command::Stop => {
                current_stream = None;

                if let Some(handle) = current_decode_thread.take() {
                    let _ = handle.join();
                }
            }

            Command::SetVolume(_) => {}
        }
    }
}
