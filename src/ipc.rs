use crate::api::Command;

use std::{
    io::{BufRead, BufReader},
    sync::{Arc, Mutex},
    thread,
};

use crossbeam_channel::Sender;
use interprocess::{
    TryClone,
    local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions, Stream, prelude::*},
};

struct Client {
    stream: Stream,
    #[allow(dead_code)]
    thread: thread::JoinHandle<()>,
}

pub fn start_ipc_server(cmd_tx: Sender<Command>, clients: Arc<Mutex<Vec<Stream>>>) {
    thread::Builder::new()
        .name("ipc-listener".into())
        .stack_size(512 * 1024)
        .spawn(move || {
            let name = if GenericNamespaced::is_supported() {
                "aurrasd.sock".to_ns_name::<GenericNamespaced>().unwrap()
            } else {
                let path = std::env::temp_dir().join("aurrasd.sock");
                let _ = std::fs::remove_file(&path);
                path.to_string_lossy()
                    .into_owned()
                    .to_fs_name::<GenericFilePath>()
                    .unwrap()
            };

            let listener = match ListenerOptions::new().name(name.clone()).create_sync() {
                Ok(l) => l,
                Err(e) => {
                    tracing::error!("Failed to bind local socket: {e}");
                    return;
                }
            };
            tracing::info!("IPC server listening on local socket");

            let mut client_threads: Vec<Client> = Vec::new();
            let reap = |threads: &mut Vec<Client>| {
                threads.retain(|c| !c.thread.is_finished());
            };

            for stream_result in listener.incoming() {
                reap(&mut client_threads);

                match stream_result {
                    Ok(stream) => {
                        tracing::debug!("New local IPC client connected");

                        let stream_for_events =
                            stream.try_clone().expect("Failed to clone event stream");
                        let stream_for_read =
                            stream.try_clone().expect("Failed to clone read stream");

                        if let Ok(mut guard) = clients.lock() {
                            guard.push(stream_for_events);
                        } else {
                            tracing::error!("Failed to lock clients mutex");
                            continue;
                        }

                        let tx = cmd_tx.clone();
                        let handle = thread::Builder::new()
                            .name("ipc-client".into())
                            .stack_size(512 * 1024)
                            .spawn(move || {
                                let reader = BufReader::new(stream_for_read);
                                for line_result in reader.lines() {
                                    match line_result {
                                        Ok(line) => {
                                            if line.trim().is_empty() {
                                                continue;
                                            }
                                            if let Ok(cmd) = serde_json::from_str::<Command>(&line)
                                            {
                                                let _ = tx.send(cmd);
                                            }
                                        }
                                        Err(_) => break,
                                    }
                                }
                                tracing::debug!("IPC client read loop terminated");
                            })
                            .expect("failed to spawn IPC client thread");

                        client_threads.push(Client {
                            stream,
                            thread: handle,
                        });
                    }
                    Err(e) => tracing::error!("Error receiving IPC connection: {}", e),
                }
            }
        })
        .expect("failed to spawn IPC listener thread");
}
