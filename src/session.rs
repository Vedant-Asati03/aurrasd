use std::{thread::JoinHandle, time::Duration};

use cpal::Stream;
use crossbeam_channel::{Receiver, Sender};

pub struct PlaybackSession {
    pub stream: Stream,
    pub decode_thread: JoinHandle<()>,
    pub shutdown_tx: Sender<()>,
    pub finished_rx: Receiver<()>,
}

impl PlaybackSession {
    pub fn new(
        stream: Stream,
        decode_thread: JoinHandle<()>,
        shutdown_tx: Sender<()>,
        finished_rx: Receiver<()>,
    ) -> Self {
        Self {
            stream,
            decode_thread,
            shutdown_tx,
            finished_rx,
        }
    }
}

impl PlaybackSession {
    pub fn stop(session: PlaybackSession) {
        let _ = session.shutdown_tx.send(());

        match session.finished_rx.recv_timeout(Duration::from_secs(2)) {
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                tracing::warn!("Timeout waiting for decode thread to cleanly finish");
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {}
        }

        drop(session.stream);
        if let Err(err) = session.decode_thread.join() {
            tracing::error!("Failed to join decode thread: {:?}", err);
        }
    }
}
