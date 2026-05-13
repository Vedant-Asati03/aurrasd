use crate::audio::{
    INTERNAL_FORMAT, PREBUFFER_MS,
    decode::decode_thread,
    output::{create_audio_buffer, get_output_device, play_audio, select_output_format},
};

use std::{
    sync::{Arc, atomic::AtomicBool},
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::Result;
use cpal::Stream;
use crossbeam_channel::{Receiver, Sender, bounded};

pub struct PlaybackSession {
    pub stream: Stream,
    pub decode_thread: JoinHandle<()>,
    pub shutdown_tx: Sender<()>,
    pub drained_rx: Receiver<()>,
    /// Receives an error message if the cpal stream encounters a fatal error.
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
        let (prebuffer_done_tx, prebuffer_done_rx) = bounded::<()>(1);

        let eof_flag_clone = Arc::clone(&eof_flag);
        let path_clone = path.to_string();

        let decode_handle = thread::Builder::new()
            .name("decode".into())
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                if let Err(err) = decode_thread(&path_clone, producer, shutdown_rx, eof_flag) {
                    tracing::error!("Decode error: {err:#}");
                }
            })
            .expect("failed to spawn decode thread");

        {
            let consumer_ref = &consumer; // borrow for inspection only
            let prebuffer_samples = (INTERNAL_FORMAT.sample_rate as usize
                * INTERNAL_FORMAT.channels as usize
                * PREBUFFER_MS)
                / 1000;

            use ringbuf::traits::Observer;
            use std::sync::atomic::{AtomicUsize, Ordering};

            let occupied = Arc::new(AtomicUsize::new(0));
            let occupied_clone = Arc::clone(&occupied);

            let _ = thread::Builder::new()
                .name("prebuffer-watch".into())
                .stack_size(256 * 1024)
                .spawn(move || {
                    loop {
                        let current = occupied_clone.load(Ordering::Relaxed);
                        if current >= prebuffer_samples {
                            let _ = prebuffer_done_tx.send(());
                            break;
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                });

            loop {
                let level = consumer_ref.occupied_len();
                occupied.store(level, Ordering::Relaxed);
                if level >= prebuffer_samples {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
        }

        let stream = play_audio(
            consumer,
            eof_flag_clone,
            drained_tx,
            stream_error_tx,
            prebuffer_done_rx,
        )?;

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
