use crate::audio::constants::{FFT_CHUNK_SIZE, INTERNAL_FORMAT};

use std::{fs::File, path::Path, thread, time::Duration};

use audioadapter_buffers::direct::InterleavedSlice;
use crossbeam_channel::{Receiver, Sender};
use reqwest::header::CONTENT_TYPE;
use ringbuf::traits::Producer;
use rubato::{Fft, FixedSync, Indexing, Resampler};
use symphonia::{
    core::{
        audio::SampleBuffer,
        codecs::DecoderOptions,
        formats::FormatOptions,
        io::{MediaSource, MediaSourceStream, ReadOnlySource},
        meta::MetadataOptions,
        probe::Hint,
    },
    default::{get_codecs, get_probe},
};

#[derive(thiserror::Error, Debug)]
pub enum DecodeError {
    #[error("failed to fetch URL")]
    Http,

    #[error("failed to open file")]
    File,

    #[error("failed to probe format")]
    Probe,

    #[error("no audio track")]
    NoTrack,

    #[error("failed to initialize decoder")]
    Decoder,

    #[error("decode failed")]
    Decode,
}

type DecodeResult<T> = std::result::Result<T, DecodeError>;

fn extension_from_mime(mime: &str) -> Option<&'static str> {
    match mime.split(';').next()?.trim() {
        "audio/mp4" | "video/mp4" => Some("mp4"),
        "audio/mpeg" => Some("mp3"),
        "audio/webm" | "video/webm" => Some("webm"),
        "audio/ogg" => Some("ogg"),
        "audio/flac" => Some("flac"),
        "audio/wav" => Some("wav"),
        _ => None,
    }
}

fn build_probe_hint(path: &str, is_url: bool, content_type: Option<&str>) -> Hint {
    let mut hint = Hint::new();

    if let Some(ext) = content_type.and_then(extension_from_mime) {
        hint.with_extension(ext);
        return hint;
    }

    if is_url {
        if let Ok(url) = reqwest::Url::parse(path)
            && let Some(seg) = url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .and_then(|n| Path::new(n).extension())
                .and_then(|e| e.to_str())
        {
            hint.with_extension(seg);
        }
    } else if let Some(ext) = Path::new(path).extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    hint
}

fn open_media(path: &str) -> DecodeResult<(Box<dyn MediaSource>, Hint)> {
    let is_url = path.starts_with("http://") || path.starts_with("https://");

    if is_url {
        let resp = reqwest::blocking::Client::new()
            .get(path)
            .send()
            .map_err(|_| DecodeError::Http)?;

        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok());

        let hint = build_probe_hint(path, true, content_type);

        Ok((Box::new(ReadOnlySource::new(resp)), hint))
    } else {
        let file = File::open(path).map_err(|_| DecodeError::File)?;
        let hint = build_probe_hint(path, false, None);

        Ok((Box::new(file), hint))
    }
}

pub fn decode_thread(
    path: &str,
    mut producer: ringbuf::HeapProd<f32>,
    shutdown_rx: Receiver<()>,
    finished_tx: Sender<()>,
) -> DecodeResult<()> {
    let res = match symphonia_decode_thread(path, &mut producer, &shutdown_rx) {
        Ok(()) => Ok(()),
        Err(DecodeError::Probe) | Err(DecodeError::NoTrack) | Err(DecodeError::Decoder) => {
            tracing::warn!("Symphonia failed, falling back to FFmpeg...");
            match ffmpeg_decode_thread(path, &mut producer, &shutdown_rx) {
                Ok(()) => Ok(()),
                Err(e) => {
                    tracing::error!("FFmpeg fallback failed: {e}");
                    Err(e)
                }
            }
        }
        Err(e) => {
            tracing::error!("Decode error: {e}");
            Err(e)
        }
    };
    let _ = finished_tx.send(());
    res
}

fn symphonia_decode_thread(
    path: &str,
    producer: &mut ringbuf::HeapProd<f32>,
    shutdown_rx: &Receiver<()>,
) -> DecodeResult<()> {
    let (source, hint) = open_media(path)?;
    let mss = MediaSourceStream::new(source, Default::default());

    let probed = get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|_| DecodeError::Probe)?;
    let mut format = probed.format;

    let track = format.default_track().ok_or(DecodeError::NoTrack)?;
    let track_id = track.id;
    let mut decoder = get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|_| DecodeError::Decoder)?;

    let ch = INTERNAL_FORMAT.channels as usize;
    let target_rate = INTERNAL_FORMAT.sample_rate;

    let push_blocking = |producer: &mut ringbuf::HeapProd<f32>, mut data: &[f32]| -> bool {
        while !data.is_empty() {
            if shutdown_rx.try_recv().is_ok() {
                return false;
            }
            let pushed = producer.push_slice(data);
            data = &data[pushed..];
            if !data.is_empty() {
                thread::sleep(Duration::from_micros(100));
            }
        }
        true
    };

    let mut accumulation = Vec::<f32>::new();
    let mut current_rate = None;
    let mut resampler: Option<Fft<f32>> = None;
    let mut out = Vec::<f32>::new();

    loop {
        if shutdown_rx.try_recv().is_ok() {
            break;
        }

        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(_) => break,
        };
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(_) => return Err(DecodeError::Decode),
        };

        let spec = *decoded.spec();
        let input_ch = spec.channels.count();
        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        let samples = buf.samples();

        // Normalize channel count to `ch`
        let normalized: Vec<f32> = match (input_ch, ch) {
            (i, o) if i == o => samples.to_vec(),
            (1, 2) => samples.iter().flat_map(|&s| [s, s]).collect(),
            (i, o) if i > o => samples
                .chunks_exact(i)
                .flat_map(|f| f[..o].iter().copied())
                .collect(),
            (i, o) => samples
                .chunks_exact(i)
                .flat_map(|f| (0..o).map(move |j| if j < i { f[j] } else { 0.0 }))
                .collect(),
        };

        if spec.rate == target_rate {
            if !push_blocking(producer, &normalized) {
                return Ok(());
            }
            continue;
        }

        if current_rate != Some(spec.rate) {
            accumulation.clear();
            resampler = Some(
                Fft::<f32>::new(
                    spec.rate as usize,
                    target_rate as usize,
                    FFT_CHUNK_SIZE,
                    ch,
                    ch,
                    FixedSync::Input,
                )
                .map_err(|_| DecodeError::Decoder)?,
            );
            current_rate = Some(spec.rate);
        }

        accumulation.extend_from_slice(&normalized);

        let resampler = resampler.as_mut().unwrap();
        while accumulation.len() >= resampler.input_frames_next() * ch {
            let needed = resampler.input_frames_next() * ch;
            let chunk: Vec<f32> = accumulation.drain(..needed).collect();

            let input_frames = needed / ch;
            let input_adapter =
                InterleavedSlice::new(&chunk, ch, input_frames).map_err(|_| DecodeError::Decode)?;
            let output_frames = resampler.output_frames_max();
            out.resize(output_frames * ch, 0.0);
            let mut output_adapter = InterleavedSlice::new_mut(&mut out, ch, output_frames)
                .map_err(|_| DecodeError::Decode)?;
            let indexing = Indexing {
                input_offset: 0,
                output_offset: 0,
                active_channels_mask: None,
                partial_len: None,
            };

            let written = match resampler.process_into_buffer(
                &input_adapter,
                &mut output_adapter,
                Some(&indexing),
            ) {
                Ok((_, w)) => w,
                Err(e) => {
                    tracing::error!("Resampler process error: {e}");
                    continue;
                }
            };

            if !push_blocking(producer, &out[..written * ch]) {
                return Ok(());
            }
        }
    }

    if let Some(resampler) = resampler.as_mut() {
        if !accumulation.is_empty() {
            let needed = resampler.input_frames_next() * ch;
            accumulation.resize(needed, 0.0);

            let input_adapter = InterleavedSlice::new(&accumulation, ch, needed / ch).unwrap();
            let output_frames = resampler.output_frames_max();
            out.resize(output_frames * ch, 0.0);
            let mut output_adapter =
                InterleavedSlice::new_mut(&mut out, ch, output_frames).unwrap();
            let indexing = Indexing {
                input_offset: 0,
                output_offset: 0,
                active_channels_mask: None,
                partial_len: None,
            };

            if let Ok((_, written)) =
                resampler.process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
            {
                push_blocking(producer, &out[..written * ch]);
            }
        }
    }

    Ok(())
}

fn ffmpeg_decode_thread(
    path: &str,
    producer: &mut ringbuf::HeapProd<f32>,
    shutdown_rx: &Receiver<()>,
) -> DecodeResult<()> {
    let mut child = std::process::Command::new("ffmpeg")
        .args([
            "-v",
            "quiet",
            "-i",
            path,
            "-f",
            "f32le",
            "-ar",
            &INTERNAL_FORMAT.sample_rate.to_string(),
            "-ac",
            &INTERNAL_FORMAT.channels.to_string(),
            "-",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|_| DecodeError::Decoder)?;

    let mut stdout = child.stdout.take().unwrap();
    let mut buf = vec![0u8; 4096 * 4]; // We use a Vec and not a stack array because rust does not guarantee byte internal alignment which is required by bytemuck

    loop {
        if shutdown_rx.try_recv().is_ok() {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(());
        }

        match std::io::Read::read(&mut stdout, &mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let valid_bytes = n - (n % 4);
                let floats: &[f32] = bytemuck::cast_slice_mut(&mut buf[..valid_bytes]);

                let mut remaining_floats = floats;
                while !remaining_floats.is_empty() {
                    if shutdown_rx.try_recv().is_ok() {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Ok(());
                    }
                    let pushed = producer.push_slice(remaining_floats);
                    remaining_floats = &remaining_floats[pushed..];
                    if !remaining_floats.is_empty() {
                        thread::sleep(Duration::from_micros(100));
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(DecodeError::Decode);
            }
        }
    }

    let _ = child.wait();
    Ok(())
}
