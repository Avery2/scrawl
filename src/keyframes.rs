use crate::model::*;
use crate::sketch::{describe_event, describe_mark, settled_events};

const THUMB_WIDTH: usize = 64;
const THUMB_HEIGHT: usize = 36;
const THUMB_PIXELS: usize = THUMB_WIDTH * THUMB_HEIGHT;
const FRAME_GAP_MS: u64 = 750;
const SPEECH_WINDOW_MS: u64 = 1500;

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

/// Combines global luma diff with localized `8×6`-block diff (using the 2nd-highest block
/// so a single moving cursor block never false-triggers, while sidebar hover states,
/// dropdowns, and tooltips across ≥2 blocks register as visual transitions).
fn scene_transition_diff(a: &[f32; THUMB_PIXELS], b: &[f32; THUMB_PIXELS]) -> f32 {
    let global = thumbnail_diff(a, b);
    const COLS: usize = 8;
    const ROWS: usize = 6;
    const BW: usize = THUMB_WIDTH / COLS;
    const BH: usize = THUMB_HEIGHT / ROWS;
    let mut best1 = 0.0_f32;
    let mut best2 = 0.0_f32;
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
            if block_diff > best1 {
                best2 = best1;
                best1 = block_diff;
            } else if block_diff > best2 {
                best2 = block_diff;
            }
        }
    }
    global.max((best2 * 1.4).clamp(0.0, 1.0))
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

fn midpoint(start: u64, end: u64) -> u64 {
    start + end.saturating_sub(start) / 2
}

/// Attach each timed word (or whole segment when word timings are absent)
/// to the keyframe whose `[action_t0 - SPEECH_WINDOW_MS, action_t1 + SPEECH_WINDOW_MS]`
/// span covers it. Overlapping spans resolve to the nearest span midpoint; speech
/// outside every window belongs to the nearest span so no words disappear.
fn align_transcript(
    selected: &mut [(Keyframe, RgbaFrame)],
    spans: &[(u64, u64)],
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
    units.sort_by_key(|unit| unit.0);
    let mut text = vec![String::new(); selected.len()];
    for (t0, t1, unit) in units {
        let unit = unit.trim();
        if unit.is_empty() {
            continue;
        }
        let center = midpoint(t0, t1);
        let index = selected
            .iter()
            .enumerate()
            .min_by_key(|(i, (frame, _))| {
                let (s0, s1) = spans.get(*i).copied().unwrap_or((frame.t, frame.t));
                let anchor_start = s0.min(frame.t);
                let anchor_end = s1.max(frame.t);
                let start = anchor_start.saturating_sub(SPEECH_WINDOW_MS);
                let end = anchor_end.saturating_add(SPEECH_WINDOW_MS);
                let gap = if t1 < start {
                    start - t1
                } else {
                    t0.saturating_sub(end)
                };
                let span_distance = if center < anchor_start {
                    anchor_start - center
                } else if center > anchor_end {
                    center - anchor_end
                } else {
                    0
                };
                (
                    gap,
                    span_distance,
                    center.abs_diff(midpoint(anchor_start, anchor_end)),
                )
            })
            .map(|(i, _)| i)
            .unwrap();
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
    let thumbnails: Vec<_> = frames.iter().map(|frame| luma_thumbnail(frame)).collect();
    let deltas: Vec<_> = thumbnails
        .windows(2)
        .map(|pair| scene_transition_diff(&pair[0], &pair[1]))
        .collect();
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
            candidates.push(scene_candidate(frame, (start_t, frames[frame].t_ms), diff));
        }
    }
    if let Some((frame, start_t, diff)) = transition {
        candidates.push(scene_candidate(frame, (start_t, frames[frame].t_ms), diff));
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
        b.mark
            .is_some()
            .cmp(&a.mark.is_some())
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
        selected.push(candidate);
    }
    selected.sort_by_key(|candidate| frames[candidate.frame].t_ms);
    let spans: Vec<(u64, u64)> = selected.iter().map(|c| c.span).collect();
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
    align_transcript(&mut out, &spans, transcript);
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
