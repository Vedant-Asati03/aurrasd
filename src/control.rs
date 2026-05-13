use crate::{
    api::{Command, Event, PlaybackState, PlaybackStatus},
    audio::{decode::decode_thread, session::AudioEngine},
};

use anyhow::Result;
use cpal::traits::StreamTrait;
use crossbeam_channel::{Receiver, Select, Sender, bounded};
use std::{sync::atomic::Ordering, thread};

enum Msg {
    Command(Option<Command>),
    ProducerReturned(Option<ringbuf::HeapProd<f32>>),
    StreamError(Option<String>),
}

struct Player {
    state: PlaybackState,
    producer: Option<ringbuf::HeapProd<f32>>,
    decode_shutdown: Option<Sender<()>>,
}

impl Player {
    fn try_start_next(
        &mut self,
        engine: &AudioEngine,
        prod_tx: Sender<ringbuf::HeapProd<f32>>,
        event_tx: &Sender<Event>,
    ) {
        if self.producer.is_none() || self.state.status != PlaybackStatus::Playing {
            return;
        }

        if let Some(path) = self.state.queue.pop_front() {
            let prod = self.producer.take().unwrap();
            let _ = event_tx.send(Event::QueueUpdated);
            let _ = event_tx.send(Event::PlaybackStarted(path.clone()));

            let (shtx, shrx) = bounded(1);
            self.decode_shutdown = Some(shtx);

            let p_tx = prod_tx.clone();
            let err_tx = engine.stream_error_tx.clone();
            thread::spawn(move || decode_thread(path, prod, shrx, err_tx, p_tx));
        } else {
            self.state.status = PlaybackStatus::Stopped;
            let _ = event_tx.send(Event::PlaybackFinished);
            let _ = event_tx.send(Event::StateChanged(self.state.status));
        }
    }

    /// Helper to cleanly kill the active decode thread
    fn kill_current_track(&mut self) {
        if let Some(tx) = self.decode_shutdown.take() {
            let _ = tx.send(());
        }
    }
}

pub fn run_control_loop(cmd_rx: Receiver<Command>, event_tx: Sender<Event>) -> Result<()> {
    let mut engine = AudioEngine::new()?;
    let (prod_tx, prod_rx) = bounded::<ringbuf::HeapProd<f32>>(1);

    let mut player = Player {
        state: PlaybackState::new(),
        producer: engine.producer.take(),
        decode_shutdown: None,
    };

    loop {
        let msg = {
            let mut sel = Select::new();
            let cmd_idx = sel.recv(&cmd_rx);
            let prod_idx = sel.recv(&prod_rx);
            let err_idx = sel.recv(&engine.stream_error_rx);

            let oper = sel.select();
            match oper.index() {
                i if i == cmd_idx => Msg::Command(oper.recv(&cmd_rx).ok()),
                i if i == prod_idx => Msg::ProducerReturned(oper.recv(&prod_rx).ok()),
                i if i == err_idx => Msg::StreamError(oper.recv(&engine.stream_error_rx).ok()),
                _ => unreachable!(),
            }
        };

        match msg {
            Msg::Command(Some(cmd)) => match cmd {
                Command::Play(path) => {
                    player.state.queue.push_front(path);
                    player.state.status = PlaybackStatus::Playing;
                    player.kill_current_track();
                    engine.flush_flag.store(true, Ordering::Release);
                    player.try_start_next(&engine, prod_tx.clone(), &event_tx);
                }
                Command::Next => {
                    player.state.status = PlaybackStatus::Playing;
                    player.kill_current_track();
                    engine.flush_flag.store(true, Ordering::Release);
                    player.try_start_next(&engine, prod_tx.clone(), &event_tx);
                }
                Command::Stop => {
                    player.state.status = PlaybackStatus::Stopped;
                    player.kill_current_track();
                    engine.flush_flag.store(true, Ordering::Release);
                    let _ = event_tx.send(Event::PlaybackStopped);
                    let _ = event_tx.send(Event::StateChanged(player.state.status));
                }
                Command::Pause => {
                    if player.state.status == PlaybackStatus::Playing {
                        if engine.stream.pause().is_ok() {
                            player.state.status = PlaybackStatus::Paused;
                            let _ = event_tx.send(Event::PlaybackPaused);
                            let _ = event_tx.send(Event::StateChanged(player.state.status));
                        }
                    }
                }
                Command::Resume => {
                    if player.state.status == PlaybackStatus::Paused {
                        if engine.stream.play().is_ok() {
                            player.state.status = PlaybackStatus::Playing;
                            let _ = event_tx.send(Event::PlaybackResumed);
                            let _ = event_tx.send(Event::StateChanged(player.state.status));
                        }
                    }
                }
                Command::Enqueue(path) => {
                    player.state.queue.push_back(path);
                    let _ = event_tx.send(Event::QueueUpdated);
                    if player.state.status == PlaybackStatus::Stopped && player.producer.is_some() {
                        player.state.status = PlaybackStatus::Playing;
                        player.try_start_next(&engine, prod_tx.clone(), &event_tx);
                    }
                }
                Command::ClearQueue => {
                    player.state.queue.clear();
                    let _ = event_tx.send(Event::QueueUpdated);
                }
                Command::SetVolume(_) | Command::Previous => {
                    let _ = event_tx.send(Event::Error("Not yet implemented".into()));
                }
            },
            Msg::ProducerReturned(Some(prod)) => {
                player.producer = Some(prod);
                player.try_start_next(&engine, prod_tx.clone(), &event_tx);
            }
            Msg::StreamError(Some(msg)) => {
                tracing::error!("Stream error: {}", msg);
                let _ = event_tx.send(Event::Error(msg));
            }
            _ => break,
        }
    }
    Ok(())
}
