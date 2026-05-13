use crate::audio::{
    AudioFormat, INTERNAL_BUFFER_SECONDS, INTERNAL_FORMAT, PREBUFFER_MS,
    output_adapter::OutputAdapter,
};

use anyhow::{Context, Result, anyhow};
use cpal::{
    Device, SampleFormat, StreamConfig,
    traits::{DeviceTrait, HostTrait},
};
use ringbuf::{
    HeapRb,
    traits::{Consumer, Observer, Split},
};
use std::sync::{Arc, atomic};

pub fn get_output_device() -> Result<Device> {
    cpal::default_host()
        .default_output_device()
        .context("No output device found")
}

pub fn select_output_format(device: &Device) -> Result<AudioFormat> {
    let supported = device.supported_output_configs()?;
    for cfg in supported.clone() {
        if (cfg.sample_format() == SampleFormat::F32
            || cfg.sample_format() == SampleFormat::I16
            || cfg.sample_format() == SampleFormat::U16)
            && cfg.channels() == INTERNAL_FORMAT.channels
        {
            let min = cfg.min_sample_rate();
            let max = cfg.max_sample_rate();
            if INTERNAL_FORMAT.sample_rate >= min && INTERNAL_FORMAT.sample_rate <= max {
                return Ok(AudioFormat::new(
                    cfg.channels(),
                    INTERNAL_FORMAT.sample_rate,
                    cfg.sample_format(),
                ));
            }
        }
    }
    for cfg in supported {
        if cfg.sample_format() == SampleFormat::F32
            || cfg.sample_format() == SampleFormat::I16
            || cfg.sample_format() == SampleFormat::U16
        {
            return Ok(AudioFormat::new(
                cfg.channels(),
                cfg.with_max_sample_rate().sample_rate(),
                cfg.sample_format(),
            ));
        }
    }
    Err(anyhow!("No compatible output format"))
}

pub fn create_audio_buffer(
    device_format: &AudioFormat,
) -> (ringbuf::HeapProd<f32>, ringbuf::HeapCons<f32>) {
    let capacity = INTERNAL_FORMAT.sample_rate as usize
        * INTERNAL_FORMAT.channels as usize
        * INTERNAL_BUFFER_SECONDS;
    let device_capacity = device_format.sample_rate as usize
        * device_format.channels as usize
        * INTERNAL_BUFFER_SECONDS;
    HeapRb::<f32>::new(capacity.max(device_capacity)).split()
}

pub fn play_audio(
    mut consumer: ringbuf::HeapCons<f32>,
    flush_flag: Arc<atomic::AtomicBool>,
    stream_error_tx: crossbeam_channel::Sender<String>,
) -> Result<cpal::Stream> {
    let device = get_output_device()?;
    let device_format = select_output_format(&device)?;

    let config = StreamConfig {
        channels: device_format.channels,
        sample_rate: cpal::SampleRate::from(device_format.sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let mut adapter = OutputAdapter::new(device_format.clone());
    let error_callback = move |err: cpal::StreamError| {
        tracing::error!("Stream error: {err}");
        let _ = stream_error_tx.try_send(format!("Stream error: {err}"));
    };

    let prebuffer_samples =
        (INTERNAL_FORMAT.sample_rate as usize * INTERNAL_FORMAT.channels as usize * PREBUFFER_MS)
            / 1000;
    let mut is_buffering = true;

    let stream = match device_format.sample_format {
        SampleFormat::F32 => device.build_output_stream(
            &config,
            move |output: &mut [f32], _| {
                if flush_flag.swap(false, atomic::Ordering::AcqRel) {
                    consumer.skip(consumer.occupied_len());
                    is_buffering = true;
                }

                if !is_buffering && consumer.is_empty() {
                    is_buffering = true;
                }

                if is_buffering {
                    if consumer.occupied_len() >= prebuffer_samples {
                        is_buffering = false;
                    } else {
                        output.fill(0.0);
                        return;
                    }
                }
                adapter.fill_buffer(&mut consumer, output);
            },
            error_callback,
            None,
        )?,
        SampleFormat::I16 => {
            let mut temp_buffer = Vec::new();
            device.build_output_stream(
                &config,
                move |output: &mut [i16], _| {
                    if flush_flag.swap(false, atomic::Ordering::AcqRel) {
                        while consumer.try_pop().is_some() {}
                        is_buffering = true;
                    }
                    if is_buffering {
                        if consumer.occupied_len() >= prebuffer_samples {
                            is_buffering = false;
                        } else {
                            output.fill(cpal::Sample::from_sample(0.0_f32));
                            return;
                        }
                    }
                    temp_buffer.resize(output.len(), 0.0_f32);
                    adapter.fill_buffer(&mut consumer, &mut temp_buffer);
                    for (i, &f) in temp_buffer.iter().enumerate() {
                        output[i] = cpal::Sample::from_sample(f);
                    }
                },
                error_callback,
                None,
            )?
        }
        SampleFormat::U16 => {
            let mut temp_buffer = Vec::new();
            device.build_output_stream(
                &config,
                move |output: &mut [u16], _| {
                    if flush_flag.swap(false, atomic::Ordering::AcqRel) {
                        while consumer.try_pop().is_some() {}
                        is_buffering = true;
                    }
                    if is_buffering {
                        if consumer.occupied_len() >= prebuffer_samples {
                            is_buffering = false;
                        } else {
                            output.fill(cpal::Sample::from_sample(0.0_f32));
                            return;
                        }
                    }
                    temp_buffer.resize(output.len(), 0.0_f32);
                    adapter.fill_buffer(&mut consumer, &mut temp_buffer);
                    for (i, &f) in temp_buffer.iter().enumerate() {
                        output[i] = cpal::Sample::from_sample(f);
                    }
                },
                error_callback,
                None,
            )?
        }
        _ => return Err(anyhow!("Unsupported sample format")),
    };

    Ok(stream)
}
