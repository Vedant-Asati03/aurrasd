use crate::api::Command;

use std::{
    io::{BufRead, BufReader},
    net::{TcpListener, TcpStream},
    sync::{Arc, Mutex},
    thread,
};

use crossbeam_channel::Sender;

/// A connected IPC client entry.  Keeping the `JoinHandle` lets us wait for
/// the per-client read thread on shutdown and gives us a hook for future
/// diagnostics (e.g. active-thread count).
struct Client {
    stream: TcpStream,
    // The join handle is stored so we hold onto the thread's lifetime.
    // We never explicitly join these because the IPC server currently has no
    // clean shutdown path of its own; the OS reclaims them when the process
    // exits.  Storing the handle here at least prevents the "detached thread"
    // smell and makes it easy to add a proper join later.
    #[allow(dead_code)]
    thread: thread::JoinHandle<()>,
}

pub fn start_ipc_server(cmd_tx: Sender<Command>, clients: Arc<Mutex<Vec<TcpStream>>>) {
    thread::Builder::new()
        .name("ipc-listener".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            let listener = match TcpListener::bind("127.0.0.1:28772") {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("Failed to bind IPC server: {e}");
                    return;
                }
            };
            tracing::info!("IPC server listening on 127.0.0.1:28772");

            // We maintain our own list of `Client` structs (with JoinHandles)
            // separately from the shared `clients` vec (write-only streams used
            // for event broadcasting).  Dead entries are pruned proactively when
            // a client disconnects, rather than lazily on the next broadcast.
            let mut client_threads: Vec<Client> = Vec::new();

            let reap = |threads: &mut Vec<Client>| {
                threads.retain(|c| !c.thread.is_finished());
            };

            for stream in listener.incoming() {
                reap(&mut client_threads);

                match stream {
                    Ok(stream) => {
                        let peer_addr = stream
                            .peer_addr()
                            .map(|a| a.to_string())
                            .unwrap_or_else(|_| "unknown".to_string());
                        tracing::debug!("New IPC client connected from {}", peer_addr);

                        let stream_for_events = match stream.try_clone() {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::error!(
                                    "Failed to clone stream for client {}: {}",
                                    peer_addr,
                                    e
                                );
                                continue;
                            }
                        };

                        let stream_for_read = match stream.try_clone() {
                            Ok(s) => s,
                            Err(e) => {
                                tracing::error!(
                                    "Failed to clone stream for read loop {}: {}",
                                    peer_addr,
                                    e
                                );
                                continue;
                            }
                        };

                        if let Ok(mut guard) = clients.lock() {
                            guard.push(stream_for_events);
                        } else {
                            tracing::error!("Failed to lock clients mutex");
                            continue;
                        }

                        let tx = cmd_tx.clone();
                        let clients_for_disconnect = Arc::clone(&clients);
                        let peer_addr_clone = peer_addr.clone();

                        let handle = thread::Builder::new()
                            .name(format!("ipc-{}", peer_addr))
                            .stack_size(512 * 1024)
                            .spawn(move || {
                                let reader = BufReader::new(stream_for_read);
                                for line_result in reader.lines() {
                                    match line_result {
                                        Ok(line) => {
                                            if line.trim().is_empty() {
                                                continue;
                                            }
                                            match serde_json::from_str::<Command>(&line) {
                                                Ok(cmd) => {
                                                    tracing::debug!(
                                                        "Received command from {}: {:?}",
                                                        peer_addr_clone,
                                                        cmd
                                                    );
                                                    if let Err(e) = tx.send(cmd) {
                                                        tracing::error!(
                                                            "Failed to route command to control loop: {}",
                                                            e
                                                        );
                                                    }
                                                }
                                                Err(e) => {
                                                    tracing::warn!(
                                                        "Failed to parse JSON command from {}: {} - Raw payload: {}",
                                                        peer_addr_clone,
                                                        e,
                                                        line
                                                    );
                                                }
                                            }
                                        }
                                        Err(e) => {
                                            tracing::debug!(
                                                "Client {} disconnected: {}",
                                                peer_addr_clone,
                                                e
                                            );
                                            break;
                                        }
                                    }
                                }

                                if let Ok(mut guard) = clients_for_disconnect.lock() {
                                    let before = guard.len();
                                    guard.retain(|s| {
                                        s.peer_addr()
                                            .map(|a| a.to_string())
                                            .unwrap_or_default()
                                            != peer_addr_clone
                                    });
                                    let removed = before - guard.len();
                                    if removed > 0 {
                                        tracing::debug!(
                                            "Proactively removed {} dead client(s) for {}",
                                            removed,
                                            peer_addr_clone
                                        );
                                    }
                                }

                                tracing::debug!(
                                    "IPC client {} read loop terminated",
                                    peer_addr_clone
                                );
                            })
                            .expect("failed to spawn IPC client thread");

                        client_threads.push(Client {
                            stream,
                            thread: handle,
                        });
                    }
                    Err(e) => {
                        tracing::error!("Error receiving IPC connection: {}", e);
                    }
                }
            }
        })
        .expect("failed to spawn IPC listener thread");
}
