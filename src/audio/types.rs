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
