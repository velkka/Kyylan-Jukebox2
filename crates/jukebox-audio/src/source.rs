//! Decoding: a file in, interleaved `f32` chunks out, with gapless trimming and
//! sample-accurate seeking. Symphonia handles every container and most codecs; Opus packets
//! come out of symphonia's Ogg reader and go through libopus.
//!
//! Errors are sentences, because they end up in the log and, after enough failures in a
//! row, in front of the admin.

use std::fs::File;
use std::path::Path;

use symphonia::core::codecs::audio::well_known::profiles::{
    CODEC_PROFILE_AAC_HE, CODEC_PROFILE_AAC_HE_V2,
};
use symphonia::core::codecs::audio::well_known::{CODEC_ID_AAC, CODEC_ID_OPUS};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Duration, Time, TimeBase, Timestamp};

use crate::mp4;

/// Largest Opus frame: 120 ms at 48 kHz.
const OPUS_MAX_FRAMES: usize = 5760;
/// Opus needs about 80 ms of pre-roll after a seek before its output settles.
const OPUS_PREROLL_SECS: f64 = 0.08;

pub type Result<T> = std::result::Result<T, String>;

pub struct Chunk {
    pub samples: Vec<f32>,
    pub rate: u32,
    pub channels: usize,
}

enum Codec {
    Symphonia(Box<dyn AudioDecoder>),
    Opus {
        decoder: opus::Decoder,
        pcm: Vec<f32>,
    },
}

pub struct Source {
    format: Box<dyn FormatReader>,
    codec: Codec,
    track_id: u32,
    time_base: Option<TimeBase>,
    rate: u32,
    channels: usize,
    is_aac: bool,
    duration: Option<f64>,
    /// Frames still to drop before output: a head the container declares but the reader
    /// doesn't trim, plus — for Opus — the gap between where a seek landed and where it was
    /// asked for.
    skip_frames: u64,
    /// After a seek, the stream frame output should resume at. Measured against each
    /// packet's own timestamp rather than where the seek landed, because some decoders
    /// (Vorbis) turn the first packet after a seek into no audio at all.
    seek_target: Option<u64>,
    /// Frames the stream's timestamps run ahead of the audio: Opus pre-skip, or the priming
    /// of an MP4 AAC file. Seeks add it on the way in and take it off on the way out.
    head: u64,
    /// Packets that failed to decode and were skipped.
    pub decode_errors: u64,
    /// Chunks decoded, so a file that yields nothing can be told from a short one.
    pub chunks: u64,
}

/// AudioSpecificConfig object type: 2 is AAC-LC, 5 HE-AAC (SBR), 29 HE-AAC v2 (PS).
fn aac_object_type(extra: &[u8]) -> Option<u8> {
    let first = *extra.first()?;
    let object_type = first >> 3;
    if object_type == 31 {
        let second = *extra.get(1)?;
        Some(32 + (((first & 0x07) << 3) | (second >> 5)))
    } else {
        Some(object_type)
    }
}

impl Source {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path).map_err(|e| format!("the file can't be opened: {e}"))?;
        let stream = MediaSourceStream::new(Box::new(file), Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                stream,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| format!("not a playable audio file ({e})"))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or("the file has no audio in it")?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or("the file's audio format isn't recognised")?
            .clone();
        let (track_id, time_base, frames) = (track.id, track.time_base, track.num_frames);
        let delay = track.delay.unwrap_or(0);
        let rate = params
            .sample_rate
            .ok_or("the file doesn't say its sample rate")?;
        let channels = params.channels.as_ref().map_or(2, |c| c.count());

        // HE-AAC doesn't fail: symphonia decodes only its core layer — half the bandwidth,
        // and mono for HE-AAC v2 — without an error. Real files usually signal the extra
        // layer implicitly, so an AAC stream at 24 kHz or below is taken to be HE-AAC.
        let mut head = 0;
        if params.codec == CODEC_ID_AAC {
            let object_type = params.extra_data.as_deref().and_then(aac_object_type);
            let he = params
                .profile
                .is_some_and(|p| p == CODEC_PROFILE_AAC_HE || p == CODEC_PROFILE_AAC_HE_V2)
                || matches!(object_type, Some(5 | 29))
                || rate <= 24_000;
            if he {
                return Err(
                    "HE-AAC isn't supported: it would play muffled, at half its bandwidth".into(),
                );
            }
            head = mp4::priming_frames(path, rate).unwrap_or(0);
        }

        let codec = if params.codec == CODEC_ID_OPUS {
            let layout = match channels {
                1 => opus::Channels::Mono,
                2 => opus::Channels::Stereo,
                n => return Err(format!("{n}-channel Opus isn't supported")),
            };
            head = u64::from(delay);
            Codec::Opus {
                decoder: opus::Decoder::new(48_000, layout)
                    .map_err(|e| format!("the Opus decoder couldn't start: {e}"))?,
                pcm: vec![0.0; OPUS_MAX_FRAMES * channels],
            }
        } else {
            let decoder = symphonia::default::get_codecs()
                .make_audio_decoder(&params, &AudioDecoderOptions::default())
                .map_err(|e| format!("the audio format can't be decoded ({e})"))?;
            Codec::Symphonia(decoder)
        };

        Ok(Source {
            format,
            codec,
            track_id,
            time_base,
            rate,
            channels,
            duration: frames.map(|n| n.saturating_sub(head) as f64 / f64::from(rate)),
            skip_frames: head,
            seek_target: None,
            head,
            is_aac: params.codec == CODEC_ID_AAC,
            decode_errors: 0,
            chunks: 0,
        })
    }

    /// Seconds, when the file says.
    pub fn duration(&self) -> Option<f64> {
        self.duration
    }

    fn frames_of(&self, duration: Duration) -> u64 {
        if duration.is_zero() {
            return 0;
        }
        match self.time_base.and_then(|tb| tb.calc_duration(duration)) {
            Some(t) => (t.as_secs_f64() * f64::from(self.rate)).round() as u64,
            None => duration.get(),
        }
    }

    fn frames_of_timestamp(&self, ts: Timestamp) -> u64 {
        self.secs_of(ts).map_or(0, |secs| {
            (secs * f64::from(self.rate)).round().max(0.0) as u64
        })
    }

    fn secs_of(&self, ts: Timestamp) -> Option<f64> {
        self.time_base
            .and_then(|tb| tb.calc_time(ts))
            .map(|t| t.as_secs_f64())
    }

    /// The next chunk of audio, or `None` at the end.
    pub fn next_chunk(&mut self) -> Result<Option<Chunk>> {
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(packet)) => packet,
                Ok(None) => return Ok(None),
                Err(SymphoniaError::IoError(e))
                    if e.kind() == std::io::ErrorKind::UnexpectedEof =>
                {
                    return Ok(None)
                }
                Err(e) => return Err(format!("reading the file failed partway: {e}")),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let trim_start = self.frames_of(packet.trim_start);
            let trim_end = self.frames_of(packet.trim_end);

            let (mut samples, rate, channels) = match &mut self.codec {
                Codec::Symphonia(decoder) => match decoder.decode(&packet) {
                    Ok(buf) => {
                        // HE-AAC signalled explicitly passes the checks at open, but its
                        // core layer decodes at half the rate the file declares.
                        if self.is_aac && buf.spec().rate() < self.rate {
                            return Err("HE-AAC isn't supported: it would play muffled, at half its bandwidth".into());
                        }
                        let mut out = vec![0.0f32; buf.samples_interleaved()];
                        buf.copy_to_slice_interleaved(&mut out);
                        (out, buf.spec().rate(), buf.spec().channels().count())
                    }
                    // A damaged packet: skip it, as players do.
                    Err(SymphoniaError::DecodeError(_)) => {
                        self.decode_errors += 1;
                        continue;
                    }
                    Err(e) => return Err(format!("decoding failed partway: {e}")),
                },
                Codec::Opus { decoder, pcm } => {
                    match decoder.decode_float(&packet.data, pcm, false) {
                        Ok(frames) => (
                            pcm[..frames * self.channels].to_vec(),
                            48_000,
                            self.channels,
                        ),
                        Err(_) => {
                            self.decode_errors += 1;
                            continue;
                        }
                    }
                }
            };
            if channels == 0 {
                continue;
            }

            // Gapless: drop the encoder delay and padding the packet carries.
            let frames = (samples.len() / channels) as u64;
            let end = frames.saturating_sub(trim_end);
            let start = trim_start.min(end);
            samples.truncate(end as usize * channels);
            samples.drain(..start as usize * channels);

            // After a seek, drop whatever this packet holds from before the target.
            if let Some(target) = self.seek_target {
                let packet_start = self.frames_of_timestamp(packet.pts) + start;
                let have = (samples.len() / channels) as u64;
                let drop = target.saturating_sub(packet_start).min(have);
                samples.drain(..drop as usize * channels);
                if !samples.is_empty() {
                    self.seek_target = None;
                }
            }
            // Then whatever else is owed: the declared head, or the tail of an Opus seek.
            if self.skip_frames > 0 {
                let have = (samples.len() / channels) as u64;
                let drop = self.skip_frames.min(have);
                samples.drain(..drop as usize * channels);
                self.skip_frames -= drop;
            }
            if samples.is_empty() {
                continue;
            }
            self.chunks += 1;
            return Ok(Some(Chunk {
                samples,
                rate,
                channels,
            }));
        }
    }

    /// Seeks so the next frame out is at `secs`, and returns where playback now starts.
    pub fn seek(&mut self, secs: f64) -> Result<f64> {
        let secs = secs.max(0.0);
        let is_opus = matches!(self.codec, Codec::Opus { .. });
        let rate = f64::from(self.rate);
        let head_secs = self.head as f64 / rate;
        // Opus: land early and decode through a short pre-roll so the output has settled.
        let target = if is_opus {
            (secs - OPUS_PREROLL_SECS).max(0.0)
        } else {
            secs
        } + head_secs;
        let time =
            Time::try_from_secs_f64(target).ok_or_else(|| format!("can't seek to {secs} s"))?;
        let seeked = self
            .format
            .seek(
                SeekMode::Accurate,
                SeekTo::Time {
                    time,
                    track_id: Some(self.track_id),
                },
            )
            .map_err(|e| format!("seeking failed: {e}"))?;
        match &mut self.codec {
            Codec::Symphonia(decoder) => decoder.reset(),
            Codec::Opus { decoder, .. } => decoder
                .reset_state()
                .map_err(|e| format!("the Opus decoder couldn't reset: {e}"))?,
        }
        // Both in stream time, which runs `head` ahead of the audio. Accurate seeks land at
        // or before the request; the frames in between are dropped as they're decoded.
        let actual = self.secs_of(seeked.actual_ts).unwrap_or(target);
        let wanted = secs + head_secs;
        if is_opus {
            let owed = ((wanted - actual).max(0.0) * rate).round() as u64;
            self.skip_frames = owed;
        } else {
            self.skip_frames = 0;
            self.seek_target = Some((wanted * rate).round() as u64);
        }
        Ok(actual.max(wanted) - head_secs)
    }
}
