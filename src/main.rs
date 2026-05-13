use aurrasd::{
    api::{Command, Event},
    control, ipc,
};

use std::{
    io::Write,
    net::TcpStream,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use crossbeam_channel::bounded;

fn show_help() {
    println!("aurrasd [OPTIONS] <COMMAND> [ARGS]");
    println!();
    println!("A simple audio playback daemon");
    println!();
    println!("Commands:");
    println!("  play <path>       - Play the specified audio file immediately");
    println!("  pause             - Pause playback");
    println!("  resume            - Resume playback");
    println!("  stop              - Stop playback");
    println!("  next              - Skip to the next track in the queue");
    println!("  prev | previous   - Go back to the previous track in the queue");
    println!("  clear             - Clear the playback queue");
    println!("  enqueue <path>    - Add the specified audio file to the end of the queue");
    println!("  volume <0.0-1.0>  - Set the playback volume (0.0 = mute, 1.0 = max)");
    println!();
    println!("Options:");
    println!("  daemon            - Run in daemon mode (default)");
    println!("  help              - Show this help message");

    std::process::exit(0);
}

fn handle_args(args: Vec<String>) -> anyhow::Result<bool> {
    if args.len() > 1 && args[1] != "daemon" {
        let cmd = match args[1].as_str() {
            "help" => {
                show_help();
                return Ok(true);
            }

            "play" => {
                if args.len() < 3 {
                    anyhow::bail!("Usage: aurrasd play <path>");
                }
                Command::Play(args[2].clone())
            }

            "pause" => Command::Pause,

            "resume" => Command::Resume,

            "stop" => Command::Stop,

            "next" => Command::Next,

            "prev" | "previous" => Command::Previous,

            "clear" => Command::ClearQueue,

            "enqueue" => {
                if args.len() < 3 {
                    anyhow::bail!("Usage: aurrasd enqueue <path>");
                }
                Command::Enqueue(args[2].clone())
            }

            "volume" => {
                if args.len() < 3 {
                    anyhow::bail!("Usage: aurrasd volume <0.0-1.0>");
                }
                let v: f32 = args[2].parse()?;
                Command::SetVolume(v)
            }
            _ => anyhow::bail!("Unknown command: {}", args[1]),
        };

        if let Ok(mut stream) = TcpStream::connect("127.0.0.1:28772") {
            let json = serde_json::to_string(&cmd)?;
            writeln!(stream, "{}", json)?;
        } else {
            eprintln!("Daemon is not running. Could not connect to 127.0.0.1:28772");
        }
        return Ok(true);
    }
    Ok(false)
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if handle_args(args)? {
        return Ok(());
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

    let active_clients = Arc::new(Mutex::new(Vec::<TcpStream>::new()));
    ipc::start_ipc_server(cmd_tx.clone(), Arc::clone(&active_clients));

    let clients_for_events = Arc::clone(&active_clients);
    let event_handle = thread::spawn(move || {
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
                } else {
                    tracing::error!("Failed to lock clients mutex for event broadcasting");
                }
            }
        }
    });

    let control_handle = thread::spawn(move || {
        if let Err(e) = control::run_control_loop(cmd_rx, event_tx) {
            tracing::error!("Control thread error: {e:#}");
        }
    });

    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_millis(100));
    }

    drop(cmd_tx);
    let _ = control_handle.join();
    let _ = event_handle.join();

    Ok(())
}
