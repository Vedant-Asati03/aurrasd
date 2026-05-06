use std::{fs::File, path::Path};

use crossbeam_channel::Sender;
use reqwest::header::CONTENT_TYPE;

use symphonia::core::io::{MediaSource, ReadOnlySource};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
    meta::MetadataOptions, probe::Hint,
};
use symphonia::default::{get_codecs, get_probe};

use crate::audio::types::AudioFormat;

#[derive(thiserror::Error, Debug)]
pub enum DecodeError {
    #[error("failed to initialize HTTP client: {source}")]
    HttpClient {
        #[source]
        source: reqwest::Error,
    },

    #[error("failed to fetch URL `{url}`: {source}")]
    HttpRequest {
        url: String,
        #[source]
        source: reqwest::Error,
    },

    #[error("URL `{url}` returned non-success HTTP status: {status}")]
    HttpStatus {
        url: String,
        status: reqwest::StatusCode,
    },

    #[error("failed to open file `{path}`: {source}")]
    OpenFile {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to detect media format for `{target}`: {source}")]
    Probe {
        target: String,
        #[source]
        source: symphonia::core::errors::Error,
    },

    #[error("no audio track")]
    NoTrack,

    #[error("failed to initialize audio decoder: {source}")]
    DecoderInit {
        #[source]
        source: symphonia::core::errors::Error,
    },

    #[error("decode failed: {source}")]
    Decode {
        #[source]
        source: symphonia::core::errors::Error,
    },
}

type DecodeResult<T> = std::result::Result<T, DecodeError>;

enum SupportedMimeType {
    Mp4,
    Mpeg,
    Webm,
    Ogg,
    Flac,
    Wav,
    Aac,
}

impl SupportedMimeType {
    fn from_str(s: &str) -> Option<Self> {
        use SupportedMimeType::*;

        match s {
            "audio/mp4" | "video/mp4" => Some(Mp4),
            "audio/mpeg" => Some(Mpeg),
            "audio/webm" | "video/webm" => Some(Webm),
            "audio/ogg" | "application/ogg" => Some(Ogg),
            "audio/flac" => Some(Flac),
            "audio/wav" | "audio/x-wav" => Some(Wav),
            "audio/aac" => Some(Aac),
            _ => None,
        }
    }
}

fn extension_from_mime(mime: &str) -> Option<&'static str> {
    let mime_str = mime.split(';').next().unwrap_or_default().trim();

    match SupportedMimeType::from_str(mime_str) {
        Some(SupportedMimeType::Mp4) => Some("mp4"),
        Some(SupportedMimeType::Mpeg) => Some("mp3"),
        Some(SupportedMimeType::Webm) => Some("webm"),
        Some(SupportedMimeType::Ogg) => Some("ogg"),
        Some(SupportedMimeType::Flac) => Some("flac"),
        Some(SupportedMimeType::Wav) => Some("wav"),
        Some(SupportedMimeType::Aac) => Some("aac"),
        None => None,
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
            if let Some((_, mime)) = url.query_pairs().find(|(key, _)| key == "mime")
                && let Some(ext) = extension_from_mime(mime.as_ref())
            {
                hint.with_extension(ext);
                return hint;
            }

            if let Some(seg) = url
                .path_segments()
                .and_then(|mut segments| segments.next_back())
                .and_then(|name| Path::new(name).extension())
                .and_then(|ext| ext.to_str())
            {
                hint.with_extension(seg);
            }
        }
    } else if let Some(ext) = Path::new(path).extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(ext);
    }

    hint
}

fn open_media(path: &str, is_url: bool) -> DecodeResult<(Box<dyn MediaSource>, Hint)> {
    if is_url {
        let client = reqwest::blocking::Client::builder()
            .user_agent("aurrasd/0.1")
            .build()
            .map_err(|source| DecodeError::HttpClient { source })?;

        let resp = client
            .get(path)
            .send()
            .map_err(|source| DecodeError::HttpRequest {
                url: path.to_owned(),
                source,
            })?;

        let status = resp.status();
        if !status.is_success() {
            return Err(DecodeError::HttpStatus {
                url: path.to_owned(),
                status,
            });
        }

        let content_type = resp
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|v| v.to_string());
        let hint = build_probe_hint(path, true, content_type.as_deref());

        Ok((Box::new(ReadOnlySource::new(resp)), hint))
    } else {
        let file = File::open(path).map_err(|source| DecodeError::OpenFile {
            path: path.to_owned(),
            source,
        })?;
        let hint = build_probe_hint(path, false, None);
        Ok((Box::new(file), hint))
    }
}

pub fn decode_thread(
    path: &str,
    is_url: bool,
    data_tx: Sender<f32>,
    fmt_tx: Sender<AudioFormat>,
) -> DecodeResult<()> {
    let (source, hint) = open_media(path, is_url)?;
    let mss = MediaSourceStream::new(source, Default::default());

    let target = if is_url {
        format!("url: {path}")
    } else {
        format!("file: {path}")
    };

    let probed = get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|source| DecodeError::Probe {
            target: target.clone(),
            source,
        })?;

    let mut format = probed.format;
    let track = format.default_track().ok_or(DecodeError::NoTrack)?;
    let track_id = track.id;

    let mut decoder = get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|source| DecodeError::DecoderInit { source })?;

    let mut sent_format: Option<AudioFormat> = None;

    'decode: while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(source) => return Err(DecodeError::Decode { source }),
        };

        let spec = *decoded.spec();

        if sent_format.is_none() {
            let fmt = AudioFormat {
                sample_rate: spec.rate,
                channels: spec.channels.count() as u16,
            };

            let _ = fmt_tx.send(fmt.clone());
            sent_format = Some(fmt);
        }

        let mut buf = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);

        for &sample in buf.samples() {
            loop {
                match data_tx.try_send(sample) {
                    Ok(_) => break,
                    Err(crossbeam_channel::TrySendError::Full(_)) => {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(crossbeam_channel::TrySendError::Disconnected(_)) => {
                        break 'decode;
                    }
                }
            }
        }
    }

    Ok(())
}
