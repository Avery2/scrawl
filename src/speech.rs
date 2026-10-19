//! On-device transcription on Windows and Linux, with measured word timings: Parakeet through
//! `transcribe.cpp`, the engine Hex uses. It loads a model Hex has already installed
//! (`voice-control/models` in the user's data folder), or the one Scrawl downloads itself on
//! request (`download_model`, into `scrawl/models` beside it, checked against its SHA-256 and
//! laid out the way Hex lays out its own). Like the Hex client, it transcribes each speech
//! island on its own; unlike it, every word keeps the time the model heard it
//! (`estimated: false`), so spoken cues ("look here") can cut keyframes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use transcribe_cpp::{Model, RunOptions, TimestampKind};

use crate::model::{TranscriptData, TranscriptSegment, WordTiming};

const KHZ: u64 = 16;
/// Silence kept around each clip, so first and last words aren't clipped.
const CLIP_PAD_MS: u64 = 200;

fn models_dir() -> PathBuf {
    crate::hex::support_dir().join("models")
}

/// Where Scrawl keeps the model it downloads (`~/.local/share/scrawl/models` on Linux).
pub fn own_models_dir() -> PathBuf {
    std::env::var_os("SCRAWL_MODELS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::data_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("scrawl/models")
        })
}

/// Installed models (verified downloads): Hex's chosen one first, then Parakeet ones, from
/// Hex's folder and Scrawl's own.
pub fn installed_models() -> Vec<PathBuf> {
    let entries: Vec<std::fs::DirEntry> = [models_dir(), own_models_dir()]
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .filter_map(Result::ok)
        .collect();
    if entries.is_empty() {
        return Vec::new();
    }
    let chosen = std::fs::read(crate::hex::support_dir().join("settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|settings| {
            settings["transcription"]["model"]
                .as_str()
                .map(str::to_owned)
        })
        .map(|id| id.replace('_', "-"));
    let mut models: Vec<PathBuf> = entries
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "gguf")
                && path
                    .with_file_name(format!(
                        "{}.verified.json",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ))
                    .is_file()
        })
        .collect();
    let rank = |path: &PathBuf| {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let picked = chosen.as_deref().is_some_and(|id| name.starts_with(id));
        (!picked, !name.starts_with("parakeet"), name.into_owned())
    };
    models.sort_by_key(rank);
    models
}

/// The model Scrawl downloads when there is none: Hex's English Parakeet, pinned to a revision
/// and checked against its SHA-256.
pub struct Artifact {
    pub filename: &'static str,
    pub url: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

pub const DEFAULT_MODEL: Artifact = Artifact {
    filename: "parakeet-unified-en-0.6b-Q8_0.gguf",
    url: "https://huggingface.co/handy-computer/parakeet-unified-en-0.6b-gguf/resolve/7e948f21b7bdbac698d3318db9d350f1096f3b6c/parakeet-unified-en-0.6b-Q8_0.gguf",
    bytes: 731_357_568,
    sha256: "4b50b6dd862bf6e346929aaf4f5eaacec003bfa3f56462d6c874b41ef2f38795",
};

/// Downloads `artifact` into `own_models_dir()` with `curl` (or `wget`), checks its size and
/// SHA-256, and marks it verified as Hex does (`<file>.verified.json`), so `installed_models`
/// finds it. `progress` gets (bytes so far, total) about twice a second.
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub fn download_model(artifact: &Artifact, progress: &dyn Fn(u64, u64)) -> Result<PathBuf> {
    use sha2::Digest as _;
    use std::io::Read as _;

    let dir = own_models_dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("Could not create {}", dir.display()))?;
    let path = dir.join(artifact.filename);
    let part = dir.join(format!("{}.part", artifact.filename));
    let _ = std::fs::remove_file(&part);
    let mut child = if crate::handoff::on_path("curl").is_some() {
        std::process::Command::new("curl")
            .args(["--fail", "--location", "--silent", "--show-error", "--output"])
            .arg(&part)
            .arg(artifact.url)
            .spawn()
    } else {
        std::process::Command::new("wget")
            .args(["--quiet", "--output-document"])
            .arg(&part)
            .arg(artifact.url)
            .spawn()
    }
    .context("Downloading needs curl or wget")?;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        let have = std::fs::metadata(&part).map_or(0, |m| m.len());
        progress(have, artifact.bytes);
        std::thread::sleep(std::time::Duration::from_millis(500));
    };
    if !status.success() {
        let _ = std::fs::remove_file(&part);
        bail!("Could not download the speech model ({status})");
    }
    let mut file = std::fs::File::open(&part)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0_u8; 1 << 20];
    let mut total = 0_u64;
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        total += n as u64;
        hasher.update(&buffer[..n]);
    }
    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    if total != artifact.bytes || digest != artifact.sha256 {
        let _ = std::fs::remove_file(&part);
        bail!("The downloaded speech model didn't match its checksum; try again");
    }
    std::fs::rename(&part, &path)?;
    std::fs::write(
        path.with_file_name(format!("{}.verified.json", artifact.filename)),
        serde_json::to_vec_pretty(&serde_json::json!({
            "sha256": artifact.sha256,
            "bytes": artifact.bytes,
            "source": artifact.url,
        }))?,
    )?;
    progress(artifact.bytes, artifact.bytes);
    Ok(path)
}

/// Where Settings' "Download speech model" is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub enum Download {
    #[default]
    Idle,
    Running {
        have: u64,
        total: u64,
    },
    Failed(String),
}

static DOWNLOAD: std::sync::Mutex<Download> = std::sync::Mutex::new(Download::Idle);

#[cfg_attr(target_os = "windows", allow(dead_code))]
pub fn download_state() -> Download {
    DOWNLOAD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

/// Downloads `DEFAULT_MODEL` on a thread of its own, once at a time (`download_state` follows).
#[cfg_attr(target_os = "windows", allow(dead_code))]
pub fn start_download() {
    let set = |state: Download| {
        *DOWNLOAD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = state;
    };
    {
        let mut state = DOWNLOAD
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if matches!(*state, Download::Running { .. }) {
            return;
        }
        *state = Download::Running {
            have: 0,
            total: DEFAULT_MODEL.bytes,
        };
    }
    let _ = std::thread::Builder::new()
        .name("scrawl-model-download".into())
        .spawn(move || {
            match download_model(&DEFAULT_MODEL, &|have, total| {
                set(Download::Running { have, total });
            }) {
                Ok(path) => {
                    crate::capture::scrawl_log(format!("Speech model saved: {}", path.display()));
                    set(Download::Idle);
                    #[cfg(target_os = "linux")]
                    crate::linux::shell::notify(
                        "Speech model ready",
                        "Scrawl transcribes your captures on this computer now.",
                    );
                }
                Err(error) => {
                    crate::capture::scrawl_log(format!("Speech model download: {error:#}"));
                    set(Download::Failed(format!("{error}")));
                }
            }
        });
}

/// The model's name for `TranscriptData.model`: its file name without the extension.
fn model_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn transcribe(audio: &Path) -> Result<TranscriptData> {
    let Some(path) = installed_models().into_iter().next() else {
        #[cfg(target_os = "linux")]
        {
            static TOLD: std::sync::Once = std::sync::Once::new();
            TOLD.call_once(|| {
                crate::linux::shell::notify(
                    "No speech model yet",
                    "Download Parakeet in Scrawl's Settings › Recording to get transcripts.",
                );
            });
            bail!("No transcription model: download Parakeet in Scrawl's Settings › Recording");
        }
        #[cfg(not(target_os = "linux"))]
        bail!(
            "No transcription model: install Hex (https://github.com/anomalyco/hex) and download a model in it"
        );
    };
    let wav =
        std::fs::read(audio).with_context(|| format!("Could not read {}", audio.display()))?;
    let samples = crate::audio::decode_wav_16k(&wav)?;
    let duration_ms = samples.len() as u64 / KHZ;
    let mut clips = crate::hex::plan_clips(&crate::audio::speech_islands(&samples), duration_ms);
    if clips.is_empty() && duration_ms >= 480 {
        clips.push((0, duration_ms));
    }
    let name = model_name(&path);
    let mut transcript = TranscriptData {
        source: format!("parakeet:{name}"),
        model: Some(name),
        estimated: false,
        ..Default::default()
    };
    if clips.is_empty() {
        return Ok(transcript);
    }

    transcribe_cpp::disable_logging();
    transcribe_cpp::init_backends_default().context("transcribe.cpp backends")?;
    let model = Model::load(&path).with_context(|| format!("Could not load {}", path.display()))?;
    let mut session = model.session()?;
    let options = RunOptions {
        timestamps: TimestampKind::Word,
        language: Some("en".into()),
        ..Default::default()
    };
    let mut texts = Vec::new();
    for (start, end) in clips {
        let from_ms = start.saturating_sub(CLIP_PAD_MS);
        let from = (from_ms * KHZ) as usize;
        let to = (((end + CLIP_PAD_MS) * KHZ) as usize).min(samples.len());
        if from >= to {
            continue;
        }
        let result = session.run(&samples[from..to], &options)?;
        let text = result.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        let at = |ms: i64| from_ms + ms.max(0) as u64;
        let words: Vec<WordTiming> = result
            .words
            .iter()
            .filter(|word| !word.text.trim().is_empty())
            .map(|word| WordTiming {
                t0: at(word.t0_ms),
                t1: at(word.t1_ms.max(word.t0_ms)),
                text: word.text.trim().to_owned(),
            })
            .collect();
        if words.is_empty() {
            // No word times from this model: spread the text over the clip, and say so.
            let (segments, words) = crate::hex::spread_words(&text, start, end);
            transcript.estimated = true;
            transcript.segments.extend(segments);
            transcript.words.extend(words);
        } else {
            let segments: Vec<TranscriptSegment> = result
                .segments
                .iter()
                .filter(|s| !s.text.trim().is_empty())
                .map(|s| TranscriptSegment {
                    t0: at(s.t0_ms),
                    t1: at(s.t1_ms.max(s.t0_ms)),
                    text: s.text.trim().to_owned(),
                })
                .collect();
            transcript.segments.extend(if segments.is_empty() {
                vec![TranscriptSegment {
                    t0: words[0].t0,
                    t1: words[words.len() - 1].t1,
                    text: text.clone(),
                }]
            } else {
                segments
            });
            transcript.words.extend(words);
        }
        texts.push(text);
    }
    transcript.text = texts.join(" ");
    Ok(transcript)
}
