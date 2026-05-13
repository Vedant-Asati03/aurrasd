use aurrasd::{
    api::{Command, Event},
    control, ipc,
};

use std::{
    io::{BufRead, BufReader, Write},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use clap::{Parser, Subcommand};
use crossbeam_channel::bounded;
use interprocess::{
    TryClone,
    local_socket::{GenericFilePath, GenericNamespaced, Stream, prelude::*},
};

#[derive(Parser)]
#[command(name = "aurrasd", version, about = "A simple audio player backend")]
struct Cli {
    #[command(subcommand)]
    command: Option<CliCommand>,
}

#[derive(Subcommand)]
pub enum CliCommand {
    /// Play the specified audio file immediately
    Play { path: String },
    /// Pause playback
    Pause,
    /// Resume playback
    Resume,
    /// Stop playback
    Stop,
    /// Skip to the next track in the queue
    Next,
    /// Go back to the previous track in the queue
    #[command(alias = "previous")]
    Prev,
    /// Clear the playback queue
    Clear,
    /// Add the specified audio file to the end of the queue
    Enqueue { path: String },
    /// Set the playback volume (0.0 = mute, 1.0 = max)
    Volume { level: f32 },
    /// Show current playback state and queue
    Status,
    /// Run in daemon mode (default if no command provided)
    Daemon,
}

fn handle_cli_client(cli_cmd: CliCommand) -> anyhow::Result<()> {
    let cmd = match cli_cmd {
        CliCommand::Play { path } => Command::Play(path),
        CliCommand::Pause => Command::Pause,
        CliCommand::Resume => Command::Resume,
        CliCommand::Stop => Command::Stop,
        CliCommand::Next => Command::Next,
        CliCommand::Prev => Command::Previous,
        CliCommand::Clear => Command::ClearQueue,
        CliCommand::Enqueue { path } => Command::Enqueue(path),
        CliCommand::Volume { level } => Command::SetVolume(level),
        CliCommand::Status => Command::GetState,
        CliCommand::Daemon => return Ok(()),
    };

    let name = if GenericNamespaced::is_supported() {
        "aurrasd.sock".to_ns_name::<GenericNamespaced>().unwrap()
    } else {
        std::env::temp_dir()
            .join("aurrasd.sock")
            .to_string_lossy()
            .into_owned()
            .to_fs_name::<GenericFilePath>()
            .unwrap()
    };

    match Stream::connect(name.clone()) {
        Ok(mut stream) => {
            let json = serde_json::to_string(&cmd)?;
            writeln!(stream, "{}", json)?;

            if let Command::GetState = cmd {
                let reader = BufReader::new(stream.try_clone().expect("Failed to clone stream"));
                for line_result in reader.lines() {
                    if let Ok(line) = line_result {
                        if let Ok(Event::FullState(state)) = serde_json::from_str::<Event>(&line) {
                            println!("Status: {:?}", state.status);
                            if state.queue.is_empty() {
                                println!("Queue is empty.");
                            } else {
                                println!("Queue ({} items):", state.queue.len());
                                for (i, track) in state.queue.iter().enumerate() {
                                    println!("  {}. {}", i + 1, track);
                                }
                            }
                            break;
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        Err(_) => {
            eprintln!("Daemon is not running. Could not connect to socket.");
            std::process::exit(1);
        }
    }

    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        if !matches!(cmd, CliCommand::Daemon) {
            handle_cli_client(cmd)?;
            return Ok(());
        }
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let (cmd_tx, cmd_rx) = bounded::<Command>(16);
    let (event_tx, event_rx) = bounded::<Event>(16);

    let running = Arc::new(AtomicBool::new(true));
    let r = Arc::clone(&running);

    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })?;

    let active_clients = Arc::new(Mutex::new(Vec::<Stream>::new()));
    ipc::start_ipc_server(cmd_tx.clone(), Arc::clone(&active_clients));

    let clients_for_events = Arc::clone(&active_clients);
    let event_handle = thread::Builder::new()
        .name("events".into())
        .stack_size(1024 * 1024)
        .spawn(move || {
            while let Ok(event) = event_rx.recv() {
                tracing::info!("Event: {:?}", event);
                if let Ok(json) = serde_json::to_string(&event) {
                    if let Ok(mut clients) = clients_for_events.lock() {
                        let mut dead_count = 0;
                        clients.retain_mut(|client| match writeln!(client, "{}", json) {
                            Ok(_) => {
                                let _ = client.flush();
                                true
                            }
                            Err(_) => {
                                dead_count += 1;
                                false
                            }
                        });
                        if dead_count > 0 {
                            tracing::debug!("Dropped {} dead IPC client(s)", dead_count);
                        }
                    }
                }
            }
        })?;

    let control_handle = thread::Builder::new()
        .name("control".into())
        .stack_size(2 * 1024 * 1024)
        .spawn(move || {
            if let Err(e) = control::run_control_loop(cmd_rx, event_tx) {
                tracing::error!("Control thread error: {e:#}");
            }
        })?;

    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }

    tracing::info!("Shutting down...");
    drop(cmd_tx);
    let _ = control_handle.join();
    let _ = event_handle.join();

    Ok(())
}
