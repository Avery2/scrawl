use std::io::{Cursor, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use serde::Deserialize;

use crate::model::{TranscriptData, TranscriptSegment, WordTiming};

const SAMPLE_RATE: u32 = 16_000;
const SPEECH_RMS: f64 = 0.012;
const WINDOW_MS: u64 = 50;
const MERGE_SILENCE_MS: u64 = 350;

#[derive(Clone, Debug)]
pub struct RecordedAudio {
    pub sample_count: usize,
    pub duration_ms: u64,
    pub speech_islands: Vec<(u64, u64)>,
    pub wav_bytes: Vec<u8>,
}

pub struct AudioRecorder {
    worker: Option<std::thread::JoinHandle<Result<()>>>,
    stop_sender: Option<std::sync::mpsc::Sender<()>>,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
}

impl AudioRecorder {
    pub fn start() -> Result<Self> {
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
        Ok(Self {
            worker: Some(worker),
            stop_sender: Some(stop_sender),
            samples,
            error,
        })
    }

    fn open_stream(
        samples: Arc<Mutex<Vec<f32>>>,
        error: Arc<Mutex<Option<String>>>,
    ) -> Result<cpal::Stream> {
        let device = cpal::default_host()
            .default_input_device()
            .context("No microphone is available")?;
        let supported = device.default_input_config()?;
        let config = supported.config();
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
        if let Some(error) = self
            .error
            .lock()
            .map_err(|_| anyhow!("Microphone error lock poisoned"))?
            .take()
        {
            bail!("Microphone capture failed: {error}");
        }
        let samples_16k = std::mem::take(
            &mut *self
                .samples
                .lock()
                .map_err(|_| anyhow!("Microphone buffer lock poisoned"))?,
        );
        let sample_count = samples_16k.len();
        let duration_ms = sample_count as u64 * 1_000 / SAMPLE_RATE as u64;
        let speech_islands = detect_speech_islands(&samples_16k, SAMPLE_RATE);
        let wav_bytes = encode_wav_16k(&samples_16k)?;
        Ok(RecordedAudio {
            sample_count,
            duration_ms,
            speech_islands,
            wav_bytes,
        })
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        let _ = self.join_worker();
    }
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
    let samples: Vec<f32> = reader
        .into_samples::<i16>()
        .map(|s| s.map(|v| f32::from(v) / 32_768.0))
        .collect::<std::result::Result<_, _>>()?;
    Ok(RecordedAudio {
        sample_count: samples.len(),
        duration_ms: samples.len() as u64 * 1_000 / u64::from(SAMPLE_RATE),
        speech_islands: detect_speech_islands(&samples, SAMPLE_RATE),
        wav_bytes,
    })
}

/// Energy-based speech estimates, quantized to 50 ms. These are not ASR timestamps.
pub fn detect_speech_islands(samples_16k: &[f32], sample_rate: u32) -> Vec<(u64, u64)> {
    if sample_rate == 0 {
        return Vec::new();
    }
    let window = (u64::from(sample_rate) * WINDOW_MS / 1_000).max(1) as usize;
    let mut islands: Vec<(u64, u64)> = Vec::new();
    for (index, chunk) in samples_16k.chunks(window).enumerate() {
        let rms = (chunk
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
            .sqrt();
        if rms <= SPEECH_RMS {
            continue;
        }
        let start = (index * window) as u64 * 1_000 / u64::from(sample_rate);
        let end = ((index * window + chunk.len()) as u64 * 1_000) / u64::from(sample_rate);
        if let Some(last) = islands.last_mut() {
            if start.saturating_sub(last.1) < MERGE_SILENCE_MS {
                last.1 = end;
                continue;
            }
        }
        islands.push((start, end));
    }
    islands
}

pub fn encode_wav_16k(samples_16k: &[f32]) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::new());
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
        for &sample in samples_16k {
            let sample = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            writer.write_sample((sample * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16)?;
        }
        writer.finalize()?;
    }
    Ok(bytes.into_inner())
}

fn clause_end(word: &str) -> bool {
    word.trim_end_matches(['"', '\'', '”', '’', ')', ']'])
        .ends_with(['.', '!', '?', ';', ':', ','])
}

/// Approximate alignment only: distribute text over measured speech, never silence.
pub fn align_transcript_to_islands(
    text: &str,
    duration_ms: u64,
    islands: &[(u64, u64)],
) -> (Vec<TranscriptSegment>, Vec<WordTiming>) {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut windows = islands.to_vec();
    windows.sort_unstable();
    let mut end = 0;
    windows = windows
        .into_iter()
        .filter_map(|(a, b)| {
            let a = a.max(end).min(duration_ms);
            let b = b.min(duration_ms);
            if a >= b {
                return None;
            }
            end = b;
            Some((a, b))
        })
        .collect();
    if windows.is_empty() {
        windows.push((0, duration_ms));
    }

    let mut segments = Vec::new();
    let mut words: Vec<WordTiming> = Vec::new();
    let mut offset = 0;
    for (index, &(start, end)) in windows.iter().enumerate() {
        let remaining = tokens.len() - offset;
        if remaining == 0 {
            break;
        }
        let later = (windows.len() - index - 1).min(remaining - 1);
        let total: u128 = windows[index..]
            .iter()
            .map(|(a, b)| u128::from(b - a))
            .sum();
        let ideal = if total == 0 {
            remaining
        } else {
            ((remaining as u128 * u128::from(end - start) + total / 2) / total) as usize
        }
        .clamp(1, remaining - later);
        let count = if index + 1 == windows.len() {
            remaining
        } else {
            // Prefer a nearby clause boundary to cutting a sentence in two.
            (1..=remaining - later)
                .filter(|&n| {
                    clause_end(tokens[offset + n - 1]) && n.abs_diff(ideal) <= (ideal / 2).max(1)
                })
                .min_by_key(|n| n.abs_diff(ideal))
                .unwrap_or(ideal)
        };
        let group = &tokens[offset..offset + count];
        let total_weight: usize = group.iter().map(|word| word.chars().count().max(1)).sum();
        let mut weight = 0;
        let mut segment_start = words.len();
        for (word_index, &word) in group.iter().enumerate() {
            let t0 =
                start + (u128::from(end - start) * weight as u128 / total_weight as u128) as u64;
            weight += word.chars().count().max(1);
            let t1 =
                start + (u128::from(end - start) * weight as u128 / total_weight as u128) as u64;
            words.push(WordTiming {
                t0,
                t1,
                text: word.to_owned(),
            });
            if clause_end(word) || word_index + 1 == count {
                let phrase = &words[segment_start..];
                segments.push(TranscriptSegment {
                    t0: phrase[0].t0,
                    t1,
                    text: phrase
                        .iter()
                        .map(|word| word.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                });
                segment_start = words.len();
            }
        }
        offset += count;
    }
    (segments, words)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Discovery {
    port: u16,
    token: String,
    api_version: String,
}

/// Missing/off microphones and unavailable Hex never prevent saving a bundle.
pub fn transcribe_with_hex(audio: Option<&RecordedAudio>) -> TranscriptData {
    let Some(audio) = audio.filter(|audio| audio.sample_count > 0) else {
        return TranscriptData {
            source: "none".into(),
            ..Default::default()
        };
    };
    match transcribe(audio) {
        Ok(transcript) => transcript,
        Err(_) => TranscriptData {
            source: "offline".into(),
            ..Default::default()
        },
    }
}

fn transcribe(audio: &RecordedAudio) -> Result<TranscriptData> {
    let path = dirs::home_dir()
        .context("Home directory unavailable")?
        .join("Library/Application Support/voice-control/local-api.json");
    let discovery: Discovery = serde_json::from_slice(&std::fs::read(path)?)?;
    ensure!(
        matches!(discovery.api_version.as_str(), "1" | "2"),
        "Unsupported Hex API version"
    );
    let mut model = "parakeet_unified_en".to_owned();
    let (mut status, mut response) = request(
        &discovery,
        "POST",
        &format!("/transcriptions?model={model}&language=en"),
        &audio.wav_bytes,
    )?;
    if status == 409
        && response
            .get("code")
            .or_else(|| response.get("error"))
            .and_then(|v| v.as_str())
            == Some("model-not-ready")
    {
        let (models_status, models) = request(&discovery, "GET", "/models", &[])?;
        ensure!(models_status == 200, "Hex model discovery failed");
        let models = models
            .as_array()
            .or_else(|| models.get("models").and_then(|v| v.as_array()))
            .context("Invalid Hex model list")?;
        model = models
            .iter()
            .find_map(|entry| {
                let id = entry.get("id")?.as_str()?;
                (matches!(id, "parakeet_v3" | "apple_speech")
                    && entry.get("installed")?.as_bool()?
                    && entry.get("verified")?.as_bool()?)
                .then(|| id.to_owned())
            })
            .context("No prepared Hex fallback model")?;
        (status, response) = request(
            &discovery,
            "POST",
            &format!("/transcriptions?model={model}&language=en"),
            &audio.wav_bytes,
        )?;
    }
    ensure!(status == 200, "Hex transcription failed (HTTP {status})");
    transcript_response(response, model, audio)
}

fn transcript_response(
    response: serde_json::Value,
    model: String,
    audio: &RecordedAudio,
) -> Result<TranscriptData> {
    let mut text = response
        .get("transcript")
        .or_else(|| response.get("text"))
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .to_owned();
    let mut segments: Vec<TranscriptSegment> = timings(&response, "segments")?
        .into_iter()
        .map(|(t0, t1, text)| TranscriptSegment { t0, t1, text })
        .collect();
    let mut words: Vec<WordTiming> = timings(&response, "words")?
        .into_iter()
        .map(|(t0, t1, text)| WordTiming { t0, t1, text })
        .collect();
    if text.is_empty() {
        text = if !segments.is_empty() {
            segments
                .iter()
                .map(|s| s.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        } else {
            words
                .iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        };
    }
    let estimated = segments.is_empty() && words.is_empty();
    if estimated {
        let duration = response
            .get("durationMs")
            .and_then(|v| v.as_u64())
            .unwrap_or(audio.duration_ms);
        (segments, words) = align_transcript_to_islands(&text, duration, &audio.speech_islands);
    } else if segments.is_empty() {
        segments = words
            .iter()
            .map(|w| TranscriptSegment {
                t0: w.t0,
                t1: w.t1,
                text: w.text.clone(),
            })
            .collect();
    }
    Ok(TranscriptData {
        source: "hex".into(),
        model: Some(model),
        text,
        segments,
        words,
        estimated,
    })
}

fn timings(response: &serde_json::Value, key: &str) -> Result<Vec<(u64, u64, String)>> {
    let Some(entries) = response.get(key).filter(|v| !v.is_null()) else {
        return Ok(Vec::new());
    };
    entries
        .as_array()
        .context("Invalid Hex timing list")?
        .iter()
        .map(|entry| {
            let timestamp = |short: &str, millis: &str, seconds: &str| -> Result<u64> {
                if let Some(ms) = entry
                    .get(short)
                    .or_else(|| entry.get(millis))
                    .and_then(|v| v.as_u64())
                {
                    return Ok(ms);
                }
                let seconds = entry
                    .get(seconds)
                    .and_then(|v| v.as_f64())
                    .context("Missing Hex timing")?;
                ensure!(seconds.is_finite() && seconds >= 0.0, "Invalid Hex timing");
                Ok((seconds * 1_000.0).round() as u64)
            };
            let t0 = timestamp("t0", "startMs", "start")?;
            let t1 = timestamp("t1", "endMs", "end")?;
            ensure!(t1 >= t0, "Reversed Hex timing");
            let text = entry
                .get("text")
                .or_else(|| entry.get("word"))
                .and_then(|v| v.as_str())
                .context("Missing Hex timing text")?;
            Ok((t0, t1, text.to_owned()))
        })
        .collect()
}

fn request(
    discovery: &Discovery,
    method: &str,
    path: &str,
    body: &[u8],
) -> Result<(u16, serde_json::Value)> {
    ensure!(
        !discovery.token.contains(['\r', '\n']),
        "Invalid Hex discovery token"
    );
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, discovery.port));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
    stream.set_write_timeout(Some(Duration::from_secs(15)))?;
    stream.set_read_timeout(Some(Duration::from_secs(120)))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: audio/wav\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        discovery.port,
        discovery.token,
        body.len()
    )?;
    // A not-ready model can reject before consuming the upload. Preserve that
    // HTTP response even if writing the rest of the WAV gets a broken pipe.
    let upload = stream.write_all(body);
    // Hex serves Content-Length responses and closes each connection.
    const LIMIT: u64 = 16 * 1024 * 1024;
    let mut bytes = Vec::new();
    let read = stream.take(LIMIT + 1).read_to_end(&mut bytes);
    if bytes.is_empty() {
        read?;
    }
    ensure!(bytes.len() as u64 <= LIMIT, "Hex response too large");
    let split = bytes
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .context("Invalid Hex HTTP response")?;
    let header = std::str::from_utf8(&bytes[..split])?;
    let status = header
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .context("Missing Hex HTTP status")?
        .parse()?;
    let content_length: usize = header
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
        .context("Missing Hex response length")?
        .1
        .trim()
        .parse()?;
    ensure!(
        bytes.len() - split - 4 == content_length,
        "Incomplete Hex HTTP response"
    );
    if status == 200 {
        upload?;
    }
    Ok((status, serde_json::from_slice(&bytes[split + 4..])?))
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn stop_propagates_background_initialization_failure() {
        let recorder = AudioRecorder {
            worker: Some(std::thread::spawn(|| bail!("device unavailable"))),
            stop_sender: None,
            samples: Arc::new(Mutex::new(Vec::new())),
            error: Arc::new(Mutex::new(None)),
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
        assert_eq!(
            detect_speech_islands(&samples, 16_000),
            vec![(100, 800), (1_150, 1_300)]
        );
        assert!(detect_speech_islands(&[0.001; 800], 16_000).is_empty());
        assert!(detect_speech_islands(&[], 16_000).is_empty());
        assert!(detect_speech_islands(&[1.0], 0).is_empty());
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

    #[test]
    fn alignment_places_sentences_in_separate_speech_windows() {
        let (segments, words) = align_transcript_to_islands(
            "First sentence. Second sentence.",
            5_000,
            &[(500, 1_500), (3_000, 4_000)],
        );
        assert_eq!(
            segments,
            vec![
                TranscriptSegment {
                    t0: 500,
                    t1: 1_500,
                    text: "First sentence.".into()
                },
                TranscriptSegment {
                    t0: 3_000,
                    t1: 4_000,
                    text: "Second sentence.".into()
                },
            ]
        );
        assert_eq!(words.len(), 4);
        assert!(words[..2].iter().all(|w| w.t0 >= 500 && w.t1 <= 1_500));
        assert!(words[2..].iter().all(|w| w.t0 >= 3_000 && w.t1 <= 4_000));
        assert!(words.windows(2).all(|pair| pair[0].t1 <= pair[1].t0));
    }

    #[test]
    fn alignment_handles_unpunctuated_text_and_missing_islands() {
        let (_, words) =
            align_transcript_to_islands("one two three four", 4_000, &[(0, 1_000), (3_000, 4_000)]);
        assert_eq!(words[1].t1, 1_000);
        assert_eq!(words[2].t0, 3_000);
        let (segments, words) = align_transcript_to_islands("one two", 900, &[]);
        assert_eq!((segments[0].t0, segments[0].t1), (0, 900));
        assert_eq!(words[1].t1, 900);
        assert_eq!(align_transcript_to_islands(" ", 0, &[]), (vec![], vec![]));
    }

    #[test]
    fn authoritative_hex_timings_are_preserved() {
        let audio = RecordedAudio {
            sample_count: 32_000,
            duration_ms: 2_000,
            speech_islands: vec![(0, 2_000)],
            wav_bytes: vec![],
        };
        let result = transcript_response(
            serde_json::json!({
                "transcript": "Hello.",
                "segments": [{"t0": 200, "t1": 450, "text": "Hello."}],
                "words": [{"t0": 220, "t1": 420, "text": "Hello."}]
            }),
            "test-model".into(),
            &audio,
        )
        .unwrap();
        assert_eq!(result.segments[0].t0, 200);
        assert_eq!(result.words[0].t1, 420);
        assert_eq!(transcribe_with_hex(None).source, "none");
    }
}
