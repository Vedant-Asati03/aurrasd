#[derive(Debug)]
pub enum Command {
    Play(String),
    Stop,
    Pause,
    Resume,
    SetVolume(f32),
}
