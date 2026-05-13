use std::sync::{Arc, atomic::AtomicBool};
use std::thread::{self, JoinHandle};

use anyhow::Result;
use cpal::Stream;
use crossbeam_channel::{Receiver, Sender, bounded};

use crate::audio::{
    decode::decode_thread,
    output::{create_audio_buffer, play_audio},
};

pub struct PlaybackSession {
    pub stream: Stream,
    pub decode_thread: JoinHandle<()>,
    pub shutdown_tx: Sender<()>,
    pub drained_rx: Receiver<()>,
}

impl PlaybackSession {
    pub fn start(path: &str) -> Result<Self> {
        let (producer, consumer) = create_audio_buffer();
        let (shutdown_tx, shutdown_rx) = bounded::<()>(1);
        let eof_flag = Arc::new(AtomicBool::new(false));
        let (drained_tx, drained_rx) = bounded::<()>(1);
        let eof_flag_clone = Arc::clone(&eof_flag);

        let path_clone = path.to_string();

        let decode_thread = thread::Builder::new()
            .name("decode".into())
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                if let Err(err) = decode_thread(&path_clone, producer, shutdown_rx, eof_flag) {
                    tracing::error!("Decode error: {err:#}");
                }
            })
            .expect("failed to spawn decode thread");

        let stream = play_audio(consumer, eof_flag_clone, drained_tx)?;

        Ok(Self {
            stream,
            decode_thread,
            shutdown_tx,
            drained_rx,
        })
    }

    pub fn stop(session: PlaybackSession) {
        // Signal the decode thread to stop prematurely
        let _ = session.shutdown_tx.send(());

        // Dropping the stream immediately stops audio playback
        drop(session.stream);

        // Wait for the decoder thread to finish its work
        if let Err(err) = session.decode_thread.join() {
            tracing::error!("Failed to join decode thread: {:?}", err);
        }
    }
}
