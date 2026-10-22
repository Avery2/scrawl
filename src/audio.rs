use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const SAMPLE_RATE: u32 = 16_000;
/// How often the narration on disk catches up with the microphone, and how often it is flushed
/// past the page cache: a crash costs at most this much.
const DURABLE_DRAIN_EVERY: std::time::Duration = std::time::Duration::from_millis(250);
const DURABLE_SYNC_EVERY: std::time::Duration = std::time::Duration::from_secs(1);
const WAV_HEADER_LEN: u64 = 44;

#[derive(Clone, Debug)]
pub struct RecordedAudio {
    pub sample_count: usize,
    pub duration_ms: u64,
    pub wav_bytes: Vec<u8>,
    /// The transcription that ran while recording, if one did (`live_transcribe`).
    pub live: crate::live_transcribe::Slot,
}

/// The recording's narration couldn't be written beside its video. `finalize_capture_bundle`
/// fails with this so the island can say the audio was lost rather than "Copied", and the
/// journal it leaves behind still lets `scrawl recover` build the video and the ink.
#[derive(Debug)]
pub struct AudioNotSaved;

impl std::fmt::Display for AudioNotSaved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the recording's audio couldn't be saved")
    }
}

impl std::error::Error for AudioNotSaved {}

pub struct AudioRecorder {
    worker: Option<std::thread::JoinHandle<Result<()>>>,
    stop_sender: Option<std::sync::mpsc::Sender<()>>,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
    live: Option<crate::live_transcribe::Live>,
    /// The narration, written to the bundle while it's recorded (`bundle_dir/audio.wav`).
    durable: Option<DurableFeed>,
}

/// Keeps `audio.wav` in the bundle as the microphone fills it, so a capture that never reaches
/// `stop()` still has its words: a force quit, a crash, or a launcher that restarts the app.
///
/// The header goes down first with placeholder sizes and is patched by `finish`; if nothing ever
/// patches it, `repair_wav_header` recomputes the sizes from the file's length. Samples are
/// drained from the shared buffer on their own thread, so the audio callback never writes.
struct DurableFeed {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl DurableFeed {
    fn start(samples: Arc<Mutex<Vec<f32>>>, wav: DurableWav) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let handle = std::thread::Builder::new()
            .name("scrawl-audio-durable".into())
            .spawn(move || {
                let mut wav = wav;
                let mut written = 0_usize;
                while !worker_stop.load(std::sync::atomic::Ordering::Acquire) {
                    std::thread::sleep(DURABLE_DRAIN_EVERY);
                    written = drain_samples(&samples, written, &mut wav);
                }
                // The stream is already stopped when `stop` sets this, so this drains the rest.
                drain_samples(&samples, written, &mut wav);
                if let Err(error) = wav.finish() {
                    crate::capture::scrawl_log(format!(
                        "Microphone: finishing audio.wav failed: {error:#}"
                    ));
                }
            })
            .ok();
        Self { stop, handle }
    }

    /// Stops the drainer and waits for it to patch the header.
    fn finish(mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Writes the samples `written` also holds that are new since `written`, and returns the new count.
fn drain_samples(samples: &Mutex<Vec<f32>>, written: usize, wav: &mut DurableWav) -> usize {
    let Ok(buffer) = samples.lock() else {
        return written;
    };
    if buffer.len() <= written {
        return written;
    }
    match wav.append(&buffer[written..]) {
        Ok(()) => buffer.len(),
        // A full disk must not stop the recording: the words are still in memory, and the build
        // at the end reports a WAV it couldn't write (the Failed island).
        Err(error) => {
            crate::capture::scrawl_log(format!("Microphone: audio.wav write failed: {error:#}"));
            buffer.len()
        }
    }
}

impl AudioRecorder {
    /// Records the microphone, writing `bundle_dir/audio.wav` as it goes when a bundle folder is
    /// given (the app always does; `record` from the CLI does not).
    pub fn start(bundle_dir: Option<&std::path::Path>) -> Result<Self> {
        let started = std::time::Instant::now();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let error = Arc::new(Mutex::new(None));
        let (stop_sender, stop_receiver) = std::sync::mpsc::channel();
        let worker_samples = Arc::clone(&samples);
        let worker_error = Arc::clone(&error);
        let worker = std::thread::Builder::new()
            .name("scrawl-audio".into())
            .spawn(move || {
                let stream =
                    Self::open_stream(worker_samples, worker_error).inspect_err(|error| {
                        crate::capture::scrawl_log(format!("Microphone startup failed: {error:#}"));
                    })?;
                crate::ui::start_metric("audio_ready_ms", started);
                // Stream never crosses threads. Disconnect also stops abandoned recorders.
                let _ = stop_receiver.recv();
                drop(stream);
                Ok(())
            })
            .context("Could not spawn microphone worker")?;
        let live = crate::live_transcribe::wanted()
            .then(|| crate::live_transcribe::Live::start(Arc::clone(&samples)))
            .and_then(|live| {
                live.inspect_err(|error| {
                    crate::capture::scrawl_log(format!("Live transcription: {error:#}"));
                })
                .ok()
            });
        let durable = bundle_dir.and_then(|dir| {
            let path = dir.join("audio.wav");
            match DurableWav::create(&path) {
                Ok(wav) => Some(DurableFeed::start(Arc::clone(&samples), wav)),
                Err(error) => {
                    crate::capture::scrawl_log(format!(
                        "Microphone: audio.wav can't be written as it records: {error:#}"
                    ));
                    None
                }
            }
        });
        Ok(Self {
            worker: Some(worker),
            stop_sender: Some(stop_sender),
            samples,
            error,
            live,
            durable,
        })
    }

    /// The chosen microphone (`mic`), else the macOS default. A chosen device that is missing or
    /// won't open never stops the recording.
    fn open_stream(
        samples: Arc<Mutex<Vec<f32>>>,
        error: Arc<Mutex<Option<String>>>,
    ) -> Result<cpal::Stream> {
        let choice = crate::mic::choice();
        if let Some(choice) = &choice {
            if let Some(device) = crate::mic::device(&choice.id) {
                let input = crate::mic::Input {
                    id: choice.id.clone(),
                    name: device_name(&device).unwrap_or_else(|| choice.name.clone()),
                };
                crate::mic::note_pick(&crate::mic::Pick::Chosen(&input));
                match Self::open_device(&device, samples.clone(), error.clone()) {
                    Ok(stream) => {
                        crate::capture::scrawl_log(format!("Microphone: {}", input.name));
                        return Ok(stream);
                    }
                    Err(e) => crate::capture::scrawl_log(format!(
                        "Microphone: could not open {} ({e:#}); using the default",
                        input.name
                    )),
                }
            } else {
                let default = cpal::default_host()
                    .default_input_device()
                    .and_then(|device| {
                        Some(crate::mic::Input {
                            id: device.id().ok()?.id().to_owned(),
                            name: device_name(&device)?,
                        })
                    });
                let inputs = crate::mic::Inputs {
                    default: default.as_ref().map(|d| d.id.clone()),
                    devices: default.into_iter().collect(),
                };
                crate::mic::note_pick(&crate::mic::Pick::new(Some(choice), &inputs));
            }
        }
        let device = cpal::default_host()
            .default_input_device()
            .context("No microphone is available")?;
        let stream = Self::open_device(&device, samples, error)?;
        crate::capture::scrawl_log(format!(
            "Microphone: {} (default)",
            device_name(&device).unwrap_or_default()
        ));
        Ok(stream)
    }

    fn open_device(
        device: &cpal::Device,
        samples: Arc<Mutex<Vec<f32>>>,
        error: Arc<Mutex<Option<String>>>,
    ) -> Result<cpal::Stream> {
        let supported = device.default_input_config()?;
        let config = supported.config();
        crate::capture::scrawl_log(format!(
            "Microphone: {} Hz, {} ch, {:?}",
            config.sample_rate,
            config.channels,
            supported.sample_format()
        ));
        ensure!(
            config.channels > 0 && config.sample_rate > 0,
            "Invalid microphone format"
        );
        macro_rules! stream {
            ($ty:ty) => {
                input_stream::<$ty>(&device, config, samples.clone(), error.clone())?
            };
        }
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => stream!(f32),
            cpal::SampleFormat::F64 => stream!(f64),
            cpal::SampleFormat::I8 => stream!(i8),
            cpal::SampleFormat::I16 => stream!(i16),
            cpal::SampleFormat::I24 => stream!(cpal::I24),
            cpal::SampleFormat::I32 => stream!(i32),
            cpal::SampleFormat::I64 => stream!(i64),
            cpal::SampleFormat::U8 => stream!(u8),
            cpal::SampleFormat::U16 => stream!(u16),
            cpal::SampleFormat::U24 => stream!(cpal::U24),
            cpal::SampleFormat::U32 => stream!(u32),
            cpal::SampleFormat::U64 => stream!(u64),
            format => bail!("Unsupported microphone format: {format}"),
        };
        stream.play().context("Could not start microphone")?;
        Ok(stream)
    }

    fn join_worker(&mut self) -> Result<()> {
        self.stop_sender.take();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow!("Microphone worker panicked"))??;
        }
        Ok(())
    }

    pub fn stop(mut self) -> Result<RecordedAudio> {
        // Joining drops the stream and stops its callback before taking the buffer,
        // including a stop requested while the device was still being initialized.
        self.join_worker()?;
        // The stream can't add samples any more, so the drainer writes the rest and patches the
        // header of the WAV that's already in the bundle.
        if let Some(durable) = self.durable.take() {
            durable.finish();
        }
        let error = self
            .error
            .lock()
            .map_err(|_| anyhow!("Microphone error lock poisoned"))?
            .take();
        let samples_16k = std::mem::take(
            &mut *self
                .samples
                .lock()
                .map_err(|_| anyhow!("Microphone buffer lock poisoned"))?,
        );
        // An overload, or a device unplugged partway, keeps what was heard before it.
        if let Some(error) = error {
            if samples_16k.is_empty() {
                bail!("Microphone capture failed: {error}");
            }
            crate::capture::scrawl_log(format!("Microphone: {error}; keeping what it heard"));
        }
        let sample_count = samples_16k.len();
        let duration_ms = sample_count as u64 * 1_000 / SAMPLE_RATE as u64;
        let wav_bytes = encode_wav_16k(&samples_16k)?;
        Ok(RecordedAudio {
            sample_count,
            duration_ms,
            wav_bytes,
            live: crate::live_transcribe::Slot::new(self.live.take()),
        })
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        let _ = self.join_worker();
    }
}

fn device_name(device: &cpal::Device) -> Option<String> {
    Some(device.description().ok()?.name().to_owned())
}

fn input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    let rate = config.sample_rate;
    let mut phase = 0_u32;
    let mut sum = 0.0_f64;
    Ok(device.build_input_stream::<T, _, _>(
        config,
        move |data, _| {
            let Ok(mut output) = samples.lock() else {
                return;
            };
            for frame in data.chunks_exact(channels) {
                let mono = frame
                    .iter()
                    .map(|sample| sample.to_sample::<f32>() as f64)
                    .sum::<f64>()
                    / channels as f64;
                let mono = if mono.is_finite() { mono } else { 0.0 };
                // Area averaging gives a continuous-phase downmix/decimator,
                // including non-integer ratios such as 44.1 kHz -> 16 kHz.
                let mut remaining = SAMPLE_RATE;
                while remaining > 0 {
                    let weight = remaining.min(rate - phase);
                    sum += mono * weight as f64;
                    phase += weight;
                    remaining -= weight;
                    if phase == rate {
                        output.push((sum / rate as f64) as f32);
                        phase = 0;
                        sum = 0.0;
                    }
                }
            }
        },
        move |err| {
            // A buffer under- or overrun drops a few samples; the recording goes on.
            if err.kind() == cpal::ErrorKind::Xrun {
                return;
            }
            if let Ok(mut error) = error.lock() {
                *error = Some(err.to_string());
            }
        },
        None,
    )?)
}

/// Reads a 16 kHz mono WAV (as written by `scrawl-helper audio`) back into a recording.
pub fn recorded_audio_from_wav(wav_bytes: Vec<u8>) -> Result<RecordedAudio> {
    let reader = hound::WavReader::new(Cursor::new(&wav_bytes))?;
    let spec = reader.spec();
    ensure!(
        spec.sample_rate == SAMPLE_RATE && spec.channels == 1 && spec.bits_per_sample == 16,
        "Expected 16 kHz mono 16-bit audio"
    );
    let sample_count = reader.duration() as usize;
    Ok(RecordedAudio {
        sample_count,
        duration_ms: sample_count as u64 * 1_000 / u64::from(SAMPLE_RATE),
        wav_bytes,
        live: Default::default(),
    })
}

/// Decodes a 16 kHz mono 16-bit WAV (this module's own format) into samples.
pub fn decode_wav_16k(wav_bytes: &[u8]) -> Result<Vec<f32>> {
    let reader = hound::WavReader::new(Cursor::new(wav_bytes))?;
    let spec = reader.spec();
    ensure!(
        spec.sample_rate == SAMPLE_RATE
            && spec.channels == 1
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "Expected 16 kHz mono 16-bit audio"
    );
    reader
        .into_samples::<i16>()
        .map(|sample| Ok(f32::from(sample?) / 32_768.0))
        .collect()
}

const ISLAND_WINDOW_MS: u64 = 50;
const ISLAND_MERGE_SILENCE_MS: u64 = 350;

/// Energy-based speech spans in milliseconds, quantized to 50 ms. These are not recognizer
/// timestamps. The speech threshold follows the recording's own noise floor (10th percentile of
/// window energy) so a quiet microphone and a noisy room both find their speech.
pub fn speech_islands(samples_16k: &[f32]) -> Vec<(u64, u64)> {
    let window = (u64::from(SAMPLE_RATE) * ISLAND_WINDOW_MS / 1_000) as usize;
    let rms: Vec<f64> = samples_16k
        .chunks(window)
        .map(|chunk| {
            (chunk
                .iter()
                .map(|&x| {
                    if x.is_finite() {
                        f64::from(x).powi(2)
                    } else {
                        0.0
                    }
                })
                .sum::<f64>()
                / chunk.len() as f64)
                .sqrt()
        })
        .collect();
    if rms.is_empty() {
        return Vec::new();
    }
    let mut sorted = rms.clone();
    sorted.sort_by(f64::total_cmp);
    let floor = sorted[sorted.len() / 10];
    let threshold = (floor * 3.0).clamp(0.005, 0.02);
    let mut islands: Vec<(u64, u64)> = Vec::new();
    for (index, &level) in rms.iter().enumerate() {
        if level <= threshold {
            continue;
        }
        let start = index as u64 * ISLAND_WINDOW_MS;
        let end = start + ISLAND_WINDOW_MS;
        match islands.last_mut() {
            Some(last) if start.saturating_sub(last.1) < ISLAND_MERGE_SILENCE_MS => last.1 = end,
            _ => islands.push((start, end)),
        }
    }
    let duration = samples_16k.len() as u64 * 1_000 / u64::from(SAMPLE_RATE);
    for island in &mut islands {
        island.1 = island.1.min(duration);
    }
    islands.retain(|&(a, b)| b > a);
    islands
}

/// One sample as `audio.wav` stores it.
pub fn pcm16(sample: f32) -> i16 {
    let sample = if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (sample * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

/// A 16 kHz mono 16-bit WAV written as the microphone fills it.
pub struct DurableWav {
    file: std::fs::File,
    data_bytes: u64,
    synced_at: std::time::Instant,
}

impl DurableWav {
    pub fn create(path: &Path) -> Result<Self> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)
            .with_context(|| format!("couldn't create {}", path.display()))?;
        file.write_all(&wav_header(0))?;
        file.sync_all()?;
        Ok(Self {
            file,
            data_bytes: 0,
            synced_at: std::time::Instant::now(),
        })
    }

    /// Appends samples, flushing past the page cache every second.
    pub fn append(&mut self, samples: &[f32]) -> Result<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for &sample in samples {
            bytes.extend_from_slice(&pcm16(sample).to_le_bytes());
        }
        self.file.write_all(&bytes)?;
        self.data_bytes += bytes.len() as u64;
        if self.synced_at.elapsed() >= DURABLE_SYNC_EVERY {
            self.file.sync_data()?;
            self.synced_at = std::time::Instant::now();
        }
        Ok(())
    }

    /// Patches the sizes in the header and syncs: the file is a complete WAV.
    pub fn finish(mut self) -> Result<()> {
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&wav_header(self.data_bytes))?;
        self.file.sync_all()?;
        Ok(())
    }
}

/// The canonical 44-byte header for 16 kHz mono 16-bit PCM, with a data length.
fn wav_header(data_bytes: u64) -> Vec<u8> {
    let data = u32::try_from(data_bytes).unwrap_or(u32::MAX & !1);
    let block_align: u16 = 2;
    let byte_rate = SAMPLE_RATE * u32::from(block_align);
    let mut header = Vec::with_capacity(WAV_HEADER_LEN as usize);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&(36_u32.saturating_add(data)).to_le_bytes());
    header.extend_from_slice(b"WAVEfmt ");
    header.extend_from_slice(&16_u32.to_le_bytes());
    header.extend_from_slice(&1_u16.to_le_bytes());
    header.extend_from_slice(&1_u16.to_le_bytes());
    header.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&block_align.to_le_bytes());
    header.extend_from_slice(&16_u16.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data.to_le_bytes());
    header
}

/// Makes a WAV that a crash left behind playable: `DurableWav` writes its sizes as placeholders
/// until `finish`, so they are recomputed from the file's length. Returns the data length.
pub fn repair_wav_header(path: &Path) -> Result<u64> {
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .with_context(|| format!("couldn't open {}", path.display()))?;
    let len = file.metadata()?.len();
    ensure!(len >= WAV_HEADER_LEN, "{} is too short to be a WAV", path.display());
    let data_bytes = len - WAV_HEADER_LEN;
    let mut header = [0_u8; WAV_HEADER_LEN as usize];
    file.read_exact(&mut header)?;
    let declared = u32::from_le_bytes([header[40], header[41], header[42], header[43]]);
    if u64::from(declared) != data_bytes {
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&wav_header(data_bytes))?;
        file.sync_all()?;
    }
    Ok(data_bytes)
}

pub fn encode_wav_16k(samples_16k: &[f32]) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::with_capacity(44 + samples_16k.len() * 2));
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: SAMPLE_RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        let mut i16_writer = writer.get_i16_writer(samples_16k.len() as u32);
        for &sample in samples_16k {
            i16_writer.write_sample(pcm16(sample));
        }
        i16_writer.flush()?;
        writer.finalize()?;
    }
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "scrawl-audio-{name}-{}-{}",
            std::process::id(),
            crate::now_epoch_ms()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("audio.wav")
    }

    #[test]
    fn a_finished_durable_wav_is_a_normal_recording() {
        let path = scratch("finished");
        let mut wav = DurableWav::create(&path).unwrap();
        wav.append(&[0.25; 16_000]).unwrap();
        wav.finish().unwrap();
        let audio = recorded_audio_from_wav(std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(audio.sample_count, 16_000);
        assert_eq!(audio.duration_ms, 1_000);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn a_crashed_durable_wav_is_repaired_from_its_length() {
        // A killed process never patches the header, so the sizes are still the placeholders.
        let path = scratch("crashed");
        let mut wav = DurableWav::create(&path).unwrap();
        wav.append(&[0.5; 32_000]).unwrap();
        std::mem::forget(wav);
        assert_eq!(repair_wav_header(&path).unwrap(), 64_000);
        let audio = recorded_audio_from_wav(std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(audio.sample_count, 32_000);
        assert_eq!(audio.duration_ms, 2_000);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_durable_feed_writes_samples_as_they_arrive() {
        // What a crash keeps: the drainer writes while the recording runs, not at stop.
        let path = scratch("feed");
        let samples = Arc::new(Mutex::new(vec![0.2_f32; 8_000]));
        let feed = DurableFeed::start(Arc::clone(&samples), DurableWav::create(&path).unwrap());
        // Read it before `finish`, exactly as a killed process would leave it.
        std::thread::sleep(DURABLE_DRAIN_EVERY * 2 + std::time::Duration::from_millis(100));
        std::mem::forget(feed);
        repair_wav_header(&path).unwrap();
        let audio = recorded_audio_from_wav(std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(audio.sample_count, 8_000);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn repairing_a_complete_wav_leaves_it_alone() {
        let path = scratch("complete");
        let bytes = encode_wav_16k(&[0.1; 8_000]).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(repair_wav_header(&path).unwrap(), 16_000);
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn audio_not_saved_survives_the_context_that_names_the_file() {
        // The island tells a full disk (AudioNotSaved) from any other build failure by
        // downcasting the error `finalize_capture_bundle` returns, which carries a context.
        let error = anyhow::Error::new(AudioNotSaved).context(
            "couldn't write /tmp/x/audio.wav: No space left on device (os error 28)",
        );
        assert!(error.downcast_ref::<AudioNotSaved>().is_some());
    }

    #[test]
    fn stop_joins_worker_before_reading_its_last_samples() {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&samples);
        let (stop_sender, stop_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            // Model a final callback flushed while the worker drops its stream.
            assert!(matches!(
                stop_receiver.recv_timeout(Duration::from_secs(2)),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ));
            output.lock().unwrap().extend([0.1, 0.2, 0.3]);
            Ok(())
        });
        let audio = AudioRecorder {
            worker: Some(worker),
            stop_sender: Some(stop_sender),
            samples,
            error: Arc::new(Mutex::new(None)),
            live: None,
            durable: None,
        }
        .stop()
        .unwrap();
        assert_eq!(audio.sample_count, 3);
        assert_eq!(
            hound::WavReader::new(Cursor::new(audio.wav_bytes))
                .unwrap()
                .duration(),
            3
        );
    }

    #[test]
    fn a_device_lost_partway_keeps_what_it_heard() {
        let recorder = |samples: Vec<f32>| AudioRecorder {
            worker: None,
            stop_sender: None,
            samples: Arc::new(Mutex::new(samples)),
            error: Arc::new(Mutex::new(Some("Device disconnected".into()))),
            live: None,
            durable: None,
        };
        assert_eq!(recorder(vec![0.1; 160]).stop().unwrap().sample_count, 160);
        assert!(recorder(Vec::new()).stop().is_err());
    }

    #[test]
    fn stop_propagates_background_initialization_failure() {
        let recorder = AudioRecorder {
            worker: Some(std::thread::spawn(|| bail!("device unavailable"))),
            stop_sender: None,
            samples: Arc::new(Mutex::new(Vec::new())),
            error: Arc::new(Mutex::new(None)),
            live: None,
            durable: None,
        };
        assert!(
            recorder
                .stop()
                .unwrap_err()
                .to_string()
                .contains("device unavailable")
        );
    }

    #[test]
    fn speech_islands_merge_only_short_silences() {
        let mut samples = vec![0.0; 32_000];
        samples[1_600..4_800].fill(0.2); // 100..300
        samples[8_800..12_800].fill(0.2); // 550..800: merge 250 ms gap
        samples[18_400..20_800].fill(0.2); // 1150..1300: keep 350 ms gap
        assert_eq!(speech_islands(&samples), vec![(100, 800), (1_150, 1_300)]);
        assert!(speech_islands(&[0.001; 800]).is_empty());
        assert!(speech_islands(&[]).is_empty());
    }

    #[test]
    fn speech_islands_adapt_to_a_noisy_floor() {
        let mut samples: Vec<f32> = (0..32_000)
            .map(|i| if i % 2 == 0 { 0.006 } else { -0.006 })
            .collect();
        samples[8_000..16_000]
            .iter_mut()
            .for_each(|s| *s = if *s > 0.0 { 0.15 } else { -0.15 });
        assert_eq!(speech_islands(&samples), vec![(500, 1_000)]);
    }

    #[test]
    fn decoded_wav_round_trips() {
        let wav = encode_wav_16k(&[0.0, 0.5, -0.5]).unwrap();
        assert_eq!(decode_wav_16k(&wav).unwrap(), vec![0.0, 0.5, -0.5]);
        assert!(decode_wav_16k(b"nope").is_err());
    }

    #[test]
    fn wav_is_mono_16k_pcm_and_clips_safely() {
        let bytes = encode_wav_16k(&[-2.0, -0.5, 0.0, 0.5, 2.0, f32::NAN]).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap(),
            vec![-32_768, -16_384, 0, 16_384, 32_767, 0]
        );
    }
}
