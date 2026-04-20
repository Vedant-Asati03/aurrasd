use aurrasd::{
    audio::{output::play_audio, ring::RingBuffer, types::AudioFormat},
    command::Command,
    control::run_control_loop,
};

use crossbeam_channel::bounded;

fn main() -> anyhow::Result<()> {
    let (cmd_tx, cmd_rx) = bounded::<Command>(16);
    let (fmt_tx, fmt_rx) = bounded::<AudioFormat>(1);

    // (~5 seconds stereo 48kHz)
    let ring = RingBuffer::new(48000 * 2 * 5);
    let ring_decode = ring.clone();
    let ring_audio = ring.clone();

    std::thread::spawn(move || {
        run_control_loop(cmd_rx, ring_decode, fmt_tx).unwrap();
    });

    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "nature-intro.wav".into());

    cmd_tx.send(Command::Play(path.into()))?;

    let format = fmt_rx.recv()?;

    println!(
        "Detected — {} Hz, {} channels",
        format.sample_rate, format.channels
    );

    play_audio(ring_audio, &format)?;

    Ok(())
}
