//! Transcription: SpeechAnalyzer through `scrawl-helper` first (measured word and segment
//! timings), Hex's local API as the fallback.

use std::path::Path;

use crate::hex::RecordedAudio;
use crate::model::TranscriptData;

/// Missing microphones and unavailable transcribers never prevent saving a bundle.
pub fn transcribe(wav: Option<&Path>, audio: Option<&RecordedAudio>) -> TranscriptData {
    let Some(audio) = audio.filter(|audio| audio.sample_count > 0) else {
        return TranscriptData {
            source: "none".into(),
            ..Default::default()
        };
    };
    if std::env::var_os("SCRAWL_TRANSCRIBER").is_none_or(|v| v != "hex")
        && let Some(wav) = wav
    {
        match crate::helper::transcribe(wav) {
            Ok(transcript) => return transcript,
            Err(error) => {
                crate::capture::scrawl_log(format!("SpeechAnalyzer failed, using Hex: {error:#}"))
            }
        }
    }
    crate::hex::transcribe_with_hex(Some(audio))
}
