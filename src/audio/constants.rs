use crate::audio::types::AudioFormat;

pub const INTERNAL_FORMAT: AudioFormat = AudioFormat {
    channels: 2,
    sample_rate: 48_000,
    sample_format: cpal::SampleFormat::F32,
};

pub const INTERNAL_BUFFER_SECONDS: usize = 4;

pub const PREBUFFER_MS: usize = 300;

pub const FFT_CHUNK_SIZE: usize = 1024;
