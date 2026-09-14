//! The decoder thread: reads a [`Source`], converts it to the output's channel count and
//! sample rate, and keeps the ring buffer topped up.
//!
//! It never blocks on a full buffer — a paused or unplugged output leaves it waiting with
//! audio in hand while it keeps listening for commands — so stopping, seeking or moving to
//! another device always takes effect at once.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use jukebox_core::player::LoadId;
use rtrb::Producer;
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::output::Shared;
use crate::source::{Chunk, Source};

/// Keep decoding until this much is buffered.
const HIGH_WATER_SECS: f64 = 0.35;
/// Resampler chunk size in input frames.
const RESAMPLE_CHUNK: usize = 1024;

pub(crate) enum Command {
    /// Resume from this position, in seconds.
    Seek(f64),
    /// Feed a different output from this position.
    Retarget {
        producer: Producer<f32>,
        rate: u32,
        channels: usize,
        resume_at: f64,
    },
    /// The output is gone: hold on until there's another.
    Detach,
    Stop,
}

/// Sent back to the player's control thread.
pub(crate) struct Failure {
    pub load: LoadId,
    pub reason: String,
}

pub(crate) struct Decoder {
    tx: Sender<Command>,
    thread: Option<JoinHandle<Option<Producer<f32>>>>,
}

impl Decoder {
    pub fn start(
        source: Source,
        output: Option<(Producer<f32>, u32, usize)>,
        shared: Arc<Shared>,
        load: LoadId,
        failures: Sender<Failure>,
    ) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let thread = thread::Builder::new()
            .name("decoder".into())
            .spawn(move || {
                let mut decoding = Decoding {
                    source,
                    sink: output
                        .map(|(producer, rate, channels)| Sink::new(producer, rate, channels)),
                    shared,
                    pending: Vec::new(),
                    sent: 0,
                    finished: false,
                };
                let result = decoding.run(&rx);
                if let Err(reason) = result {
                    let _ = failures.send(Failure { load, reason });
                }
                decoding.sink.map(|sink| sink.producer)
            })
            .expect("starting the decoder thread");
        Decoder {
            tx,
            thread: Some(thread),
        }
    }

    pub fn send(&self, command: Command) {
        let _ = self.tx.send(command);
    }

    /// Stops the thread, handing back the buffer it was writing to.
    pub fn stop(mut self) -> Option<Producer<f32>> {
        let _ = self.tx.send(Command::Stop);
        self.thread.take().and_then(|t| t.join().ok()).flatten()
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
    }
}

/// An output being written to, with the conversion into its format.
struct Sink {
    producer: Producer<f32>,
    capacity: usize,
    rate: u32,
    channels: usize,
    /// The resampler, and the input rate it was built for.
    resampler: Option<(u32, Fft<f32>)>,
    fifo: Vec<f32>,
    out: Vec<f32>,
    /// Resampler output still to drop: its processing delay.
    trim_out: usize,
}

impl Sink {
    fn new(producer: Producer<f32>, rate: u32, channels: usize) -> Self {
        Sink {
            capacity: producer.slots(),
            producer,
            rate,
            channels,
            resampler: None,
            fifo: Vec::new(),
            out: Vec::new(),
            trim_out: 0,
        }
    }

    fn buffered_secs(&self) -> f64 {
        let samples = self.capacity - self.producer.slots();
        samples as f64 / f64::from(self.rate) / self.channels as f64
    }

    /// Converts a chunk, appending the result to `pending`.
    fn convert(&mut self, chunk: Chunk, pending: &mut Vec<f32>) -> Result<(), String> {
        let mapped = map_channels(&chunk.samples, chunk.channels, self.channels);
        if chunk.rate == self.rate {
            pending.extend_from_slice(&mapped);
            return Ok(());
        }
        if self
            .resampler
            .as_ref()
            .is_none_or(|(rate, _)| *rate != chunk.rate)
        {
            let resampler = Fft::<f32>::new(
                chunk.rate as usize,
                self.rate as usize,
                RESAMPLE_CHUNK,
                self.channels,
                FixedSync::Input,
            )
            .map_err(|e| format!("resampling isn't possible: {e}"))?;
            self.trim_out = resampler.output_delay();
            self.out = vec![0.0; resampler.output_frames_max() * self.channels];
            self.fifo.clear();
            self.resampler = Some((chunk.rate, resampler));
        }
        self.fifo.extend_from_slice(&mapped);
        self.resample(false, pending)
    }

    /// Runs the resampler over whole chunks of the fifo, or everything left if `tail`.
    fn resample(&mut self, tail: bool, pending: &mut Vec<f32>) -> Result<(), String> {
        let ch = self.channels;
        loop {
            let Some((_, resampler)) = self.resampler.as_mut() else {
                return Ok(());
            };
            let need = resampler.input_frames_next();
            let have = self.fifo.len() / ch;
            let partial = if have >= need {
                None
            } else if tail && have > 0 {
                Some(have)
            } else {
                return Ok(());
            };
            let input = InterleavedSlice::new(&self.fifo, ch, have).map_err(|e| e.to_string())?;
            let out_frames = self.out.len() / ch;
            let mut output = InterleavedSlice::new_mut(&mut self.out, ch, out_frames)
                .map_err(|e| e.to_string())?;
            let mut indexing = Indexing::new();
            indexing.partial_len = partial;
            let (consumed, produced) = resampler
                .process_into_buffer(&input, &mut output, Some(&indexing))
                .map_err(|e| format!("resampling failed: {e}"))?;
            let consumed = if partial.is_some() { have } else { consumed };
            self.fifo.drain(..consumed * ch);
            let skip = self.trim_out.min(produced);
            self.trim_out -= skip;
            pending.extend_from_slice(&self.out[skip * ch..produced * ch]);
            if partial.is_some() {
                return Ok(());
            }
        }
    }

    /// Flushes the resampler's last frames by feeding it silence.
    fn finish(&mut self, pending: &mut Vec<f32>) -> Result<(), String> {
        if self.resampler.is_some() {
            self.fifo
                .extend(std::iter::repeat_n(0.0, RESAMPLE_CHUNK * self.channels));
            self.resample(true, pending)?;
        }
        Ok(())
    }

    /// Forgets converter state after a jump in the audio.
    fn reset(&mut self) {
        if let Some((_, resampler)) = self.resampler.as_mut() {
            resampler.reset();
            self.trim_out = resampler.output_delay();
        }
        self.fifo.clear();
    }
}

/// Mono is copied to every channel, anything down to mono is averaged, and otherwise the
/// first channels are kept.
fn map_channels(input: &[f32], from: usize, to: usize) -> Vec<f32> {
    if from == to {
        return input.to_vec();
    }
    let mut out = Vec::with_capacity(input.len() / from * to);
    for frame in input.chunks(from) {
        match (from, to) {
            (1, _) => out.extend(std::iter::repeat_n(frame[0], to)),
            (_, 1) => out.push(frame.iter().sum::<f32>() / from as f32),
            _ => out.extend((0..to).map(|c| frame[c.min(from - 1)])),
        }
    }
    out
}

struct Decoding {
    source: Source,
    sink: Option<Sink>,
    shared: Arc<Shared>,
    /// Converted audio not yet in the ring buffer, and how much of it has gone in.
    pending: Vec<f32>,
    sent: usize,
    /// The file has been read to the end; `eof` is set once `pending` is empty too.
    finished: bool,
}

impl Decoding {
    fn run(&mut self, rx: &Receiver<Command>) -> Result<(), String> {
        loop {
            let busy = self.sink.is_some() && !self.finished && self.wants_audio();
            let wait = if busy {
                Duration::ZERO
            } else {
                Duration::from_millis(5)
            };
            match rx.recv_timeout(wait) {
                Ok(Command::Stop) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
                Ok(Command::Seek(secs)) => self.jump(secs, None)?,
                Ok(Command::Retarget {
                    producer,
                    rate,
                    channels,
                    resume_at,
                }) => self.jump(resume_at, Some(Sink::new(producer, rate, channels)))?,
                Ok(Command::Detach) => {
                    self.sink = None;
                    self.clear_pending();
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.step()?;
        }
    }

    /// Whether the buffer wants more audio than is pending.
    fn wants_audio(&self) -> bool {
        self.sink
            .as_ref()
            .is_some_and(|s| s.buffered_secs() < HIGH_WATER_SECS)
            && self.sent >= self.pending.len()
    }

    fn clear_pending(&mut self) {
        self.pending.clear();
        self.sent = 0;
    }

    /// Continues from `secs`, on a new output if one is given.
    fn jump(&mut self, secs: f64, sink: Option<Sink>) -> Result<(), String> {
        let replacing = sink.is_some();
        if let Some(sink) = sink {
            self.sink = Some(sink);
        } else if self.sink.is_some() {
            // Have the callback drop what's buffered, and give it a moment to.
            self.shared.flush.store(true, Relaxed);
            let asked = Instant::now();
            while self.shared.flush.load(Relaxed) && asked.elapsed() < Duration::from_millis(200) {
                thread::sleep(Duration::from_millis(1));
            }
        }
        let landed = self.source.seek(secs)?;
        if let Some(sink) = self.sink.as_mut() {
            if !replacing {
                sink.reset();
            }
        }
        self.clear_pending();
        self.finished = false;
        if let Some(sink) = &self.sink {
            self.shared.out_rate.store(sink.rate, Relaxed);
        }
        self.shared.base_pos_bits.store(landed.to_bits(), Relaxed);
        self.shared.frames_played.store(0, Relaxed);
        self.shared.eof.store(false, Relaxed);
        self.shared.drained.store(false, Relaxed);
        Ok(())
    }

    /// One round of work: push what's pending, then decode more if there's room.
    fn step(&mut self) -> Result<(), String> {
        let Some(sink) = self.sink.as_mut() else {
            return Ok(());
        };
        if self.sent < self.pending.len() {
            let ch = sink.channels;
            let room = sink.producer.slots();
            let n = room.min(self.pending.len() - self.sent);
            let n = n - n % ch;
            if n > 0 {
                if let Ok(chunk) = sink.producer.write_chunk_uninit(n) {
                    chunk.fill_from_iter(self.pending[self.sent..self.sent + n].iter().copied());
                }
                self.sent += n;
            }
            if self.sent < self.pending.len() {
                return Ok(());
            }
            self.pending.clear();
            self.sent = 0;
        }
        if self.finished {
            self.shared.eof.store(true, Relaxed);
            return Ok(());
        }
        if sink.buffered_secs() >= HIGH_WATER_SECS {
            return Ok(());
        }
        match self.source.next_chunk()? {
            Some(chunk) => sink.convert(chunk, &mut self.pending),
            None => {
                if self.source.chunks == 0 {
                    return Err("no audio could be decoded from the file".into());
                }
                sink.finish(&mut self.pending)?;
                self.finished = true;
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_map_down_up_and_across() {
        assert_eq!(map_channels(&[0.5, 0.25], 1, 2), vec![0.5, 0.5, 0.25, 0.25]);
        assert_eq!(map_channels(&[1.0, 0.0], 2, 1), vec![0.5]);
        assert_eq!(
            map_channels(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0], 6, 2),
            vec![1.0, 2.0]
        );
    }
}
