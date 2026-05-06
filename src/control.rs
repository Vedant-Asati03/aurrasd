use std::thread;

use anyhow::Result;
use cpal::traits::StreamTrait;
use crossbeam_channel::Receiver;

use crate::{
    audio::{decode::decode_thread, types::AudioFormat},
    command::Command,
};

fn is_url_path(path_str: &str) -> bool {
    path_str.starts_with("http://") || path_str.starts_with("https://")
}

pub fn run_control_loop(cmd_rx: Receiver<Command>) -> Result<()> {
    let mut current_decode_thread: Option<thread::JoinHandle<()>> = None;
    let mut current_stream: Option<cpal::Stream> = None;

    loop {
        let command = match cmd_rx.recv() {
            Ok(cmd) => cmd,
            Err(err) => {
                if let Some(handle) = current_decode_thread.take() {
                    let _ = handle.join();
                }
                return Err(err.into());
            }
        };

        match command {
            Command::Play(path) => {
                current_stream = None;
                if let Some(handle) = current_decode_thread.take() {
                    let _ = handle.join();
                }

                let (data_tx, data_rx) = crossbeam_channel::bounded::<f32>(240000);
                let (fmt_tx, fmt_rx) = crossbeam_channel::bounded::<AudioFormat>(1);

                let path_str = path.to_string_lossy().to_string();
                let is_url = is_url_path(&path_str);

                current_decode_thread = Some(thread::spawn(move || {
                    if let Err(err) = decode_thread(&path_str, is_url, data_tx, fmt_tx) {
                        eprintln!("Decode thread error: {err:#}");
                    }
                }));

                if let Ok(format) = fmt_rx.recv() {
                    match crate::audio::output::play_audio(data_rx, &format) {
                        Ok(stream) => current_stream = Some(stream),
                        Err(e) => eprintln!("Failed to play audio: {}", e),
                    }
                }
            }

            Command::Stop => {
                current_stream = None;
                if let Some(handle) = current_decode_thread.take() {
                    let _ = handle.join();
                }
            }

            Command::Pause => {
                if let Some(stream) = &current_stream {
                    let _ = stream.pause();
                    println!("Song paused");
                }
            }

            Command::Resume => {
                if let Some(stream) = &current_stream {
                    let _ = stream.play();
                    println!("Song resumed");
                }
            }

            Command::SetVolume(_) => {
                // ...
            }
        }
    }
}
