//! `scrawl-helper`, the Swift executable shipped beside `scrawl` (see `helper/main.swift`).
//!
//! It owns the Apple frameworks that are Swift-only or absent from the objc2 bindings Scrawl
//! uses: SpeechAnalyzer transcription, CoreMediaIO/AVFoundation iPhone capture with a live
//! mirror window, and AVFoundation movie decoding.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use crate::model::{RgbaFrame, TranscriptData, TranscriptSegment, WordTiming};

pub fn path() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("SCRAWL_HELPER") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().context("Could not locate scrawl")?;
    // Beside the app binary, or one level up from `target/*/deps` for tests.
    exe.ancestors()
        .skip(1)
        .take(2)
        .map(|dir| dir.join("scrawl-helper"))
        .find(|path| path.is_file())
        .context("scrawl-helper is missing; rebuild Scrawl")
}

/// Runs a helper command to completion and parses its JSON output.
fn run<T: for<'de> Deserialize<'de>>(args: &[&std::ffi::OsStr], timeout: Duration) -> Result<T> {
    let mut child = Command::new(path()?)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Could not start scrawl-helper")?;
    let mut stdout = child.stdout.take().context("No helper stdout")?;
    let mut stderr = child.stderr.take().context("No helper stderr")?;
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        out
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("scrawl-helper {:?} timed out", args.first());
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let out = reader.join().unwrap_or_default();
    if !status.success() {
        let mut message = String::new();
        let _ = stderr.read_to_string(&mut message);
        bail!("{}", message.trim());
    }
    serde_json::from_slice(&out).context("Invalid scrawl-helper output")
}

fn os(s: &str) -> &std::ffi::OsStr {
    std::ffi::OsStr::new(s)
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct PhoneDevice {
    pub id: String,
    pub name: String,
    pub model: String,
    pub width: u32,
    pub height: u32,
}

/// Cabled iPhones and iPads that can be mirrored.
pub fn devices(timeout: Duration) -> Result<Vec<PhoneDevice>> {
    let seconds = format!("{:.1}", timeout.as_secs_f64());
    run(
        &[os("devices"), os("--timeout"), os(&seconds)],
        timeout + Duration::from_secs(5),
    )
}

#[derive(Deserialize)]
struct Transcript {
    locale: String,
    text: String,
    segments: Vec<TranscriptSegment>,
    words: Vec<WordTiming>,
}

/// On-device SpeechAnalyzer transcription with measured word timings.
pub fn transcribe(audio: &Path) -> Result<TranscriptData> {
    let transcript: Transcript = run(
        &[os("transcribe"), audio.as_os_str()],
        Duration::from_secs(180),
    )?;
    Ok(TranscriptData {
        source: "speech-analyzer".into(),
        model: Some(format!("SpeechTranscriber {}", transcript.locale)),
        text: transcript.text,
        segments: transcript.segments,
        words: transcript.words,
        estimated: false,
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Probe {
    pub duration_ms: u64,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub audio_tracks: u32,
}

pub fn probe(movie: &Path) -> Result<Probe> {
    run(&[os("probe"), movie.as_os_str()], Duration::from_secs(30))
}

/// Mixes a movie's audio to 16 kHz mono WAV, aligned to the movie's start.
pub fn export_audio(movie: &Path, wav: &Path) -> Result<()> {
    let _: serde_json::Value = run(
        &[os("audio"), movie.as_os_str(), wav.as_os_str()],
        Duration::from_secs(120),
    )?;
    Ok(())
}

#[derive(Deserialize)]
struct FrameFile {
    t: u64,
    actual: u64,
    width: u32,
    height: u32,
    file: PathBuf,
}

pub enum FrameTimes<'a> {
    /// Nearest decodable frame every N ms, stamped with its actual time.
    Every(u64),
    /// Exactly these times.
    At(&'a [u64]),
}

/// Decodes movie frames into memory; `max_edge` 0 keeps native size.
pub fn frames(movie: &Path, times: FrameTimes, max_edge: u32) -> Result<Vec<RgbaFrame>> {
    let dir = std::env::temp_dir().join(format!("scrawl-frames-{}", crate::make_bundle_id()));
    let (flag, value, sampled) = match times {
        FrameTimes::Every(ms) => ("--every", ms.max(1).to_string(), true),
        FrameTimes::At(times) => {
            if times.is_empty() {
                return Ok(Vec::new());
            }
            let list = times.iter().map(u64::to_string).collect::<Vec<_>>();
            ("--at", list.join(","), false)
        }
    };
    let edge = max_edge.to_string();
    let result: Result<Vec<FrameFile>> = run(
        &[
            os("frames"),
            movie.as_os_str(),
            os("--out-dir"),
            dir.as_os_str(),
            os(flag),
            os(&value),
            os("--max-edge"),
            os(&edge),
        ],
        Duration::from_secs(300),
    );
    let frames = result.and_then(|files| {
        files
            .into_iter()
            .map(|file| {
                let rgba = std::fs::read(&file.file)?;
                ensure!(
                    rgba.len() == file.width as usize * file.height as usize * 4,
                    "Truncated helper frame"
                );
                let t = if sampled { file.actual } else { file.t };
                Ok(RgbaFrame::new(t, file.width, file.height, rgba))
            })
            .collect()
    });
    let _ = std::fs::remove_dir_all(&dir);
    frames
}
