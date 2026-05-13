use crate::audio::output::{
    create_audio_buffer, get_output_device, play_audio, select_output_format,
};

use anyhow::Result;
use cpal::{Stream, traits::StreamTrait};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::sync::{Arc, atomic::AtomicBool};

pub struct AudioEngine {
    pub stream: Stream,
    pub producer: Option<ringbuf::HeapProd<f32>>,
    pub flush_flag: Arc<AtomicBool>,
    pub stream_error_tx: Sender<String>,
    pub stream_error_rx: Receiver<String>,
}

impl AudioEngine {
    pub fn new() -> Result<Self> {
        let device = get_output_device()?;
        let device_format = select_output_format(&device)?;

        let (producer, consumer) = create_audio_buffer(&device_format);
        let flush_flag = Arc::new(AtomicBool::new(false));

        let (stream_error_tx, stream_error_rx) = bounded::<String>(4);

        let stream = play_audio(consumer, Arc::clone(&flush_flag), stream_error_tx.clone())?;

        stream.play()?;

        Ok(Self {
            stream,
            producer: Some(producer),
            flush_flag,
            stream_error_tx,
            stream_error_rx,
        })
    }
}
