#[derive(Clone, Debug)]
pub struct AudioFormat {
    pub channels: u16,
    pub sample_rate: u32,
}

impl AudioFormat {
    pub fn new(channels: u16, sample_rate: u32) -> Self {
        Self {
            channels: channels,
            sample_rate: sample_rate,
        }
    }
}
