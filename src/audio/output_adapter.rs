use crate::audio::{AudioFormat, FFT_CHUNK_SIZE, INTERNAL_FORMAT};

use audioadapter_buffers::direct::InterleavedSlice;
use ringbuf::traits::Consumer;
use rubato::{Fft, FixedSync, Indexing, Resampler};

pub struct OutputAdapter {
    device_format: AudioFormat,
    resampler: Option<Fft<f32>>,
    accum: Vec<f32>,
    scratch: Vec<f32>,
}

impl OutputAdapter {
    pub fn new(device_format: AudioFormat) -> Self {
        let needs_resample = device_format.sample_rate != INTERNAL_FORMAT.sample_rate;

        let resampler = if needs_resample {
            Some(
                Fft::<f32>::new(
                    INTERNAL_FORMAT.sample_rate as usize,
                    device_format.sample_rate as usize,
                    FFT_CHUNK_SIZE,
                    INTERNAL_FORMAT.channels as usize,
                    INTERNAL_FORMAT.channels as usize,
                    FixedSync::Input,
                )
                .unwrap(),
            )
        } else {
            None
        };

        let scratch_capacity = if needs_resample {
            // resampler output_frames_max * channels — conservative upper bound
            FFT_CHUNK_SIZE * 4 * INTERNAL_FORMAT.channels as usize
        } else {
            // cpal callback is typically ~512-2048 frames
            4096 * INTERNAL_FORMAT.channels as usize
        };

        Self {
            device_format,
            resampler,
            accum: Vec::new(),
            scratch: Vec::with_capacity(scratch_capacity),
        }
    }

    /// Fill `output` directly from `consumer`, resampling and converting channels inline.
    /// Zeros any frames that couldn't be filled (underrun).
    pub fn fill_buffer(&mut self, consumer: &mut ringbuf::HeapCons<f32>, output: &mut [f32]) {
        let ch = INTERNAL_FORMAT.channels as usize;
        let filled = if let Some(resampler) = self.resampler.as_mut() {
            fill_with_resample(
                consumer,
                output,
                resampler,
                &mut self.accum,
                &mut self.scratch,
                ch,
                &self.device_format,
            )
        } else {
            fill_direct(consumer, output, ch, &self.device_format)
        };

        output[filled..].fill(0.0);
    }
}

/// Direct path: no resampling needed. Pull samples from ring buffer, convert channels inline.
fn fill_direct(
    consumer: &mut ringbuf::HeapCons<f32>,
    output: &mut [f32],
    internal_ch: usize,
    device_format: &AudioFormat,
) -> usize {
    let device_ch = device_format.channels as usize;
    let out_frames = output.len() / device_ch;

    match (internal_ch, device_ch) {
        (2, 2) | (1, 1) => {
            // Exact match — read directly into output
            let mut written = 0;
            while written < output.len() {
                match consumer.try_pop() {
                    Some(s) => {
                        output[written] = s;
                        written += 1;
                    }
                    None => return written,
                }
            }
            written
        }
        (2, 1) => {
            // Stereo → mono: mix down pairs
            let mut written = 0;
            for frame in 0..out_frames {
                match (consumer.try_pop(), consumer.try_pop()) {
                    (Some(l), Some(r)) => {
                        output[frame] = (l + r) * 0.5;
                        written += 1;
                    }
                    _ => return written,
                }
            }
            written
        }
        (1, 2) => {
            // Mono → stereo: duplicate
            let mut written = 0;
            for frame in 0..out_frames {
                match consumer.try_pop() {
                    Some(s) => {
                        output[frame * 2] = s;
                        output[frame * 2 + 1] = s;
                        written += 2;
                    }
                    None => return written,
                }
            }
            written
        }
        _ => 0,
    }
}

/// Resampling path: accumulate into `accum`, process chunks, write converted output inline.
fn fill_with_resample(
    consumer: &mut ringbuf::HeapCons<f32>,
    output: &mut [f32],
    resampler: &mut Fft<f32>,
    accum: &mut Vec<f32>,
    scratch: &mut Vec<f32>,
    internal_ch: usize,
    device_format: &AudioFormat,
) -> usize {
    while let Some(s) = consumer.try_pop() {
        accum.push(s);
    }

    let device_ch = device_format.channels as usize;
    let mut out_pos = 0;

    loop {
        let needed_frames = resampler.input_frames_next();
        let needed_samples = needed_frames * internal_ch;

        if accum.len() < needed_samples {
            break;
        }

        let output_frames = resampler.output_frames_max();
        let output_samples = output_frames * internal_ch;
        scratch.resize(output_samples, 0.0);

        let input_adapter =
            InterleavedSlice::new(&accum[..needed_samples], internal_ch, needed_frames).unwrap();
        let mut output_adapter =
            InterleavedSlice::new_mut(scratch, internal_ch, output_frames).unwrap();

        let indexing = Indexing {
            input_offset: 0,
            output_offset: 0,
            active_channels_mask: None,
            partial_len: None,
        };

        let written_frames = match resampler.process_into_buffer(
            &input_adapter,
            &mut output_adapter,
            Some(&indexing),
        ) {
            Ok((_, w)) => w,
            Err(_) => break,
        };

        accum.drain(..needed_samples);

        let resampled = &scratch[..written_frames * internal_ch];
        let space_left = output.len() - out_pos;

        match (internal_ch, device_ch) {
            (2, 2) | (1, 1) => {
                let to_copy = resampled.len().min(space_left);
                output[out_pos..out_pos + to_copy].copy_from_slice(&resampled[..to_copy]);
                out_pos += to_copy;
            }
            (2, 1) => {
                for frame in resampled.chunks_exact(2) {
                    if out_pos >= output.len() {
                        break;
                    }
                    output[out_pos] = (frame[0] + frame[1]) * 0.5;
                    out_pos += 1;
                }
            }
            (1, 2) => {
                for &s in resampled {
                    if out_pos + 1 >= output.len() {
                        break;
                    }
                    output[out_pos] = s;
                    output[out_pos + 1] = s;
                    out_pos += 2;
                }
            }
            _ => break,
        }

        if out_pos >= output.len() {
            break;
        }
    }

    out_pos
}
