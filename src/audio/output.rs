use std::sync::{Arc, Barrier};

use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::audio::{ring::RingBuffer, types::AudioFormat};

pub fn play_audio(ring: Arc<RingBuffer>, _format: &AudioFormat) -> Result<()> {
    let device = cpal::default_host()
        .default_output_device()
        .context("No output device found")?;

    // let config = StreamConfig {
    //     channels: format.channels,
    //     sample_rate: format.sample_rate,
    //     buffer_size: cpal::BufferSize::Default,
    // };
    let config = device.default_output_config()?.config();

    let barrier = Arc::new(Barrier::new(2));
    let err_barrier_cb = barrier.clone();
    let err_barrier = barrier.clone();

    let stream = device.build_output_stream(
        &config,
        move |output: &mut [f32], _| {
            let read = ring.read(output);

            if read == 0 {
                if ring.is_finished() {
                    // end of stream → signal completion
                    err_barrier_cb.wait();
                    return;
                }

                // temporary underrun → silence
                for sample in output {
                    *sample = 0.0;
                }
                return;
            }

            // fill remaining with silence (underrun)
            if read < output.len() {
                for sample in &mut output[read..] {
                    *sample = 0.0;
                }
            }
        },
        move |err| {
            eprintln!("Stream error: {err}");
            err_barrier.wait();
        },
        None,
    )?;

    stream.play()?;
    barrier.wait();

    Ok(())
}
