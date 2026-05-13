use crate::audio::{
    AudioFormat, INTERNAL_BUFFER_SECONDS, INTERNAL_FORMAT, output_adapter::OutputAdapter,
};

use std::sync::{Arc, atomic};

use anyhow::{Context, Result, anyhow};
use cpal::{
    Device, SampleFormat, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use ringbuf::{
    HeapRb,
    traits::{Observer, Split},
};

pub fn get_output_device() -> Result<Device> {
    cpal::default_host()
        .default_output_device()
        .context("No output device found")
}

pub fn select_output_format(device: &Device) -> Result<AudioFormat> {
    let supported = device.supported_output_configs()?;

    // exact match
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

    // nearest match
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

/// Create the ring buffer sized to the *device* sample rate so that the
/// INTERNAL_BUFFER_SECONDS constant is meaningful even after resampling.
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
    eof_flag: Arc<atomic::AtomicBool>,
    drained_tx: crossbeam_channel::Sender<()>,
    stream_error_tx: crossbeam_channel::Sender<String>,
    prebuffer_rx: crossbeam_channel::Receiver<()>,
) -> Result<cpal::Stream> {
    let device = get_output_device()?;
    let device_format = select_output_format(&device)?;

    let config = StreamConfig {
        channels: device_format.channels,
        sample_rate: cpal::SampleRate::from(device_format.sample_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    drop(prebuffer_rx);

    let mut adapter = OutputAdapter::new(device_format.clone());

    let stream_error_tx_clone = stream_error_tx.clone();
    let error_callback = move |err: cpal::StreamError| {
        let msg = format!("Stream error: {err}");
        tracing::error!("{}", msg);
        let _ = stream_error_tx_clone.try_send(msg);
    };

    let stream = match device_format.sample_format {
        SampleFormat::F32 => {
            let mut sent_drained = false;
            device.build_output_stream(
                &config,
                move |output: &mut [f32], _| {
                    adapter.fill_buffer(&mut consumer, output);
                    if !sent_drained
                        && consumer.is_empty()
                        && eof_flag.load(atomic::Ordering::Acquire)
                    {
                        let _ = drained_tx.try_send(());
                        sent_drained = true;
                    }
                },
                error_callback,
                None,
            )?
        }
        SampleFormat::I16 => {
            let mut temp_buffer = Vec::new();
            let mut sent_drained = false;
            device.build_output_stream(
                &config,
                move |output: &mut [i16], _| {
                    temp_buffer.resize(output.len(), 0.0_f32);
                    adapter.fill_buffer(&mut consumer, &mut temp_buffer);
                    for (i, &f) in temp_buffer.iter().enumerate() {
                        output[i] = cpal::Sample::from_sample(f);
                    }
                    if !sent_drained
                        && consumer.is_empty()
                        && eof_flag.load(atomic::Ordering::Acquire)
                    {
                        let _ = drained_tx.try_send(());
                        sent_drained = true;
                    }
                },
                error_callback,
                None,
            )?
        }
        SampleFormat::U16 => {
            let mut temp_buffer = Vec::new();
            let mut sent_drained = false;
            device.build_output_stream(
                &config,
                move |output: &mut [u16], _| {
                    temp_buffer.resize(output.len(), 0.0_f32);
                    adapter.fill_buffer(&mut consumer, &mut temp_buffer);
                    for (i, &f) in temp_buffer.iter().enumerate() {
                        output[i] = cpal::Sample::from_sample(f);
                    }
                    if !sent_drained
                        && consumer.is_empty()
                        && eof_flag.load(atomic::Ordering::Acquire)
                    {
                        let _ = drained_tx.try_send(());
                        sent_drained = true;
                    }
                },
                error_callback,
                None,
            )?
        }
        _ => return Err(anyhow!("Unsupported sample format")),
    };

    stream.play()?;
    Ok(stream)
}
