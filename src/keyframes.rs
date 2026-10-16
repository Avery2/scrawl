use crate::model::*;
use crate::sketch::{describe_event, describe_mark, settled_events};

const THUMB_WIDTH: usize = 64;
const THUMB_HEIGHT: usize = 36;
const THUMB_PIXELS: usize = THUMB_WIDTH * THUMB_HEIGHT;
const FRAME_GAP_MS: u64 = 750;

/// Sample the centers of a fixed grid; cost is independent of capture resolution.
fn luma_thumbnail(frame: &RgbaFrame) -> [f32; THUMB_PIXELS] {
    let mut thumbnail = [0.0; THUMB_PIXELS];
    if frame.width == 0 || frame.height == 0 {
        return thumbnail;
    }
    let width = frame.width as usize;
    let height = frame.height as usize;
    for y in 0..THUMB_HEIGHT {
        let sy = ((2 * y + 1) * height / (2 * THUMB_HEIGHT)).min(height - 1);
        for x in 0..THUMB_WIDTH {
            let sx = ((2 * x + 1) * width / (2 * THUMB_WIDTH)).min(width - 1);
            let offset = (sy * width + sx) * 4;
            if let Some(pixel) = frame.rgba.get(offset..offset + 3) {
                thumbnail[y * THUMB_WIDTH + x] = (0.2126 * pixel[0] as f32
                    + 0.7152 * pixel[1] as f32
                    + 0.0722 * pixel[2] as f32)
                    / 255.0;
            }
        }
    }
    thumbnail
}

fn thumbnail_diff(a: &[f32; THUMB_PIXELS], b: &[f32; THUMB_PIXELS]) -> f32 {
    (a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / THUMB_PIXELS as f32).clamp(0.0, 1.0)
}

/// Combines global luma diff with localized `8×6`-block diff (using the 4th-highest block
/// so a moving cursor straddling up to a 2×2 block corner in a small region capture never
/// false-triggers, while real UI transitions across ≥4 blocks register cleanly).
fn scene_transition_diff(a: &[f32; THUMB_PIXELS], b: &[f32; THUMB_PIXELS]) -> f32 {
    let global = thumbnail_diff(a, b);
    const COLS: usize = 8;
    const ROWS: usize = 6;
    const BW: usize = THUMB_WIDTH / COLS;
    const BH: usize = THUMB_HEIGHT / ROWS;
    let mut top = [0.0_f32; 4];
    for by in 0..ROWS {
        for bx in 0..COLS {
            let mut sum = 0.0_f32;
            for y in (by * BH)..((by + 1) * BH) {
                let row = y * THUMB_WIDTH;
                for x in (bx * BW)..((bx + 1) * BW) {
                    let idx = row + x;
                    sum += (a[idx] - b[idx]).abs();
                }
            }
            let block_diff = sum / (BW * BH) as f32;
            if block_diff > top[3] {
                top[3] = block_diff;
                top.sort_by(|x, y| y.total_cmp(x));
            }
        }
    }
    global.max((top[3] * 1.4).clamp(0.0, 1.0))
}

#[cfg(test)]
pub fn luma_diff(a: &RgbaFrame, b: &RgbaFrame) -> f32 {
    thumbnail_diff(&luma_thumbnail(a), &luma_thumbnail(b))
}

struct Candidate {
    frame: usize,
    span: (u64, u64),
    score: f32,
    reason: KeyframeReason,
    mark: Option<usize>,
    event: String,
    cursor: Option<[f32; 2]>,
}

fn nearest_frame(frames: &[&RgbaFrame], t: u64) -> usize {
    let after = frames.partition_point(|frame| frame.t_ms < t);
    if after == 0 {
        return 0;
    }
    if after == frames.len() {
        return after - 1;
    }
    if frames[after].t_ms.abs_diff(t) < frames[after - 1].t_ms.abs_diff(t) {
        after
    } else {
        after - 1
    }
}

/// Speech that starts a new thought: after clause punctuation, a pause, or a recognizer segment.
const CLAUSE_GAP_MS: u64 = 1_500;
/// A spoken cue only cuts a keyframe if its image differs from every keyframe already chosen.
const CUE_NOVELTY: f32 = 0.03;

/// Words that point at the screen. The strong ones are deliberate cues ("now", "look");
/// the rest are just as often filler ("I see why", "here and there", "that's weird"), so they
/// rank with ordinary scene changes and only break ties when they coincide with one.
fn spoken_cue(word: &str) -> Option<(&'static str, f32, bool)> {
    let word = word
        .trim_matches(|c: char| !c.is_alphanumeric())
        .to_lowercase();
    [
        ("now", 58.0, true),
        ("here", 56.0, true),
        ("look", 58.0, true),
        ("watch", 58.0, true),
        ("see", 44.0, false),
        ("there", 44.0, false),
        ("this", 44.0, false),
        ("these", 44.0, false),
        ("that", 42.0, false),
        ("those", 42.0, false),
    ]
    .into_iter()
    .find(|(cue, _, _)| *cue == word)
}

fn ends_clause(text: &str, clause_words: usize) -> bool {
    let trimmed = text.trim_end_matches(['"', '\'', '”', '’', ')', ']']);
    trimmed.ends_with(['.', '!', '?', ';', ':']) || (clause_words >= 3 && trimmed.ends_with(','))
}

fn word_count(text: &str) -> usize {
    text.split_whitespace()
        .filter(|w| w.chars().any(char::is_alphanumeric))
        .count()
        .max(1)
}

/// Each keyframe's text runs from the start of the clause in progress at its anchor (when its
/// action began, or the cue word) until the next keyframe's start, so the transcript is
/// partitioned in order with nothing repeated or dropped. Speech before the first keyframe
/// belongs to it.
fn align_transcript(
    selected: &mut [(Keyframe, RgbaFrame)],
    anchors: &[u64],
    transcript: &TranscriptData,
) {
    if selected.is_empty() {
        return;
    }
    let mut units: Vec<(u64, u64, &str)> = if transcript.words.is_empty() {
        transcript
            .segments
            .iter()
            .map(|s| (s.t0, s.t1, s.text.as_str()))
            .collect()
    } else {
        transcript
            .words
            .iter()
            .map(|w| (w.t0, w.t1, w.text.as_str()))
            .collect()
    };
    units.retain(|unit| !unit.2.trim().is_empty());
    units.sort_by_key(|unit| unit.0);

    let mut clauses: Vec<(u64, u64)> = Vec::new();
    let mut previous: Option<(u64, &str)> = None;
    let mut clause_words = 0_usize;
    for &(t0, t1, text) in &units {
        let segment_start = transcript.segments.iter().any(|s| s.t0.abs_diff(t0) <= 20);
        let new_clause = previous.is_none_or(|(end, prev)| {
            ends_clause(prev, clause_words)
                || t0.saturating_sub(end) >= CLAUSE_GAP_MS
                || segment_start
        });
        match clauses.last_mut() {
            Some(clause) if !new_clause => {
                clause.1 = clause.1.max(t1);
                clause_words += word_count(text);
            }
            _ => {
                clauses.push((t0, t1));
                clause_words = word_count(text);
            }
        }
        previous = Some((t1, text));
    }
    let clause_start = |anchor: u64| {
        clauses
            .iter()
            .find(|(t0, t1)| *t0 <= anchor && anchor <= *t1)
            .map_or(anchor, |clause| clause.0)
    };
    let mut starts: Vec<u64> = Vec::with_capacity(selected.len());
    for (i, (frame, _)) in selected.iter().enumerate() {
        let anchor = anchors.get(i).copied().unwrap_or(frame.t);
        let mut start = clause_start(anchor);
        if let Some(&prev) = starts.last()
            && start <= prev
        {
            // Two keyframes in one clause: the later one takes the words from its own moment.
            start = anchor.max(prev + 1);
        }
        starts.push(start);
    }

    let mut text = vec![String::new(); selected.len()];
    for (t0, _, unit) in units {
        let unit = unit.trim();
        let index = starts
            .iter()
            .rposition(|&start| start <= t0.saturating_add(50))
            .unwrap_or(0);
        // ASR words normally include their punctuation; separate punctuation tokens
        // and contractions should still produce natural text.
        let attached = unit.starts_with([',', '.', '!', '?', ';', ':', '’', '\'', ')', ']', '”']);
        let open = text[index].ends_with(['(', '[', '“']);
        if !text[index].is_empty() && !attached && !open {
            text[index].push(' ');
        }
        text[index].push_str(unit);
    }
    // Some transcribers supply full text without timings.
    if text.iter().all(String::is_empty) && !transcript.text.trim().is_empty() {
        text[0] = transcript.text.trim().to_owned();
    }
    for ((frame, _), text) in selected.iter_mut().zip(text) {
        frame.said = if text.is_empty() { None } else { Some(text) };
    }
}

/// Select representative captured frames. Callers use `MAX_KEYFRAMES` for the
/// default limit; an explicit zero requests no keyframes.
pub fn select_keyframes(
    frames: &[RgbaFrame],
    strokes: &[Stroke],
    marks: &mut [Mark],
    events: &[InteractionEvent],
    transcript: &TranscriptData,
    max_keyframes: usize,
) -> Vec<(Keyframe, RgbaFrame)> {
    for mark in marks.iter_mut() {
        mark.frame = None;
    }
    if frames.is_empty() || max_keyframes == 0 {
        return Vec::new();
    }
    let mut frames: Vec<_> = frames.iter().collect();
    frames.sort_by_key(|frame| frame.t_ms);
    let mut candidates = Vec::new();
    for (i, mark) in marks.iter().enumerate() {
        candidates.push(Candidate {
            frame: nearest_frame(&frames, mark.t1),
            span: (mark.t0, mark.t1),
            score: 100.0,
            reason: KeyframeReason::Mark,
            mark: Some(i),
            event: describe_mark(mark),
            cursor: None,
        });
    }
    let thumbnails: Vec<_> = if marks.len() < max_keyframes {
        if frames.len() >= 128 {
            let chunk_size = frames.len().div_ceil(6).max(1);
            std::thread::scope(|scope| {
                let workers: Vec<_> = frames
                    .chunks(chunk_size)
                    .map(|chunk| {
                        scope.spawn(move || -> Vec<[f32; THUMB_PIXELS]> {
                            chunk.iter().map(|frame| luma_thumbnail(frame)).collect()
                        })
                    })
                    .collect();
                let mut out = Vec::with_capacity(frames.len());
                for worker in workers {
                    out.extend(worker.join().unwrap());
                }
                out
            })
        } else {
            frames.iter().map(|frame| luma_thumbnail(frame)).collect()
        }
    } else {
        Vec::new()
    };
    if marks.len() < max_keyframes {
        let deltas: Vec<_> = thumbnails
            .windows(2)
            .map(|pair| scene_transition_diff(&pair[0], &pair[1]))
            .collect();
        for event in settled_events(events) {
            match &event {
                InteractionEvent::Click { t, x, y, .. }
                | InteractionEvent::Drag {
                    t, x1: x, y1: y, ..
                } => {
                    let end = match &event {
                        InteractionEvent::Drag { t1, .. } => *t1,
                        _ => *t,
                    };
                    let before = frames
                        .partition_point(|frame| frame.t_ms < *t)
                        .saturating_sub(1);
                    // A short delay captures the result of the action rather than the
                    // mouse-down state. Prefer an actual post-action frame when present.
                    let first_after = frames.partition_point(|frame| frame.t_ms < end);
                    let after = nearest_frame(&frames, end.saturating_add(250))
                        .max(first_after.min(frames.len() - 1));
                    let delta = thumbnail_diff(&thumbnails[before], &thumbnails[after]);
                    candidates.push(Candidate {
                        frame: after,
                        span: (*t, end),
                        score: 60.0 + 30.0 * delta,
                        reason: KeyframeReason::Click,
                        mark: None,
                        event: describe_event(&event),
                        cursor: Some([*x, *y]),
                    });
                }
                InteractionEvent::Point { t, x, y, .. } => candidates.push(Candidate {
                    frame: nearest_frame(&frames, *t),
                    span: (*t, *t),
                    score: 55.0,
                    reason: KeyframeReason::Laser,
                    mark: None,
                    event: describe_event(&event),
                    cursor: Some([*x, *y]),
                }),
                _ => {}
            }
        }
        for stroke in strokes.iter().filter(|s| s.laser) {
            let cursor = stroke.normalized_points().last().copied();
            let label = stroke
                .on
                .as_ref()
                .map(|on| format!(" “{on}”"))
                .unwrap_or_default();
            let position = cursor
                .map(|[x, y]| format!(" (x {x:.2}, y {y:.2})"))
                .unwrap_or_default();
            candidates.push(Candidate {
                frame: nearest_frame(&frames, stroke.t1),
                span: (stroke.t0, stroke.t1),
                score: 55.0,
                reason: KeyframeReason::Laser,
                mark: None,
                event: format!("pointed at{label}{position}"),
                cursor,
            });
        }
        // A run of changing frames is one transition. Its last changed frame is
        // the settled view, avoiding several near-identical animation keyframes.
        let mut transitions: Vec<(usize, (u64, u64), f32)> = Vec::new();
        let mut transition: Option<(usize, u64, f32)> = None;
        for (i, &delta) in deltas.iter().enumerate() {
            if delta > 0.08 {
                let start_t = transition.map_or(frames[i].t_ms, |(_, t0, _)| t0);
                transition = Some((
                    i + 1,
                    start_t,
                    transition.map_or(delta, |(_, _, max)| max.max(delta)),
                ));
            } else if let Some((frame, start_t, diff)) = transition.take() {
                transitions.push((frame, (start_t, frames[frame].t_ms), diff));
            }
        }
        if let Some((frame, start_t, diff)) = transition {
            transitions.push((frame, (start_t, frames[frame].t_ms), diff));
        }
        for &(frame, span, diff) in &transitions {
            candidates.push(scene_candidate(frame, span, diff));
        }
        // A pointing word is a deliberate cue ("if I say now, find the right image"). It looks for
        // a transition that starts around the word; otherwise it takes what was on screen as it
        // was said. Speech alone never cuts a keyframe over an unchanged screen: cues must show
        // something new (checked during selection), and pauses never cut at all.
        if !transcript.estimated {
            for word in &transcript.words {
                let Some((cue, score, strong)) = spoken_cue(&word.text) else {
                    continue;
                };
                let snapped = transitions
                    .iter()
                    .filter(|(_, (start, _), _)| {
                        start.saturating_add(300) >= word.t0 && *start <= word.t0 + 1_200
                    })
                    .min_by_key(|(_, (start, _), _)| start.abs_diff(word.t0));
                let (frame, score, event) = match snapped {
                    Some(&(frame, _, diff)) => (
                        frame,
                        score.max(40.0 + 40.0 * diff) + if strong { 6.0 } else { 1.0 },
                        format!("said “{cue}” as the screen changed"),
                    ),
                    None => (
                        nearest_frame(&frames, word.t0 + 150),
                        score,
                        format!("said “{cue}”"),
                    ),
                };
                candidates.push(Candidate {
                    frame,
                    span: (word.t0, word.t0),
                    score,
                    reason: KeyframeReason::Speech,
                    mark: None,
                    event,
                    cursor: None,
                });
            }
        }
    }
    if candidates.is_empty() {
        let mid = frames.len() / 2;
        let t = frames[mid].t_ms;
        candidates.push(Candidate {
            frame: mid,
            span: (t, t),
            score: 0.0,
            reason: KeyframeReason::Initial,
            mark: None,
            event: "initial view".into(),
            cursor: None,
        });
    }
    candidates.sort_by(|a, b| {
        let a_unsnapped_speech =
            a.reason == KeyframeReason::Speech && !a.event.ends_with("as the screen changed");
        let b_unsnapped_speech =
            b.reason == KeyframeReason::Speech && !b.event.ends_with("as the screen changed");
        b.mark
            .is_some()
            .cmp(&a.mark.is_some())
            .then_with(|| a_unsnapped_speech.cmp(&b_unsnapped_speech))
            .then_with(|| b.score.total_cmp(&a.score))
            .then_with(|| frames[a.frame].t_ms.cmp(&frames[b.frame].t_ms))
    });
    let mut selected: Vec<Candidate> = Vec::new();
    for candidate in candidates {
        if selected.len() == max_keyframes {
            break;
        }
        if candidate.mark.is_none()
            && selected.iter().any(|other| {
                frames[other.frame]
                    .t_ms
                    .abs_diff(frames[candidate.frame].t_ms)
                    < FRAME_GAP_MS
            })
        {
            continue;
        }
        if matches!(
            candidate.reason,
            KeyframeReason::Speech | KeyframeReason::SceneChange
        ) && selected.iter().any(|other| {
            scene_transition_diff(&thumbnails[other.frame], &thumbnails[candidate.frame])
                < CUE_NOVELTY
        }) {
            continue;
        }
        selected.push(candidate);
    }
    // Order-independent novelty pass: if a speech cue duplicates the screen of any visual/input
    // keyframe selected after it, drop the redundant speech cue (keeping at least one frame).
    if selected.len() > 1 {
        let non_speech_frames: Vec<usize> = selected
            .iter()
            .filter(|c| c.reason != KeyframeReason::Speech)
            .map(|c| c.frame)
            .collect();
        if !non_speech_frames.is_empty() {
            selected.retain(|c| {
                c.reason != KeyframeReason::Speech
                    || non_speech_frames.iter().all(|&other_frame| {
                        scene_transition_diff(&thumbnails[other_frame], &thumbnails[c.frame])
                            >= CUE_NOVELTY
                    })
            });
        }
    }
    selected.sort_by_key(|candidate| frames[candidate.frame].t_ms);
    let anchors: Vec<u64> = selected
        .iter()
        .map(|c| c.span.0.min(frames[c.frame].t_ms))
        .collect();
    let mut out: Vec<_> = selected
        .into_iter()
        .enumerate()
        .map(|(i, candidate)| {
            let index = i + 1;
            let file = format!("frames/frame-{index:02}.png");
            let mark_id = candidate.mark.map(|j| {
                marks[j].frame = Some(file.clone());
                marks[j].id.clone()
            });
            let frame = frames[candidate.frame];
            (
                Keyframe {
                    index,
                    t: frame.t_ms,
                    reason: candidate.reason,
                    file,
                    event: candidate.event,
                    said: None,
                    mark_id,
                    cursor: candidate.cursor,
                },
                frame.clone(),
            )
        })
        .collect();
    align_transcript(&mut out, &anchors, transcript);
    out
}

fn scene_candidate(frame: usize, span: (u64, u64), diff: f32) -> Candidate {
    Candidate {
        frame,
        span,
        score: 40.0 + 40.0 * diff,
        reason: KeyframeReason::SceneChange,
        mark: None,
        event: "scene changed".into(),
        cursor: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(t: u64, gray: u8) -> RgbaFrame {
        RgbaFrame::new(t, 2, 2, [gray, gray, gray, 255].repeat(4))
    }

    fn mark(id: &str, t: u64) -> Mark {
        Mark {
            id: id.into(),
            strokes: vec![0],
            t0: t.saturating_sub(100),
            t1: t,
            box_norm: [0.5; 4],
            shape: Shape::Tap,
            from: None,
            to: None,
            pin: None,
            on: None,
            description: "tapped x 0.50, y 0.50".into(),
            frame: None,
        }
    }

    fn click(t: u64) -> InteractionEvent {
        InteractionEvent::Click {
            t,
            x: 0.5,
            y: 0.5,
            label: None,
            role: None,
            button: None,
        }
    }

    #[test]
    fn luma_is_normalized_and_resolution_independent() {
        assert_eq!(luma_diff(&frame(0, 0), &frame(1, 0)), 0.0);
        assert!((luma_diff(&frame(0, 0), &frame(1, 255)) - 1.0).abs() < 0.001);
        assert!((luma_diff(&frame(0, 0), &frame(1, 128)) - 128.0 / 255.0).abs() < 0.001);
        let larger = RgbaFrame::new(0, 128, 72, [128, 128, 128, 255].repeat(128 * 72));
        assert_eq!(luma_diff(&frame(0, 128), &larger), 0.0);
    }

    #[test]
    fn marks_take_priority_even_when_close_together() {
        let frames = vec![
            frame(0, 0),
            frame(1000, 255),
            frame(2000, 255),
            frame(2200, 255),
        ];
        let mut marks = vec![mark("m1", 2000), mark("m2", 2200), mark("m3", 2200)];
        let selected = select_keyframes(
            &frames,
            &[],
            &mut marks,
            &[click(750)],
            &TranscriptData::default(),
            2,
        );
        assert_eq!(selected.len(), 2);
        assert!(
            selected
                .iter()
                .all(|(k, _)| k.reason == KeyframeReason::Mark)
        );
        assert_eq!(selected[0].0.mark_id.as_deref(), Some("m1"));
        assert_eq!(selected[1].0.mark_id.as_deref(), Some("m2"));
        assert_eq!(marks[0].frame.as_deref(), Some("frames/frame-01.png"));
        assert_eq!(marks[1].frame.as_deref(), Some("frames/frame-02.png"));
        assert_eq!(marks[2].frame, None);
    }

    #[test]
    fn scene_changes_select_the_settled_view() {
        let frames = vec![
            frame(0, 0),
            frame(1000, 80),
            frame(1250, 160),
            frame(1500, 255),
            frame(1750, 255),
        ];
        let selected = select_keyframes(
            &frames,
            &[],
            &mut [],
            &[],
            &TranscriptData::default(),
            MAX_KEYFRAMES,
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0.reason, KeyframeReason::SceneChange);
        assert_eq!(selected[0].0.t, 1500);
        assert_eq!(selected[0].1.rgba, frames[3].rgba);
    }

    #[test]
    fn nearby_candidates_are_deduplicated_using_capture_times() {
        let frames: Vec<_> = (0..=12).map(|i| frame(i * 250, 128)).collect();
        let events = vec![click(1000), click(1100), click(2300)];
        let selected = select_keyframes(
            &frames,
            &[],
            &mut [],
            &events,
            &TranscriptData::default(),
            MAX_KEYFRAMES,
        );
        assert_eq!(selected.len(), 2);
        assert!(selected[1].0.t - selected[0].0.t >= FRAME_GAP_MS);
        assert_eq!(selected[0].0.index, 1);
        assert_eq!(selected[1].0.file, "frames/frame-02.png");
    }

    #[test]
    fn transcript_words_partition_without_duplicates_or_gaps() {
        let frames = vec![frame(1000, 0), frame(4000, 0)];
        let mut marks = vec![mark("m1", 1000), mark("m2", 4000)];
        let transcript = TranscriptData {
            text: "Make this smaller, and move that.".into(),
            words: [
                (0, "Make"),
                (1000, "this"),
                (2000, "smaller"),
                (2100, ","),
                (3000, "and"),
                (4000, "move"),
                (7000, "that."),
            ]
            .into_iter()
            .map(|(t, text)| WordTiming {
                t0: t,
                t1: t + 100,
                text: text.into(),
            })
            .collect(),
            segments: vec![TranscriptSegment {
                t0: 0,
                t1: 7100,
                text: "Make this smaller, and move that.".into(),
            }],
            ..Default::default()
        };
        let selected = select_keyframes(&frames, &[], &mut marks, &[], &transcript, MAX_KEYFRAMES);
        assert_eq!(selected[0].0.said.as_deref(), Some("Make this smaller,"));
        assert_eq!(selected[1].0.said.as_deref(), Some("and move that."));
    }

    #[test]
    fn segment_only_transcript_is_assigned_once() {
        let frames = vec![frame(1000, 0), frame(5000, 0)];
        let events = vec![click(1000), click(5000)];
        let transcript = TranscriptData {
            segments: vec![
                TranscriptSegment {
                    t0: 0,
                    t1: 2000,
                    text: "First thought.".into(),
                },
                TranscriptSegment {
                    t0: 4000,
                    t1: 6000,
                    text: "Second thought.".into(),
                },
            ],
            ..Default::default()
        };
        let selected = select_keyframes(&frames, &[], &mut [], &events, &transcript, MAX_KEYFRAMES);
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].0.said.as_deref(), Some("First thought."));
        assert_eq!(selected[1].0.said.as_deref(), Some("Second thought."));

        // Speech alone over a static screen should not cut extra keyframes.
        let static_speech =
            select_keyframes(&frames, &[], &mut [], &[], &transcript, MAX_KEYFRAMES);
        assert_eq!(static_speech.len(), 1);
        assert_eq!(static_speech[0].0.reason, KeyframeReason::Initial);
        // Words spoken during a long mark span stay attached to that mark even when
        // the post-mark keyframe timestamp is near the end of the stroke.
        let mut long_marks = vec![
            Mark {
                t0: 500,
                t1: 600,
                ..mark("m1", 600)
            },
            Mark {
                t0: 1800,
                t1: 5000,
                ..mark("m2", 5000)
            },
        ];
        let span_frames = vec![frame(600, 0), frame(5000, 0)];
        let span_transcript = TranscriptData {
            words: [
                (400, "First."),
                (2000, "Started"),
                (2600, "drawing"),
                (4800, "this."),
            ]
            .into_iter()
            .map(|(t, text)| WordTiming {
                t0: t,
                t1: t + 100,
                text: text.into(),
            })
            .collect(),
            ..Default::default()
        };
        let span_selected = select_keyframes(
            &span_frames,
            &[],
            &mut long_marks,
            &[],
            &span_transcript,
            MAX_KEYFRAMES,
        );
        assert_eq!(span_selected[0].0.said.as_deref(), Some("First."));
        assert_eq!(
            span_selected[1].0.said.as_deref(),
            Some("Started drawing this.")
        );
    }

    fn words(list: &[(u64, &str)]) -> Vec<WordTiming> {
        list.iter()
            .map(|&(t, text)| WordTiming {
                t0: t,
                t1: t + 200,
                text: text.into(),
            })
            .collect()
    }

    #[test]
    fn spoken_cues_never_duplicate_an_unchanged_screen() {
        let frames: Vec<_> = (0..=20).map(|i| frame(i * 250, 90)).collect();
        let transcript = TranscriptData {
            words: words(&[
                (500, "Look"),
                (700, "at"),
                (900, "this."),
                (3000, "Now"),
                (3200, "it's"),
                (3400, "stuck."),
            ]),
            ..Default::default()
        };
        let selected = select_keyframes(&frames, &[], &mut [], &[], &transcript, MAX_KEYFRAMES);
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0.reason, KeyframeReason::Speech);
        assert_eq!(
            selected[0].0.said.as_deref(),
            Some("Look at this. Now it's stuck.")
        );

        // Even when a scene change (score < 58) settles onto a screen and a strong unsnapped
        // cue ("Look", score 58) is spoken later over that same settled screen, cue novelty
        // must be enforced order-independently so the settled screen is not duplicated.
        let mut changing: Vec<_> = (0..=20).map(|i| frame(i * 250, 40)).collect();
        for f in changing.iter_mut().filter(|f| f.t_ms >= 1_000) {
            *f = frame(f.t_ms, 90);
        }
        let cue_after_change = TranscriptData {
            words: words(&[(3_500, "Look"), (3_700, "here.")]),
            ..Default::default()
        };
        let selected2 = select_keyframes(
            &changing,
            &[],
            &mut [],
            &[],
            &cue_after_change,
            MAX_KEYFRAMES,
        );
        assert_eq!(selected2.len(), 1);
        assert_eq!(selected2[0].0.reason, KeyframeReason::SceneChange);
    }

    #[test]
    fn short_comma_lead_in_stays_with_its_clause() {
        let frames: Vec<_> = (0..=24).map(|i| frame(i * 250, 128)).collect();
        let transcript = TranscriptData {
            words: words(&[
                (200, "The"),
                (400, "message"),
                (600, "was"),
                (800, "received."),
                (2_000, "Um,"),
                (2_300, "Or"),
                (2_500, "it"),
                (2_700, "should"),
                (2_900, "appear."),
            ]),
            ..Default::default()
        };
        let selected = select_keyframes(
            &frames,
            &[],
            &mut [],
            &[click(500), click(2_600)],
            &transcript,
            MAX_KEYFRAMES,
        );
        assert_eq!(selected.len(), 2);
        assert_eq!(
            selected[0].0.said.as_deref(),
            Some("The message was received.")
        );
        assert_eq!(
            selected[1].0.said.as_deref(),
            Some("Um, Or it should appear.")
        );
    }

    #[test]
    fn a_spoken_cue_finds_the_change_it_points_at() {
        // The screen changes 0.6 s after "now"; the cue picks the settled view, and its clause
        // starts that keyframe's text.
        let mut frames: Vec<_> = (0..=24).map(|i| frame(i * 250, 40)).collect();
        for f in frames.iter_mut().filter(|f| f.t_ms >= 3_250) {
            *f = frame(f.t_ms, 220);
        }
        let transcript = TranscriptData {
            words: words(&[
                (200, "Here's"),
                (400, "the"),
                (600, "list."),
                (2_400, "Watch"),
                (2_600, "it"),
                (2_800, "now."),
            ]),
            ..Default::default()
        };
        let selected = select_keyframes(
            &frames,
            &[],
            &mut [],
            &[click(500)],
            &transcript,
            MAX_KEYFRAMES,
        );
        assert_eq!(
            selected.len(),
            2,
            "{:?}",
            selected.iter().map(|k| &k.0).collect::<Vec<_>>()
        );
        assert_eq!(selected[1].0.t, 3_250);
        assert_eq!(selected[1].0.reason, KeyframeReason::Speech);
        assert_eq!(selected[0].0.said.as_deref(), Some("Here's the list."));
        assert_eq!(selected[1].0.said.as_deref(), Some("Watch it now."));
    }

    #[test]
    fn estimated_word_times_do_not_cut_keyframes() {
        let mut frames: Vec<_> = (0..=8).map(|i| frame(i * 250, 40)).collect();
        frames.push(frame(2_500, 220));
        let transcript = TranscriptData {
            words: words(&[(1_000, "now")]),
            estimated: true,
            ..Default::default()
        };
        let selected = select_keyframes(&frames, &[], &mut [], &[], &transcript, MAX_KEYFRAMES);
        assert!(
            selected
                .iter()
                .all(|k| k.0.reason != KeyframeReason::Speech)
        );
    }

    #[test]
    fn an_action_mid_sentence_takes_the_sentence_from_its_start() {
        let frames: Vec<_> = (0..=24).map(|i| frame(i * 250, 128)).collect();
        let transcript = TranscriptData {
            words: words(&[
                (100, "First"),
                (300, "the"),
                (500, "header."),
                (2_000, "When"),
                (2_200, "I"),
                (2_400, "tap"),
                (2_600, "it,"),
                (2_800, "nothing."),
            ]),
            ..Default::default()
        };
        let selected = select_keyframes(
            &frames,
            &[],
            &mut [],
            &[click(400), click(2_450)],
            &transcript,
            MAX_KEYFRAMES,
        );
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].0.said.as_deref(), Some("First the header."));
        assert_eq!(
            selected[1].0.said.as_deref(),
            Some("When I tap it, nothing.")
        );
    }

    #[test]
    fn flags_are_priority_keyframes() {
        let frames: Vec<_> = (0..=8).map(|i| frame(i * 250, 128)).collect();
        let mut marks = vec![Mark {
            strokes: Vec::new(),
            t0: 1_500,
            shape: Shape::Flag,
            ..mark("f1", 1_500)
        }];
        let selected = select_keyframes(
            &frames,
            &[],
            &mut marks,
            &[click(1_600)],
            &TranscriptData::default(),
            1,
        );
        assert_eq!(selected[0].0.mark_id.as_deref(), Some("f1"));
        assert_eq!(selected[0].0.t, 1_500);
    }

    #[test]
    fn fallback_and_empty_inputs() {
        let frames = vec![frame(0, 0), frame(1000, 0), frame(2000, 0)];
        let selected = select_keyframes(
            &frames,
            &[],
            &mut [],
            &[],
            &TranscriptData::default(),
            MAX_KEYFRAMES,
        );
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0.reason, KeyframeReason::Initial);
        assert_eq!(selected[0].0.t, 1000);
        assert!(
            select_keyframes(&frames, &[], &mut [], &[], &TranscriptData::default(), 0).is_empty()
        );
        assert!(
            select_keyframes(
                &[],
                &[],
                &mut [],
                &[],
                &TranscriptData::default(),
                MAX_KEYFRAMES
            )
            .is_empty()
        );
    }
}
