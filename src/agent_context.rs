//! The `agentContext` experiment's text: each keyframe's speech broken up by what happened while
//! it was said (marks, clicks, drags, shortcuts, windows opening and closing, app events), so the
//! agent reads "this part" next to the mark drawn as it was said. Marks sharing a keyframe are
//! numbered ①②③ in drawing order, matching the badges `render` draws at each mark's start.

use crate::model::*;
use crate::sketch::{describe_event, describe_mark, marks_on, settled_events};

/// One keyframe's text: `head` follows `- #N (M:SS)` on the bullet line, `lines` go under it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameText {
    pub head: String,
    pub lines: Vec<String>,
}

/// How far back an item's place in the speech moves to the start of its clause.
const CLAUSE_SNAP_MS: u64 = 3_000;

/// How far forward an item's place moves to the end of its clause when the start is too far back.
const CLAUSE_SNAP_AFTER_MS: u64 = 1_200;

/// A pause this long ends a clause (`keyframes::CLAUSE_GAP_MS`).
const CLAUSE_GAP_MS: u64 = 1_500;

/// Drags that moved less than this (normalized) are a held click's jitter, not a drag.
const MIN_DRAG: f32 = 0.01;

/// Repeats of one shortcut closer together than this read as one line (`pressed ⌘T ×3`).
const KEY_REPEAT_MS: u64 = 2_000;

/// More on-screen windows than this changing at once (a Space switch) is one line.
const WINDOW_BURST: usize = 4;

#[derive(Clone, Debug)]
enum Item<'a> {
    Mark {
        mark: &'a Mark,
        number: Option<usize>,
    },
    Event {
        text: String,
    },
}

#[derive(Clone, Debug)]
struct Timed<'a> {
    t: u64,
    item: Item<'a>,
}

/// The circled number for the `n`th mark (1-based) on a shared keyframe.
pub fn circled(n: usize) -> String {
    const DIGITS: [char; 20] = [
        '①', '②', '③', '④', '⑤', '⑥', '⑦', '⑧', '⑨', '⑩', '⑪', '⑫', '⑬', '⑭', '⑮', '⑯', '⑰', '⑱',
        '⑲', '⑳',
    ];
    DIGITS
        .get(n.wrapping_sub(1))
        .map_or_else(|| format!("({n})"), char::to_string)
}

pub fn keyframe_texts(bundle: &ScrawlBundle) -> Vec<FrameText> {
    let frames = &bundle.keyframes;
    if frames.is_empty() {
        return Vec::new();
    }
    let words = frame_words(bundle);
    let starts = frame_starts(bundle, words.as_deref());
    let owner = |t: u64| starts.iter().rposition(|&s| s <= t).unwrap_or(0);

    let mut items: Vec<Vec<Timed>> = vec![Vec::new(); frames.len()];
    for (i, frame) in frames.iter().enumerate() {
        let shared = marks_on(frame, &bundle.marks);
        let numbered = shared.len() >= 2;
        for (n, mark) in shared.into_iter().enumerate() {
            items[i].push(Timed {
                t: mark.t0,
                item: Item::Mark {
                    mark,
                    number: numbered.then_some(n + 1),
                },
            });
        }
    }
    for (t, text) in events(bundle) {
        items[owner(t)].push(Timed {
            t,
            item: Item::Event { text },
        });
    }

    frames
        .iter()
        .enumerate()
        .map(|(i, frame)| {
            let mut timed = std::mem::take(&mut items[i]);
            timed.sort_by_key(|item| item.t);
            let mut text = match &words {
                Some(words) => interleave(&words[i], &timed),
                None => interleave_unsplit(frame.said.as_deref(), &timed),
            };
            if let Some(changed) = changed_regions(bundle, i) {
                text.head = if text.head.is_empty() {
                    format!("[changed: {changed}]")
                } else {
                    format!("[changed: {changed}] {}", text.head)
                };
            }
            if text.head.is_empty() && text.lines.is_empty() {
                text.head = frame.event.clone();
            }
            text
        })
        .collect()
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Recovers which timed words each keyframe's `said` was built from (`keyframes::align_transcript`
/// gives every frame a run of the time-sorted words). `None` when the transcript has no word
/// times or the text doesn't line up, and the speech is then shown whole.
fn frame_words(bundle: &ScrawlBundle) -> Option<Vec<Vec<&WordTiming>>> {
    let mut words: Vec<&WordTiming> = bundle
        .transcript
        .words
        .iter()
        .filter(|w| !w.text.trim().is_empty())
        .collect();
    if words.is_empty() {
        return None;
    }
    words.sort_by_key(|w| w.t0);
    let mut next = 0;
    let mut out = Vec::with_capacity(bundle.keyframes.len());
    for frame in &bundle.keyframes {
        let target = normalized(frame.said.as_deref().unwrap_or(""));
        let mut acc = String::new();
        let mut taken = Vec::new();
        while acc.len() < target.len() && next < words.len() {
            acc.push_str(&normalized(&words[next].text));
            taken.push(words[next]);
            next += 1;
        }
        if acc != target {
            return None;
        }
        out.push(taken);
    }
    Some(out)
}

/// When each keyframe's stretch of the recording starts: its first word, first mark or own moment,
/// whichever is earliest, and never before the previous one. The first frame owns the lead-in.
fn frame_starts(bundle: &ScrawlBundle, words: Option<&[Vec<&WordTiming>]>) -> Vec<u64> {
    let mut starts: Vec<u64> = Vec::with_capacity(bundle.keyframes.len());
    for (i, frame) in bundle.keyframes.iter().enumerate() {
        let first_word = words.and_then(|w| w[i].first().map(|w| w.t0));
        let first_mark = marks_on(frame, &bundle.marks).first().map(|m| m.t0);
        let start = [Some(frame.t), first_word, first_mark]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(frame.t);
        let start = if i == 0 { 0 } else { start.max(starts[i - 1]) };
        starts.push(start);
    }
    starts
}

/// Everything besides marks that the summary shows inline, in time order.
fn events(bundle: &ScrawlBundle) -> Vec<(u64, String)> {
    let mut out: Vec<(u64, String)> = Vec::new();
    let mut last_key: Option<(u64, String, usize)> = None;
    fn flush_key(out: &mut Vec<(u64, String)>, key: Option<(u64, String, usize)>) {
        if let Some((t, shortcut, count)) = key {
            let times = if count > 1 {
                format!(" ×{count}")
            } else {
                String::new()
            };
            out.push((t, format!("pressed {shortcut}{times}")));
        }
    }
    let mut key_t = 0;
    for event in settled_events(&bundle.events) {
        match &event {
            InteractionEvent::Key { t, shortcut } => {
                match &mut last_key {
                    Some((_, prev, count))
                        if prev == shortcut && t.saturating_sub(key_t) < KEY_REPEAT_MS =>
                    {
                        *count += 1;
                    }
                    _ => {
                        flush_key(&mut out, last_key.take());
                        last_key = Some((*t, shortcut.clone(), 1));
                    }
                }
                key_t = *t;
            }
            InteractionEvent::AppChange { t, app, .. } => {
                // Window titles stay in scrawl.json; the summary names apps only.
                out.push((*t, format!("switched to {app}")));
            }
            InteractionEvent::Drag { x0, y0, x1, y1, .. }
                if (x1 - x0).hypot(y1 - y0) < MIN_DRAG => {}
            other => out.push((other.timestamp_ms(), describe_event(other))),
        }
    }
    flush_key(&mut out, last_key);
    out.extend(window_changes(&bundle.windows));
    out.extend(bundle.context_changes.iter().map(|change| {
        let text = match &change.value {
            Some(value) => format!("{} → {}", one_line(&change.key), one_line(value)),
            None => format!("{} cleared", one_line(&change.key)),
        };
        (change.t, text)
    }));
    out.sort_by_key(|(t, _)| *t);
    out
}

fn one_line(raw: &str) -> String {
    let compact = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 60 {
        compact
    } else {
        let cut: String = compact.chars().take(59).collect();
        format!("{}…", cut.trim_end())
    }
}

/// Ordinary app windows appearing and disappearing between window-log snapshots, by app name.
fn window_changes(snapshots: &[WindowSnapshot]) -> Vec<(u64, String)> {
    let normal = |snapshot: &WindowSnapshot| -> Vec<(u32, String)> {
        snapshot
            .windows
            .iter()
            .filter(|w| w.layer == 0 && w.regular && w.app != "Scrawl")
            .map(|w| (w.id, w.app.clone()))
            .collect()
    };
    let mut out = Vec::new();
    for pair in snapshots.windows(2) {
        let (before, after) = (normal(&pair[0]), normal(&pair[1]));
        let opened: Vec<&String> = after
            .iter()
            .filter(|(id, _)| !before.iter().any(|(b, _)| b == id))
            .map(|(_, app)| app)
            .collect();
        let closed: Vec<&String> = before
            .iter()
            .filter(|(id, _)| !after.iter().any(|(a, _)| a == id))
            .map(|(_, app)| app)
            .collect();
        let t = pair[1].t;
        if opened.len() + closed.len() > WINDOW_BURST {
            out.push((
                t,
                format!(
                    "on-screen windows changed ({} opened, {} closed)",
                    opened.len(),
                    closed.len()
                ),
            ));
            continue;
        }
        for app in closed {
            out.push((t, format!("{app} window closed")));
        }
        for app in opened {
            out.push((t, format!("{app} window opened")));
        }
    }
    out
}

/// Joins recognizer words the way `keyframes::align_transcript` does.
fn join_words(words: &[&WordTiming]) -> String {
    let mut text = String::new();
    for word in words {
        let unit = word.text.trim();
        let attached = unit.starts_with([',', '.', '!', '?', ';', ':', '’', '\'', ')', ']', '”']);
        let open = text.ends_with(['(', '[', '“']);
        if !text.is_empty() && !attached && !open {
            text.push(' ');
        }
        text.push_str(unit);
    }
    text
}

fn quoted(text: &str) -> String {
    let text = text.trim();
    if text.is_empty() {
        String::new()
    } else {
        format!("“{text}”")
    }
}

fn label(item: &Timed) -> String {
    let clock = format_clock(item.t);
    match &item.item {
        Item::Mark { mark, number } => {
            let n = number.map(circled).unwrap_or_default();
            format!("✎{n} {clock} {}", describe_mark(mark))
        }
        Item::Event { text } => format!("· {clock} {text}"),
    }
}

/// Indexes of the words that start a clause: after clause punctuation (a comma once the clause
/// has three words), or after a pause of `CLAUSE_GAP_MS`, as `keyframes` breaks clauses.
fn clause_starts(words: &[&WordTiming]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut clause_words = 0;
    for (i, word) in words.iter().enumerate() {
        let new_clause = i == 0 || {
            let prev = words[i - 1];
            let text = prev.text.trim_end_matches(['"', '\'', '”', '’', ')', ']']);
            text.ends_with(['.', '!', '?', ';', ':'])
                || (clause_words >= 3 && text.ends_with(','))
                || word.t0.saturating_sub(prev.t1) >= CLAUSE_GAP_MS
        };
        if new_clause {
            starts.push(i);
            clause_words = 0;
        }
        clause_words += 1;
    }
    starts
}

/// Where the words split for an item. Speech isn't cut mid-clause when it can be helped: an item
/// moves back to the start of the clause in progress if that began within `CLAUSE_SNAP_MS`,
/// else forward to the clause's end if that comes within `CLAUSE_SNAP_AFTER_MS`.
fn split_point(t: u64, words: &[&WordTiming], clauses: &[usize]) -> u64 {
    let Some(inside) = words.iter().position(|w| w.t0 >= t) else {
        return t;
    };
    if clauses.contains(&inside) {
        return t.min(words[inside].t0);
    }
    let start = clauses
        .iter()
        .rev()
        .find(|&&c| c < inside)
        .copied()
        .unwrap_or(0);
    if t.saturating_sub(words[start].t0) <= CLAUSE_SNAP_MS {
        return words[start].t0;
    }
    match clauses.iter().find(|&&c| c > inside) {
        Some(&end) if words[end].t0.saturating_sub(t) <= CLAUSE_SNAP_AFTER_MS => words[end].t0,
        _ => t,
    }
}

/// Speech before the first item goes on the bullet; each mark, or run of events with no speech
/// between them, starts a line followed by the words said until the next one.
fn interleave(words: &[&WordTiming], items: &[Timed]) -> FrameText {
    let clauses = clause_starts(words);
    let mut splits: Vec<u64> = items
        .iter()
        .map(|item| split_point(item.t, words, &clauses))
        .collect();
    for i in 1..splits.len() {
        splits[i] = splits[i].max(splits[i - 1]);
    }
    let said_between = |from: Option<u64>, to: Option<u64>| -> String {
        let run: Vec<&WordTiming> = words
            .iter()
            .filter(|w| from.is_none_or(|f| w.t0 >= f) && to.is_none_or(|t| w.t0 < t))
            .copied()
            .collect();
        quoted(&join_words(&run))
    };
    let head = said_between(None, splits.first().copied());
    let mut lines: Vec<String> = Vec::new();
    let mut open: Option<String> = None;
    for (i, item) in items.iter().enumerate() {
        let is_mark = matches!(item.item, Item::Mark { .. });
        let line = match open.take() {
            Some(line) if !is_mark => format!("{line} {}", label(item)),
            previous => {
                lines.extend(previous);
                label(item)
            }
        };
        let said = said_between(Some(splits[i]), splits.get(i + 1).copied());
        if said.is_empty() && !is_mark {
            open = Some(line);
        } else if said.is_empty() {
            lines.push(line);
        } else {
            lines.push(format!("{line} {said}"));
        }
    }
    lines.extend(open);
    FrameText { head, lines }
}

/// Without word times the speech stays whole on the bullet and the items follow it.
fn interleave_unsplit(said: Option<&str>, items: &[Timed]) -> FrameText {
    FrameText {
        head: quoted(said.unwrap_or("")),
        lines: items.iter().map(label).collect(),
    }
}

/// Where a scene-change keyframe differs from the one before (autocrop's change boxes), merged
/// and largest first.
fn changed_regions(bundle: &ScrawlBundle, index: usize) -> Option<String> {
    let frame = bundle.keyframes.get(index)?;
    if frame.reason != KeyframeReason::SceneChange {
        return None;
    }
    let boxes = bundle.autocrop.as_ref()?.changes.get(index)?;
    let mut merged = merge_boxes(boxes);
    if merged.is_empty() {
        return None;
    }
    let area = |b: &[f32; 4]| (b[2] - b[0]) * (b[3] - b[1]);
    merged.sort_by(|a, b| area(b).total_cmp(&area(a)));
    if area(&merged[0]) >= 0.9 {
        return Some("the whole screen".into());
    }
    let shown: Vec<String> = merged
        .iter()
        .take(2)
        .map(|[x0, y0, x1, y1]| format!("x {x0:.2}–{x1:.2}, y {y0:.2}–{y1:.2}"))
        .collect();
    let rest = merged.len().saturating_sub(2);
    let more = if rest > 0 {
        format!(" +{rest} smaller")
    } else {
        String::new()
    };
    Some(format!("{}{more}", shown.join("; ")))
}

fn merge_boxes(boxes: &[[f32; 4]]) -> Vec<[f32; 4]> {
    const TOUCH: f32 = 0.02;
    let mut out: Vec<[f32; 4]> = boxes.to_vec();
    loop {
        let mut joined = false;
        'outer: for i in 0..out.len() {
            for j in i + 1..out.len() {
                let (a, b) = (out[i], out[j]);
                if a[0] - TOUCH <= b[2]
                    && b[0] - TOUCH <= a[2]
                    && a[1] - TOUCH <= b[3]
                    && b[1] - TOUCH <= a[3]
                {
                    out[i] = [
                        a[0].min(b[0]),
                        a[1].min(b[1]),
                        a[2].max(b[2]),
                        a[3].max(b[3]),
                    ];
                    out.remove(j);
                    joined = true;
                    break 'outer;
                }
            }
        }
        if !joined {
            return out;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::windows::WindowEntry;

    fn stroke(t0: u64, x: f32, y: f32) -> Stroke {
        let p = |v: f32| (v * SCALE as f32) as i32;
        Stroke {
            t0,
            t1: t0 + 400,
            fw: 1000,
            fh: 1000,
            on: None,
            pin: None,
            laser: false,
            pts: vec![p(x), p(y), 0, p(x + 0.05), p(y), 200, p(x + 0.1), p(y), 400],
        }
    }

    fn words(list: &[(u64, &str)]) -> Vec<WordTiming> {
        list.iter()
            .map(|&(t0, text)| WordTiming {
                t0,
                t1: t0 + 250,
                text: text.into(),
            })
            .collect()
    }

    fn keyframe(index: usize, t: u64, reason: KeyframeReason, said: &str) -> Keyframe {
        Keyframe {
            index,
            t,
            reason,
            file: format!("frames/frame-{index:02}.png"),
            event: "scene changed".into(),
            said: (!said.is_empty()).then(|| said.into()),
            mark_id: None,
            cursor: None,
        }
    }

    fn bundle(words: Vec<WordTiming>, keyframes: Vec<Keyframe>) -> ScrawlBundle {
        ScrawlBundle {
            version: 1,
            id: "t".into(),
            created_at_ms: 0,
            duration_ms: 20_000,
            target: TargetInfo {
                mode: CaptureMode::Display,
                app: None,
                bundle_id: None,
                window_title: None,
                window_id: None,
                display_id: 1,
                frame: TargetFrame {
                    x: 0.0,
                    y: 0.0,
                    width: 1000.0,
                    height: 1000.0,
                    scale: 1.0,
                },
            },
            transcript: TranscriptData {
                source: "test".into(),
                words,
                ..Default::default()
            },
            note: None,
            strokes: Vec::new(),
            marks: Vec::new(),
            events: Vec::new(),
            keyframes,
            contact_sheet: "contact-sheet.png".into(),
            video: None,
            video_offset_ms: 0,
            audio: None,
            source: None,
            device: None,
            context: Default::default(),
            context_changes: Vec::new(),
            screens: Vec::new(),
            windows: Vec::new(),
            cursor: Vec::new(),
            autocrop: None,
            destination: None,
        }
    }

    #[test]
    fn marks_sharing_a_frame_are_numbered_and_take_the_speech_drawn_over() {
        let said =
            "These should stay put. Maybe this hover part can move. And drop these dividers.";
        let mut b = bundle(
            words(&[
                (1_000, "These"),
                (1_300, "should"),
                (1_600, "stay"),
                (1_900, "put."),
                (4_000, "Maybe"),
                (4_300, "this"),
                (4_600, "hover"),
                (4_900, "part"),
                (5_200, "can"),
                (5_500, "move."),
                (8_000, "And"),
                (8_300, "drop"),
                (8_600, "these"),
                (8_900, "dividers."),
            ]),
            vec![keyframe(1, 9_500, KeyframeReason::Mark, said)],
        );
        // The second mark starts mid-clause ("hover"), so its speech starts with "Maybe".
        b.strokes = vec![
            stroke(1_200, 0.1, 0.2),
            stroke(4_700, 0.4, 0.5),
            stroke(8_400, 0.6, 0.1),
        ];
        b.marks = crate::sketch::marks_of(&b.strokes);
        for mark in &mut b.marks {
            mark.frame = Some("frames/frame-01.png".into());
        }
        b.keyframes[0].mark_id = Some(b.marks[0].id.clone());
        let text = &keyframe_texts(&b)[0];
        assert_eq!(text.head, "");
        assert_eq!(text.lines.len(), 3, "{text:?}");
        assert!(text.lines[0].starts_with("✎① 0:01 "));
        assert!(text.lines[0].ends_with("“These should stay put.”"));
        assert!(text.lines[1].starts_with("✎② 0:04 "));
        assert!(text.lines[1].ends_with("“Maybe this hover part can move.”"));
        assert!(text.lines[2].starts_with("✎③ 0:08 "));
        assert!(text.lines[2].ends_with("“And drop these dividers.”"));
    }

    #[test]
    fn shortcuts_drags_and_windows_appear_inline_without_titles_or_jitter() {
        let said = "Okay, I hit command shift B. The whole window closes.";
        let mut b = bundle(
            words(&[
                (500, "Okay,"),
                (900, "I"),
                (1_100, "hit"),
                (1_300, "command"),
                (1_600, "shift"),
                (1_900, "B."),
                (5_000, "The"),
                (5_200, "whole"),
                (5_400, "window"),
                (5_600, "closes."),
            ]),
            vec![keyframe(1, 1_000, KeyframeReason::Click, said)],
        );
        let key = |t| InteractionEvent::Key {
            t,
            shortcut: "⇧⌘B".into(),
        };
        let drag = |t, x1: f32| InteractionEvent::Drag {
            t,
            t1: t + 300,
            x0: 0.5,
            y0: 0.5,
            x1,
            y1: 0.5,
            label: None,
            button: None,
            path: Vec::new(),
        };
        b.events = vec![
            key(2_000),
            key(2_400),
            key(2_900),
            drag(3_000, 0.503),
            drag(3_500, 0.8),
        ];
        let window = |id, title: &str| WindowEntry {
            id,
            app: "OpenCode".into(),
            bundle_id: None,
            regular: true,
            title: title.into(),
            bounds: [0, 0, 100, 100],
            layer: 0,
            frontmost: true,
        };
        let overlay = WindowEntry {
            layer: 25,
            ..window(9, "Hex")
        };
        b.windows = vec![
            WindowSnapshot {
                t: 0,
                windows: vec![window(1, "Secret plans"), overlay],
            },
            WindowSnapshot {
                t: 4_800,
                windows: Vec::new(),
            },
        ];
        let text = &keyframe_texts(&b)[0];
        let all = format!("{}\n{}", text.head, text.lines.join("\n"));
        assert!(all.contains("pressed ⇧⌘B ×3"), "{all}");
        assert!(all.contains("0:03 dragged right"), "{all}");
        assert_eq!(
            all.matches("dragged").count(),
            1,
            "jitter is no drag: {all}"
        );
        assert!(
            all.contains("· 0:04 OpenCode window closed “The whole window closes.”"),
            "{all}"
        );
        assert!(!all.contains("Secret") && !all.contains("Hex"), "{all}");
    }

    #[test]
    fn scene_changes_say_where_the_screen_changed() {
        let mut b = bundle(
            Vec::new(),
            vec![
                keyframe(1, 1_000, KeyframeReason::SceneChange, ""),
                keyframe(2, 3_000, KeyframeReason::SceneChange, ""),
                keyframe(3, 5_000, KeyframeReason::Click, ""),
            ],
        );
        b.autocrop = Some(AutocropInfo {
            changes: vec![
                vec![[0.0, 0.0, 1.0, 0.95]],
                vec![
                    [0.70, 0.02, 0.80, 0.10],
                    [0.80, 0.02, 0.96, 0.12],
                    [0.10, 0.50, 0.12, 0.52],
                ],
                vec![[0.0, 0.0, 0.5, 0.5]],
            ],
            ..Default::default()
        });
        let texts = keyframe_texts(&b);
        assert_eq!(texts[0].head, "[changed: the whole screen]");
        assert_eq!(
            texts[1].head,
            "[changed: x 0.70–0.96, y 0.02–0.12; x 0.10–0.12, y 0.50–0.52]"
        );
        assert_eq!(
            texts[2].head, "scene changed",
            "clicks don't describe changes"
        );
    }

    #[test]
    fn speech_without_word_times_stays_whole() {
        let mut b = bundle(
            Vec::new(),
            vec![keyframe(
                1,
                1_000,
                KeyframeReason::Click,
                "Make it smaller.",
            )],
        );
        b.events = vec![InteractionEvent::Key {
            t: 600,
            shortcut: "⌘K".into(),
        }];
        let text = &keyframe_texts(&b)[0];
        assert_eq!(text.head, "“Make it smaller.”");
        assert_eq!(text.lines, vec!["· 0:00 pressed ⌘K".to_owned()]);
    }

    #[test]
    fn the_summary_uses_these_lines_only_with_the_experiment() {
        let mut b = bundle(
            words(&[(500, "Look"), (800, "here.")]),
            vec![keyframe(1, 1_000, KeyframeReason::Click, "Look here.")],
        );
        b.events = vec![InteractionEvent::Key {
            t: 400,
            shortcut: "⌘K".into(),
        }];
        let dir = std::path::Path::new("/b");
        let off = crate::sketch::summary_markdown_with(&b, dir, false);
        let on = crate::sketch::summary_markdown_with(&b, dir, true);
        assert!(!off.contains("⌘K"), "{off}");
        assert!(
            on.contains("- #1 (0:01)\n  · 0:00 pressed ⌘K “Look here.”\n"),
            "{on}"
        );
    }
}
