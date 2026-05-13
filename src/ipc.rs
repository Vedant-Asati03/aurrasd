use crate::api::Command;
use crossbeam_channel::Sender;
use std::io::{BufRead, BufReader};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;

pub fn start_ipc_server(cmd_tx: Sender<Command>, clients: Arc<Mutex<Vec<TcpStream>>>) {
    thread::spawn(move || {
        let listener = match TcpListener::bind("127.0.0.1:28772") {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("Failed to bind IPC server: {e}");
                return;
            }
        };
        tracing::info!("IPC server listening on 127.0.0.1:28772");

        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let peer_addr = stream
                        .peer_addr()
                        .map(|a| a.to_string())
                        .unwrap_or_else(|_| "unknown".to_string());
                    tracing::debug!("New IPC client connected from {}", peer_addr);

                    let stream_clone = match stream.try_clone() {
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

                    if let Ok(mut clients_guard) = clients.lock() {
                        clients_guard.push(stream_clone);
                    } else {
                        tracing::error!("Failed to lock clients mutex");
                        continue;
                    }

                    let tx = cmd_tx.clone();
                    thread::spawn(move || {
                        let reader = BufReader::new(stream);
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
                                                peer_addr,
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
                                                peer_addr,
                                                e,
                                                line
                                            );
                                        }
                                    }
                                }
                                Err(e) => {
                                    tracing::debug!("Client {} disconnected: {}", peer_addr, e);
                                    break;
                                }
                            }
                        }
                        tracing::debug!("IPC client {} read loop terminated", peer_addr);
                    });
                }
                Err(e) => {
                    tracing::error!("Error receiving IPC connection: {}", e);
                }
            }
        }
    });
}
