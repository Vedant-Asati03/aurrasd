use anyhow::{Context, Result};
use cpal::{
    Device, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use crossbeam_channel::Receiver;

use crate::audio::types::AudioFormat;

fn get_output_device() -> Result<Device> {
    cpal::default_host()
        .default_output_device()
        .context("No output device found")
}

pub fn play_audio(data_rx: Receiver<f32>, format: &AudioFormat) -> Result<cpal::Stream> {
    let device = get_output_device()?;

    let config = StreamConfig {
        channels: format.channels,
        sample_rate: format.sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    let stream = device.build_output_stream(
        &config,
        move |output: &mut [f32], _| {
            for sample in output.iter_mut() {
                match data_rx.try_recv() {
                    Ok(s) => *sample = s,
                    Err(crossbeam_channel::TryRecvError::Empty) => *sample = 0.0,
                    Err(crossbeam_channel::TryRecvError::Disconnected) => *sample = 0.0,
                }
            }
        },
        move |err| {
            eprintln!("Playback error: {}", err);
        },
        None,
    )?;

    stream.play()?;

    Ok(stream)
}
