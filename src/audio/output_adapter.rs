use crate::audio::{AudioFormat, FFT_CHUNK_SIZE, INTERNAL_FORMAT};

use audioadapter_buffers::direct::InterleavedSlice;
use ringbuf::{
    HeapRb,
    traits::{Consumer, Observer, Producer},
};
use rubato::{Fft, FixedSync, Indexing, Resampler};

pub struct OutputAdapter {
    device_format: AudioFormat,
    resampler: Option<Fft<f32>>,
    accumulation: HeapRb<f32>,
    output_buffer: HeapRb<f32>,
    scratch_in: Vec<f32>,
    scratch_out: Vec<f32>,
    scratch_mono: Vec<f32>,
    scratch_stereo: Vec<f32>,
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

        Self {
            device_format,
            resampler,
            accumulation: HeapRb::new(65536),
            output_buffer: HeapRb::new(65536),
            scratch_in: Vec::with_capacity(65536),
            scratch_out: Vec::with_capacity(65536),
            scratch_mono: Vec::with_capacity(65536),
            scratch_stereo: Vec::with_capacity(65536),
        }
    }
}

impl OutputAdapter {
    pub fn fill_buffer(&mut self, consumer: &mut ringbuf::HeapCons<f32>, output: &mut [f32]) {
        while self.output_buffer.occupied_len() < output.len() {
            let produced_any = self.process_one_chunk();

            if !produced_any {
                let mut fetched = false;
                for _ in 0..1024 {
                    if let Some(s) = consumer.try_pop() {
                        let _ = self.accumulation.try_push(s);
                        fetched = true;
                    } else {
                        break;
                    }
                }
                if !fetched {
                    break;
                }
            }
        }

        let to_copy = std::cmp::min(self.output_buffer.occupied_len(), output.len());
        for i in 0..to_copy {
            output[i] = self.output_buffer.try_pop().unwrap();
        }
        for i in to_copy..output.len() {
            output[i] = 0.0;
        }
    }

    fn process_one_chunk(&mut self) -> bool {
        let processed: &[f32] = if let Some(resampler) = self.resampler.as_mut() {
            let needed_frames = resampler.input_frames_next();
            let needed_samples = needed_frames * INTERNAL_FORMAT.channels as usize;

            if self.accumulation.occupied_len() < needed_samples {
                return false;
            }

            self.scratch_in.clear();
            for _ in 0..needed_samples {
                self.scratch_in.push(self.accumulation.try_pop().unwrap());
            }

            let input_adapter = InterleavedSlice::new(
                &self.scratch_in,
                INTERNAL_FORMAT.channels as usize,
                needed_frames,
            )
            .unwrap();

            let output_frames = resampler.output_frames_max();
            let needed_out_samples = output_frames * INTERNAL_FORMAT.channels as usize;

            self.scratch_out.clear();
            self.scratch_out.resize(needed_out_samples, 0.0);

            let mut output_adapter = InterleavedSlice::new_mut(
                &mut self.scratch_out,
                INTERNAL_FORMAT.channels as usize,
                output_frames,
            )
            .unwrap();

            let indexing = Indexing {
                input_offset: 0,
                output_offset: 0,
                active_channels_mask: None,
                partial_len: None,
            };

            let (_, written_frames) = match resampler.process_into_buffer(
                &input_adapter,
                &mut output_adapter,
                Some(&indexing),
            ) {
                Ok(v) => v,
                Err(_) => return false,
            };

            self.scratch_out
                .truncate(written_frames * INTERNAL_FORMAT.channels as usize);

            &self.scratch_out[..]
        } else {
            if self.accumulation.is_empty() {
                return false;
            }
            self.scratch_out.clear();
            while let Some(v) = self.accumulation.try_pop() {
                self.scratch_out.push(v);
            }
            &self.scratch_out[..]
        };

        if self.device_format.channels == 1 {
            self.scratch_mono.clear();
            for frame in processed.chunks_exact(2) {
                self.scratch_mono.push((frame[0] + frame[1]) * 0.5);
            }
            for &s in &self.scratch_mono {
                let _ = self.output_buffer.try_push(s);
            }
        } else if self.device_format.channels == 2 && INTERNAL_FORMAT.channels == 1 {
            self.scratch_stereo.clear();
            for &sample in processed {
                self.scratch_stereo.push(sample);
                self.scratch_stereo.push(sample);
            }
            for &s in &self.scratch_stereo {
                let _ = self.output_buffer.try_push(s);
            }
        } else {
            for &s in processed {
                let _ = self.output_buffer.try_push(s);
            }
        }

        true
    }
}
