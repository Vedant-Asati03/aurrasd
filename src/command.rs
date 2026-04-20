use std::path::PathBuf;

#[derive(Debug)]
pub enum Command {
    Play(PathBuf),
    Stop,
    Pause,
    Resume,
    SetVolume(f32),
}
