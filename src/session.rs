use std::{thread::JoinHandle, time::Duration};

use cpal::Stream;
use crossbeam_channel::{Receiver, Sender};

pub struct PlaybackSession {
    pub stream: Stream,
    pub decode_thread: JoinHandle<()>,
    pub shutdown_rx: Sender<()>,
    pub finished_tx: Receiver<()>,
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
            shutdown_rx: shutdown_tx,
            finished_tx: finished_rx,
        }
    }
}

impl PlaybackSession {
    pub fn stop(session: PlaybackSession) {
        let _ = session.shutdown_rx.send(());
        let _ = session.finished_tx.recv_timeout(Duration::from_secs(2));
        drop(session.stream);
        let _ = session.decode_thread.join();
    }
}
