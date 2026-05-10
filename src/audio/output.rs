use crate::audio::constants::{
    INTERNAL_BUFFER_SECONDS, INTERNAL_CHANNELS, INTERNAL_SAMPLE_RATE, PREBUFFER_MS,
};

use std::{thread, time::Duration};

use anyhow::{Context, Result};
use cpal::{
    Device, SampleFormat, StreamConfig, SupportedStreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use ringbuf::{
    HeapRb,
    traits::{Consumer, Observer, Split},
};

fn get_output_device() -> Result<Device> {
    cpal::default_host()
        .default_output_device()
        .context("No output device found")
}

fn select_output_config(device: &Device) -> Result<SupportedStreamConfig> {
    let supported = device.supported_output_configs()?;

    for cfg in supported {
        if cfg.channels() == INTERNAL_CHANNELS && cfg.sample_format() == SampleFormat::F32 {
            return Ok(cfg.with_sample_rate(INTERNAL_SAMPLE_RATE));
        }
    }

    Err(anyhow::anyhow!("No suitable output config found"))
}

pub fn create_audio_buffer() -> (ringbuf::HeapProd<f32>, ringbuf::HeapCons<f32>) {
    let capacity =
        INTERNAL_SAMPLE_RATE as usize * INTERNAL_CHANNELS as usize * INTERNAL_BUFFER_SECONDS;

    HeapRb::<f32>::new(capacity).split()
}

pub fn play_audio(mut consumer: ringbuf::HeapCons<f32>) -> Result<cpal::Stream> {
    let device = get_output_device()?;

    let config = select_output_config(&device)?;

    let prebuffer_samples =
        (INTERNAL_SAMPLE_RATE as usize * INTERNAL_CHANNELS as usize * PREBUFFER_MS) / 1000;

    while consumer.occupied_len() < prebuffer_samples {
        thread::sleep(Duration::from_millis(10));
    }

    let stream = device.build_output_stream(
        &StreamConfig {
            channels: config.channels(),
            sample_rate: config.sample_rate(),
            buffer_size: cpal::BufferSize::Default,
        },
        move |output: &mut [f32], _| {
            for sample in output.iter_mut() {
                *sample = consumer.try_pop().unwrap_or(0.0);
            }
        },
        move |err| {
            eprintln!("Playback error: {err}");
        },
        None,
    )?;

    stream.play()?;

    Ok(stream)
}
