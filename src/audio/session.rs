use crate::audio::{
    decode::decode_thread,
    output::{create_audio_buffer, get_output_device, play_audio, select_output_format},
};

use std::{
    sync::{Arc, atomic::AtomicBool},
    thread::{self, JoinHandle},
};

use anyhow::Result;
use cpal::Stream;
use crossbeam_channel::{Receiver, Sender, bounded};

pub struct PlaybackSession {
    pub stream: Stream,
    pub decode_thread: JoinHandle<()>,
    pub shutdown_tx: Sender<()>,
    pub drained_rx: Receiver<()>,
    /// Receives an error message if the cpal stream encounters a fatal error,
    /// or if the decode thread fails to process the audio.
    pub stream_error_rx: Receiver<String>,
}

impl PlaybackSession {
    pub fn start(path: &str) -> Result<Self> {
        let device = get_output_device()?;
        let device_format = select_output_format(&device)?;

        let (producer, consumer) = create_audio_buffer(&device_format);
        let (shutdown_tx, shutdown_rx) = bounded::<()>(1);
        let eof_flag = Arc::new(AtomicBool::new(false));
        let (drained_tx, drained_rx) = bounded::<()>(1);

        let (stream_error_tx, stream_error_rx) = bounded::<String>(4);

        let eof_flag_clone = Arc::clone(&eof_flag);
        let path_clone = path.to_string();

        let decode_error_tx = stream_error_tx.clone();

        let decode_handle = thread::Builder::new()
            .name("decode".into())
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                if let Err(err) = decode_thread(&path_clone, producer, shutdown_rx, eof_flag) {
                    tracing::error!("Decode error: {err:#}");
                    let _ = decode_error_tx.try_send(format!("Decode failed: {}", err));
                }
            })
            .expect("failed to spawn decode thread");

        let stream = play_audio(consumer, eof_flag_clone, drained_tx, stream_error_tx)?;

        Ok(Self {
            stream,
            decode_thread: decode_handle,
            shutdown_tx,
            drained_rx,
            stream_error_rx,
        })
    }

    pub fn stop(session: PlaybackSession) {
        let _ = session.shutdown_tx.send(());
        drop(session.stream);

        if let Err(err) = session.decode_thread.join() {
            tracing::error!("Failed to join decode thread: {:?}", err);
        }
    }
}
