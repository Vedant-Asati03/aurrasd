use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use anyhow::Result;
use crossbeam_channel::Receiver;

use crate::{
    audio::{decode::decode_thread, ring::RingBuffer, types::AudioFormat},
    command::Command,
};

pub fn run_control_loop(
    cmd_rx: Receiver<Command>,
    ring: Arc<RingBuffer>,
    fmt_tx: crossbeam_channel::Sender<AudioFormat>,
) -> Result<()> {
    let mut current_decode: Option<thread::JoinHandle<()>> = None;
    let mut stop_flag: Option<Arc<AtomicBool>> = None;

    loop {
        match cmd_rx.recv()? {
            Command::Play(path) => {
                if let Some(flag) = stop_flag.take() {
                    flag.store(true, Ordering::Relaxed);
                }

                if let Some(handle) = current_decode.take() {
                    let _ = handle.join();
                }

                let ring_clone = ring.clone();
                let fmt_tx = fmt_tx.clone();

                let path_str = path.to_string_lossy().to_string();
                let is_url = path_str.starts_with("http://") || path_str.starts_with("https://");

                let stop = Arc::new(AtomicBool::new(false));
                let stop_clone = stop.clone();

                let handle = thread::spawn(move || {
                    let _ = decode_thread(&path_str, is_url, ring_clone, fmt_tx, stop_clone);
                });

                current_decode = Some(handle);
                stop_flag = Some(stop);
            }

            Command::Stop => {
                if let Some(flag) = stop_flag.take() {
                    flag.store(true, Ordering::Relaxed);
                }

                if let Some(handle) = current_decode.take() {
                    let _ = handle.join();
                }
            }

            Command::Pause => {
                // later aligator
            }

            Command::Resume => {
                // ...
            }

            Command::SetVolume(_) => {
                // ...
            }
        }
    }
}
