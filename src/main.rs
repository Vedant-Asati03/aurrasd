use std::{thread, time::Duration};
use crossbeam_channel::bounded;

use aurrasd::{command::Command, control::run_control_loop};

fn main() -> anyhow::Result<()> {
    let (cmd_tx, cmd_rx) = bounded::<Command>(16);

    let _control_handle = thread::spawn(move || {
        if let Err(e) = run_control_loop(cmd_rx) {
            eprintln!("Control thread error: {e:#}");
        }
    });

    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("nature-intro.wav"));

    cmd_tx.send(Command::Play(path.into()))?;

    loop {
        thread::sleep(Duration::from_secs(1));
    }
}
