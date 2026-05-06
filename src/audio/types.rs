use std::fmt::Display;

#[derive(Clone, Debug)]
pub struct AudioFormat {
    pub channels: u16,
    pub sample_rate: u32,
}

impl Display for AudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AudioFormat(channels: {}, sample_rate: {} Hz)", self.channels, self.sample_rate)
    }
}
