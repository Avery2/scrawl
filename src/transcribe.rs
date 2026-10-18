//! Transcription with Apple's SpeechAnalyzer through `scrawl-helper` (measured word and segment
//! timings).

use std::path::Path;

use crate::audio::RecordedAudio;
use crate::model::TranscriptData;

/// Missing microphones and a failed transcriber never prevent saving a bundle.
pub fn transcribe(wav: Option<&Path>, audio: Option<&RecordedAudio>) -> TranscriptData {
    let (Some(wav), Some(_)) = (wav, audio.filter(|audio| audio.sample_count > 0)) else {
        return TranscriptData {
            source: "none".into(),
            ..Default::default()
        };
    };
    crate::helper::transcribe(wav).unwrap_or_else(|error| {
        crate::capture::scrawl_log(format!("SpeechAnalyzer failed: {error:#}"));
        TranscriptData {
            source: "offline".into(),
            ..Default::default()
        }
    })
}
