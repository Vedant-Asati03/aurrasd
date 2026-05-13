pub mod decode;
pub mod output;
pub mod output_adapter;
pub mod session;
use cpal::SampleFormat;

#[derive(Clone, Debug)]
pub struct AudioFormat {
    pub channels: u16,
    pub sample_rate: u32,
    pub sample_format: SampleFormat,
}

impl AudioFormat {
    pub fn new(channels: u16, sample_rate: u32, sample_format: SampleFormat) -> Self {
        Self {
            channels,
            sample_rate,
            sample_format,
        }
    }
}

pub const INTERNAL_FORMAT: AudioFormat = AudioFormat {
    channels: 2,
    sample_rate: 48_000,
    sample_format: cpal::SampleFormat::F32,
};

pub const INTERNAL_BUFFER_SECONDS: usize = 4;

pub const PREBUFFER_MS: usize = 300;

pub const FFT_CHUNK_SIZE: usize = 1024;

/// Maximum number of interleaved f32 samples allowed to accumulate in a
/// resampler's input buffer before we consider it a runaway growth situation
/// and stop feeding it.  At 48 kHz stereo that is ~10 seconds of audio.
pub const ACCUM_MAX_SAMPLES: usize = 48_000 * 2 * 10;
