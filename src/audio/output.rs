use crate::audio::{
    constants::{INTERNAL_BUFFER_SECONDS, INTERNAL_FORMAT, PREBUFFER_MS},
    types::AudioFormat,
};

use std::{thread, time::Duration};

use anyhow::{Context, Ok, Result};
use cpal::{
    Device, SampleFormat, StreamConfig,
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

fn select_output_format(device: &Device) -> Result<AudioFormat> {
    let supported = device.supported_output_configs()?;

    // best case - exact match
    for cfg in supported.clone() {
        if cfg.sample_format() == SampleFormat::F32 && cfg.channels() == INTERNAL_FORMAT.channels {
            let min = cfg.min_sample_rate();

            let max = cfg.max_sample_rate();

            if INTERNAL_FORMAT.sample_rate >= min && INTERNAL_FORMAT.sample_rate <= max {
                return Ok(AudioFormat::new(
                    cfg.channels(),
                    INTERNAL_FORMAT.sample_rate,
                ));
            }
        }
    }

    // fallback case - stereo f32 nearest supported
    for cfg in supported.clone() {
        if cfg.sample_format() == SampleFormat::F32 && cfg.channels() == 2 {
            return Ok(AudioFormat::new(
                2,
                cfg.with_max_sample_rate().sample_rate(),
            ));
        }
    }

    // final fallback
    for cfg in supported {
        if cfg.sample_format() == SampleFormat::F32 {
            return Ok(AudioFormat::new(
                cfg.channels(),
                cfg.with_max_sample_rate().sample_rate(),
            ));
        }
    }

    Err(anyhow::anyhow!("No compatible output device format"))
}

pub fn create_audio_buffer() -> (ringbuf::HeapProd<f32>, ringbuf::HeapCons<f32>) {
    let capacity = INTERNAL_FORMAT.sample_rate as usize
        * INTERNAL_FORMAT.channels as usize
        * INTERNAL_BUFFER_SECONDS;

    HeapRb::<f32>::new(capacity).split()
}

pub fn play_audio(mut consumer: ringbuf::HeapCons<f32>) -> Result<cpal::Stream> {
    let device = get_output_device()?;
    let device_format = select_output_format(&device)?;

    let config = StreamConfig {
        channels: device_format.channels,
        sample_rate: device_format.sample_rate,
        buffer_size: cpal::BufferSize::Default,
    };

    let prebuffer_samples =
        (INTERNAL_FORMAT.sample_rate as usize * INTERNAL_FORMAT.channels as usize * PREBUFFER_MS)
            / 1000;

    while consumer.occupied_len() < prebuffer_samples {
        thread::sleep(Duration::from_millis(10));
    }

    let stream = device.build_output_stream(
        &config,
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
