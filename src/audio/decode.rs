use crate::audio::constants::{FFT_CHUNK_SIZE, INTERNAL_CHANNELS, INTERNAL_SAMPLE_RATE};

use std::{collections::VecDeque, fs::File, path::Path, thread, time::Duration};

use audioadapter_buffers::direct::InterleavedSlice;
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
        if let Ok(url) = reqwest::Url::parse(path) {
            if let Some(seg) = url
                .path_segments()
                .and_then(|mut s| s.next_back())
                .and_then(|n| Path::new(n).extension())
                .and_then(|e| e.to_str())
            {
                hint.with_extension(seg);
            }
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

pub fn decode_thread(path: &str, mut producer: ringbuf::HeapProd<f32>) -> DecodeResult<()> {
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

    let mut accumulation = VecDeque::<f32>::new();

    let mut current_rate = None;
    let mut resampler = None;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(_) => break,
        };

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(_) => return Err(DecodeError::Decode),
        };

        let spec = *decoded.spec();

        let input_rate = spec.rate;
        let input_channels = spec.channels.count();

        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);

        buf.copy_interleaved_ref(decoded);

        let samples = buf.samples();

        let normalized_samples = if input_channels == 1 {
            let mut stereo = Vec::with_capacity(samples.len() * 2);

            for &s in samples {
                stereo.push(s);
                stereo.push(s);
            }

            stereo
        } else {
            samples.to_vec()
        };

        if input_rate == INTERNAL_SAMPLE_RATE {
            for &sample in &normalized_samples {
                loop {
                    match producer.try_push(sample) {
                        Ok(_) => break,

                        Err(_) => {
                            thread::sleep(Duration::from_micros(100));
                        }
                    }
                }
            }

            continue;
        }

        if current_rate != Some(input_rate) {
            accumulation.clear();

            resampler = Some(
                Fft::<f32>::new(
                    input_rate as usize,
                    INTERNAL_SAMPLE_RATE as usize,
                    FFT_CHUNK_SIZE,
                    INTERNAL_CHANNELS as usize,
                    INTERNAL_CHANNELS as usize,
                    FixedSync::Input,
                )
                .unwrap(),
            );

            current_rate = Some(input_rate);
        }

        let resampler = resampler.as_mut().unwrap();

        accumulation.extend(normalized_samples);

        let needed_frames = resampler.input_frames_next();

        let needed_samples = needed_frames * INTERNAL_CHANNELS as usize;

        while accumulation.len() >= needed_samples {
            let mut chunk = Vec::<f32>::with_capacity(needed_samples);

            for _ in 0..needed_samples {
                chunk.push(accumulation.pop_front().unwrap());
            }

            let input_adapter =
                InterleavedSlice::new(&chunk, INTERNAL_CHANNELS as usize, needed_frames).unwrap();

            let output_frames = resampler.output_frames_max();

            let mut out = vec![0.0f32; output_frames * INTERNAL_CHANNELS as usize];

            let mut output_adapter =
                InterleavedSlice::new_mut(&mut out, INTERNAL_CHANNELS as usize, output_frames)
                    .unwrap();

            let indexing = Indexing {
                input_offset: 0,
                output_offset: 0,
                active_channels_mask: None,
                partial_len: None,
            };

            let (_, written_frames) = match resampler.process_into_buffer(
                &input_adapter,
                &mut output_adapter,
                Some(&indexing),
            ) {
                Ok(v) => v,

                Err(_) => continue,
            };

            let output_samples = &out[..written_frames * INTERNAL_CHANNELS as usize];

            for &sample in output_samples {
                loop {
                    match producer.try_push(sample) {
                        Ok(_) => break,

                        Err(_) => {
                            thread::sleep(Duration::from_micros(100));
                        }
                    }
                }
            }
        }
    }

    if let Some(resampler) = resampler.as_mut() {
        let needed_frames = resampler.input_frames_next();

        let needed_samples = needed_frames * INTERNAL_CHANNELS as usize;

        if !accumulation.is_empty() {
            while accumulation.len() < needed_samples {
                accumulation.push_back(0.0);
            }

            let mut chunk = Vec::<f32>::with_capacity(needed_samples);

            for _ in 0..needed_samples {
                chunk.push(accumulation.pop_front().unwrap());
            }

            let input_adapter =
                InterleavedSlice::new(&chunk, INTERNAL_CHANNELS as usize, needed_frames).unwrap();

            let output_frames = resampler.output_frames_max();

            let mut out = vec![0.0f32; output_frames * INTERNAL_CHANNELS as usize];

            let mut output_adapter =
                InterleavedSlice::new_mut(&mut out, INTERNAL_CHANNELS as usize, output_frames)
                    .unwrap();

            let indexing = Indexing {
                input_offset: 0,
                output_offset: 0,
                active_channels_mask: None,
                partial_len: None,
            };

            if let Ok((_, written_frames)) =
                resampler.process_into_buffer(&input_adapter, &mut output_adapter, Some(&indexing))
            {
                let output_samples = &out[..written_frames * INTERNAL_CHANNELS as usize];

                for &sample in output_samples {
                    loop {
                        match producer.try_push(sample) {
                            Ok(_) => break,

                            Err(_) => {
                                thread::sleep(Duration::from_micros(100));
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
