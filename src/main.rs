use aurrasd::{command::Command, control::run_control_loop};

use std::{
    sync::Arc,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use crossbeam_channel::bounded;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let (cmd_tx, cmd_rx) = bounded::<Command>(16);

    let running = Arc::new(AtomicBool::new(true));
    let r = Arc::clone(&running);

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })?;

    let control_handle = thread::spawn(move || {
        if let Err(e) = run_control_loop(cmd_rx) {
            tracing::error!("Control thread error: {e:#}");
        }
    });

    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| String::from("nature-intro.wav"));

    cmd_tx.send(Command::Play(path.into()))?;

    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }

    drop(cmd_tx);
    let _ = control_handle.join();

    Ok(())
}
