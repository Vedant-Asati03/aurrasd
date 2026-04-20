use std::{
    fs::File,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use anyhow::{Context, Result};
use crossbeam_channel::Sender;

use symphonia::core::io::{MediaSource, ReadOnlySource};
use symphonia::core::{
    audio::SampleBuffer, codecs::DecoderOptions, formats::FormatOptions, io::MediaSourceStream,
    meta::MetadataOptions, probe::Hint,
};
use symphonia::default::{get_codecs, get_probe};

use crate::audio::{ring::RingBuffer, types::AudioFormat};

fn open_media(path: &str, is_url: bool) -> Result<Box<dyn MediaSource>> {
    if is_url {
        let resp = reqwest::blocking::get(path).context("Failed to fetch URL")?;
        Ok(Box::new(ReadOnlySource::new(resp)))
    } else {
        Ok(Box::new(File::open(path)?))
    }
}

pub fn decode_thread(
    path: &str,
    is_url: bool,
    ring: Arc<RingBuffer>,
    fmt_tx: Sender<AudioFormat>,
    stop: Arc<AtomicBool>,
) -> Result<()> {
    let source = open_media(path, is_url)?;
    let mss = MediaSourceStream::new(source, Default::default());

    let probed = get_probe().format(
        &Hint::new(),
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;

    let mut format = probed.format;
    let track = format.default_track().context("No audio track")?;
    let track_id = track.id;

    let mut decoder = get_codecs().make(&track.codec_params, &DecoderOptions::default())?;

    let mut sent_format: Option<AudioFormat> = None;
    let mut chunk = Vec::with_capacity(4096);

    while let Ok(packet) = format.next_packet() {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }

        if packet.track_id() != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
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
            if stop.load(Ordering::Relaxed) {
                return Ok(());
            }

            chunk.push(sample);

            if chunk.len() >= 4096 {
                ring.write(&chunk);
                chunk.clear();
            }
        }
    }

    if !chunk.is_empty() {
        ring.write(&chunk);
    }

    ring.mark_finished();

    Ok(())
}
