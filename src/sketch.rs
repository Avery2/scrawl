use crate::model::*;
use std::f32::consts::PI;

fn bounds(points: &[[f32; 2]]) -> [f32; 4] {
    let Some(&[x, y]) = points.first() else {
        return [0.0; 4];
    };
    points.iter().fold([x, y, x, y], |b, p| {
        [
            b[0].min(p[0]),
            b[1].min(p[1]),
            b[2].max(p[0]),
            b[3].max(p[1]),
        ]
    })
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}

pub fn shape_of(stroke: &Stroke) -> Shape {
    if stroke.pin.is_some() {
        return Shape::Pin;
    }
    let points: Vec<_> = stroke
        .normalized_points()
        .into_iter()
        .map(|p| [p[0] * stroke.fw as f32, p[1] * stroke.fh as f32])
        .collect();
    let Some(&first) = points.first() else {
        return Shape::Tap;
    };
    let b = bounds(&points);
    let (w, h) = (b[2] - b[0], b[3] - b[1]);
    let big = w.max(h);
    let len: f32 = points.windows(2).map(|p| distance(p[0], p[1])).sum();
    let chord = distance(first, *points.last().unwrap());
    if w.hypot(h) < 14.0 {
        Shape::Tap
    } else if chord < 0.35 * big && len > 1.85 * big && w.min(h) > 0.2 * big {
        Shape::Circle
    } else if chord > 0.88 * len {
        if w > 2.5 * h {
            Shape::Underline
        } else {
            Shape::Line
        }
    } else {
        Shape::Mark
    }
}

pub fn marks_of(strokes: &[Stroke]) -> Vec<Mark> {
    let mut groups: Vec<Vec<usize>> = Vec::new();
    for (i, stroke) in strokes.iter().enumerate().filter(|(_, s)| !s.laser) {
        if let Some(group) = groups.last_mut() {
            let prev = &strokes[*group.last().unwrap()];
            if prev.pin.is_none()
                && stroke.pin.is_none()
                && stroke.t0.saturating_sub(prev.t1) <= MARK_GAP_MS
                && group.len() < MAX_MARK_STROKES
            {
                group.push(i);
                continue;
            }
        }
        groups.push(vec![i]);
    }
    groups
        .into_iter()
        .enumerate()
        .map(|(i, group)| {
            let first = &strokes[group[0]];
            let all: Vec<_> = group
                .iter()
                .flat_map(|&j| strokes[j].normalized_points())
                .collect();
            let mut mark = Mark {
                id: format!("m{}", i + 1),
                t0: first.t0,
                t1: group
                    .iter()
                    .map(|&j| strokes[j].t1)
                    .max()
                    .unwrap_or(first.t1),
                box_norm: bounds(&all),
                shape: Shape::Mark,
                from: None,
                to: None,
                pin: first.pin,
                on: group.iter().find_map(|&j| strokes[j].on.clone()),
                strokes: group.clone(),
                description: String::new(),
                frame: None,
            };
            if first.pin.is_some() {
                let [x, y] = first
                    .normalized_points()
                    .first()
                    .copied()
                    .unwrap_or([0.0; 2]);
                mark.box_norm = [x, y, x, y];
                mark.shape = Shape::Pin;
            } else {
                let mut sized: Vec<_> = group
                    .iter()
                    .map(|&j| {
                        let s = &strokes[j];
                        let pts = s.normalized_points();
                        let b = bounds(&pts);
                        let size = (b[2] - b[0]) * s.fw as f32 + (b[3] - b[1]) * s.fh as f32;
                        (s, shape_of(s), b, pts, size)
                    })
                    .collect();
                sized.sort_by(|a, b| b.4.total_cmp(&a.4));
                let (main, shape, b, pts, _) = &sized[0];
                let rest = &sized[1..];
                let a = pts.first().copied().unwrap_or([0.0; 2]);
                let z = pts.last().copied().unwrap_or(a);
                if matches!(shape, Shape::Line | Shape::Underline) && (1..=2).contains(&rest.len())
                {
                    let px = |p: [f32; 2]| [p[0] * main.fw as f32, p[1] * main.fh as f32];
                    let len = distance(px(a), px(z));
                    let near = |p| {
                        rest.iter().all(|r| {
                            distance(
                                px([(r.2[0] + r.2[2]) / 2.0, (r.2[1] + r.2[3]) / 2.0]),
                                px(p),
                            ) < 0.35 * len
                        })
                    };
                    if near(z) || near(a) {
                        mark.shape = Shape::Arrow;
                        mark.from = Some(if near(z) { a } else { z });
                        mark.to = Some(if near(z) { z } else { a });
                    }
                }
                if mark.shape != Shape::Arrow
                    && (rest.iter().all(|r| r.1 == *shape) || *shape == Shape::Circle)
                {
                    match shape {
                        Shape::Circle => {
                            mark.shape = Shape::Circle;
                            if !rest.iter().all(|r| r.1 == Shape::Circle) {
                                mark.box_norm = *b;
                            }
                        }
                        Shape::Line if rest.is_empty() => {
                            mark.shape = Shape::Line;
                            mark.from = Some(a);
                            mark.to = Some(z);
                        }
                        Shape::Tap | Shape::Underline => mark.shape = *shape,
                        _ => {}
                    }
                }
            }
            mark.description = describe_mark(&mark);
            mark
        })
        .collect()
}

fn at([x, y]: [f32; 2]) -> String {
    format!("x {x:.2}, y {y:.2}")
}

pub fn describe_mark(mark: &Mark) -> String {
    let [x0, y0, x1, y1] = mark.box_norm;
    let region = format!("x {x0:.2}–{x1:.2}, y {y0:.2}–{y1:.2}");
    let many = if mark.strokes.len() > 1 {
        format!(" ({} strokes)", mark.strokes.len())
    } else {
        String::new()
    };
    let mut text = match mark.shape {
        Shape::Circle => format!("circled region {region}{many}"),
        Shape::Underline => format!(
            "underlined x {x0:.2}–{x1:.2} at y {:.2}{many}",
            (y0 + y1) / 2.0
        ),
        Shape::Line => format!(
            "drew a line from {} to {}",
            at(mark.from.unwrap_or([x0, y0])),
            at(mark.to.unwrap_or([x1, y1]))
        ),
        Shape::Arrow => format!(
            "drew an arrow from {} pointing to {}",
            at(mark.from.unwrap_or([x0, y0])),
            at(mark.to.unwrap_or([x1, y1]))
        ),
        Shape::Pin => format!("pin {} at {}", mark.pin.unwrap_or(1), at([x0, y0])),
        Shape::Tap => format!("tapped {}{many}", at([(x0 + x1) / 2.0, (y0 + y1) / 2.0])),
        Shape::Mark => format!("marked region {region}{many}"),
    };
    if let Some(on) = &mark.on {
        text.push_str(&format!(" over “{on}”"));
    }
    text
}

/// Coalesce each label independently, even when other events occur during a burst.
pub fn settled_events(events: &[InteractionEvent]) -> Vec<InteractionEvent> {
    let mut sorted = events.to_vec();
    sorted.sort_by_key(InteractionEvent::timestamp_ms);
    let mut out: Vec<InteractionEvent> = Vec::new();
    for mut event in sorted {
        let prior = out.iter().rposition(|prev| match (prev, &event) {
            (
                InteractionEvent::Scroll { t: a, label: x, .. },
                InteractionEvent::Scroll { t: b, label: y, .. },
            ) => x == y && b.saturating_sub(*a) < 1500,
            (
                InteractionEvent::Point { t: a, label: x, .. },
                InteractionEvent::Point { t: b, label: y, .. },
            ) => x == y && b.saturating_sub(*a) < 1000,
            _ => false,
        });
        if let Some(i) = prior {
            if let (
                InteractionEvent::Scroll { dx, dy, .. },
                InteractionEvent::Scroll { dx: px, dy: py, .. },
            ) = (&mut event, &out[i])
            {
                *dx += px;
                *dy += py;
            }
            out.remove(i);
        }
        out.push(event);
    }
    out
}

fn direction(dx: f32, dy: f32, threshold: f32) -> String {
    let h = if dx.abs() > threshold {
        if dx > 0.0 { "right" } else { "left" }
    } else {
        ""
    };
    let v = if dy.abs() > threshold {
        if dy > 0.0 { "down" } else { "up" }
    } else {
        ""
    };
    let parts = if dx.abs() > 2.0 * dy.abs() {
        vec![h]
    } else if dy.abs() > 2.0 * dx.abs() {
        vec![v]
    } else {
        vec![v, h]
    };
    let text = parts
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" and ");
    if text.is_empty() {
        "a little".into()
    } else {
        text
    }
}

pub fn describe_event(event: &InteractionEvent) -> String {
    match event {
        InteractionEvent::Click { x, y, label, .. } => format!(
            "clicked “{}” ({})",
            label.as_deref().unwrap_or("something"),
            at([*x, *y])
        ),
        InteractionEvent::Drag {
            x0,
            y0,
            x1,
            y1,
            label,
            ..
        } => format!(
            "dragged{} {} ({} → {})",
            label
                .as_ref()
                .map(|s| format!(" “{s}”"))
                .unwrap_or_default(),
            direction(x1 - x0, y1 - y0, 0.04),
            at([*x0, *y0]),
            at([*x1, *y1])
        ),
        InteractionEvent::Scroll { dx, dy, label, .. } => format!(
            "scrolled “{}” {}",
            label.as_deref().unwrap_or("the page"),
            direction(*dx, *dy, 0.0)
        ),
        InteractionEvent::Point { x, y, label, .. } => format!(
            "pointed at “{}” ({})",
            label.as_deref().unwrap_or("something"),
            at([*x, *y])
        ),
        InteractionEvent::Key { shortcut, .. } => format!("pressed {shortcut}"),
        InteractionEvent::AppChange {
            app, window_title, ..
        } => format!(
            "switched to {app}{}",
            window_title
                .as_ref()
                .map(|s| format!(" — “{s}”"))
                .unwrap_or_default()
        ),
    }
}

pub fn timeline(bundle: &ScrawlBundle) -> Vec<String> {
    let mut items: Vec<(u64, String)> = bundle
        .transcript
        .segments
        .iter()
        .map(|s| (s.t0, format!("{} “{}”", format_clock(s.t0), s.text)))
        .collect();
    for mark in &bundle.marks {
        let frame = bundle.keyframes.iter().find(|k| {
            k.mark_id.as_deref() == Some(mark.id.as_str())
                || mark.frame.as_deref() == Some(k.file.as_str())
        });
        let suffix = frame
            .map(|k| format!(" — frame #{}", k.index))
            .unwrap_or_default();
        items.push((
            mark.t0,
            format!(
                "{} ✎ {}{suffix}",
                format_clock(mark.t0),
                describe_mark(mark)
            ),
        ));
    }
    for event in settled_events(&bundle.events) {
        items.push((
            event.timestamp_ms(),
            format!(
                "{} · {}",
                format_clock(event.timestamp_ms()),
                describe_event(&event)
            ),
        ));
    }
    items.sort_by_key(|item| item.0);
    items.into_iter().map(|item| item.1).collect()
}

pub fn summary_markdown(bundle: &ScrawlBundle, bundle_dir: Option<&std::path::Path>) -> String {
    let mut target = bundle
        .target
        .app
        .clone()
        .unwrap_or_else(|| "Display".into());
    if let Some(title) = &bundle.target.window_title {
        target.push_str(&format!(" “{title}”"));
    }
    let mut out = format!("[scrawl {} · {target}]\n", format_clock(bundle.duration_ms));
    if let Some(note) = &bundle.note {
        if !note.trim().is_empty() {
            out.push_str(&format!("\nNote: {note}\n"));
        }
    }
    if !bundle.transcript.text.trim().is_empty() {
        out.push_str(&format!("\nSaid: {}\n", bundle.transcript.text));
    }
    out.push_str("\nTimeline:\n");
    for line in timeline(bundle) {
        out.push_str(&format!("- {line}\n"));
    }
    out.push_str("\nKeyframes:\n");
    for frame in &bundle.keyframes {
        out.push_str(&format!(
            "- #{} {} — {} ({})\n",
            frame.index,
            format_clock(frame.t),
            frame.event,
            frame.file
        ));
        if let Some(said) = &frame.said {
            out.push_str(&format!("  Said: {said}\n"));
        }
    }
    let path = bundle_dir
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| ".".into());
    out.push_str(&format!("\nBundle: {path}\n"));
    out
}

#[derive(Clone, Copy, Debug)]
pub struct FreehandOptions {
    pub size: f32,
    pub thinning: f32,
    pub smoothing: f32,
    pub streamline: f32,
    pub simulate_pressure: bool,
    pub taper_start: bool,
    pub taper_end: bool,
}

impl Default for FreehandOptions {
    fn default() -> Self {
        Self {
            size: 16.0,
            thinning: 0.5,
            smoothing: 0.5,
            streamline: 0.5,
            simulate_pressure: true,
            taper_start: false,
            taper_end: false,
        }
    }
}

fn add(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] + b[0], a[1] + b[1]]
}
fn sub(a: [f32; 2], b: [f32; 2]) -> [f32; 2] {
    [a[0] - b[0], a[1] - b[1]]
}
fn mul(a: [f32; 2], n: f32) -> [f32; 2] {
    [a[0] * n, a[1] * n]
}
fn lerp(a: [f32; 2], b: [f32; 2], t: f32) -> [f32; 2] {
    add(a, mul(sub(b, a), t))
}
fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}
fn normal(a: [f32; 2]) -> [f32; 2] {
    [a[1], -a[0]]
}
fn rotate(point: [f32; 2], center: [f32; 2], angle: f32) -> [f32; 2] {
    let p = sub(point, center);
    let (sin, cos) = angle.sin_cos();
    add(center, [p[0] * cos - p[1] * sin, p[0] * sin + p[1] * cos])
}

struct StrokePoint {
    point: [f32; 2],
    vector: [f32; 2],
    distance: f32,
    length: f32,
}

/// Completed-stroke variant of perfect-freehand's streamline/pressure/outline pipeline.
/// Adapted from perfect-freehand 1.2.2, Copyright (c) 2021 Stephen Ruiz Ltd.
///
/// MIT License: Permission is hereby granted, free of charge, to any person obtaining
/// a copy of this software and associated documentation files (the "Software"), to
/// deal in the Software without restriction, including without limitation the rights
/// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell copies of
/// the Software, and to permit persons to whom the Software is furnished to do so,
/// subject to the following conditions: The above copyright notice and this
/// permission notice shall be included in all copies or substantial portions of the
/// Software. THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS
/// OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
/// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
/// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY,
/// WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM, OUT OF OR IN
/// CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
pub fn freehand_outline(points: &[[f32; 2]], options: &FreehandOptions) -> Vec<[f32; 2]> {
    if !options.size.is_finite() || options.size <= 0.0 {
        return Vec::new();
    }
    let mut input: Vec<_> = points
        .iter()
        .copied()
        .filter(|p| p.iter().all(|v| v.is_finite()))
        .collect();
    input.dedup();
    if input.is_empty() {
        return Vec::new();
    }
    let size = options.size;
    let thinning = options.thinning.clamp(-1.0, 1.0);
    let radius = |pressure: f32| size * (0.5 - thinning * (0.5 - pressure));
    if input.len() == 1 {
        let r = radius(0.25).max(0.01);
        let center = input[0];
        let mut circle: Vec<_> = (0..32)
            .map(|i| {
                let angle = 2.0 * PI * i as f32 / 32.0;
                add(center, [r * angle.cos(), r * angle.sin()])
            })
            .collect();
        circle.push(circle[0]);
        return circle;
    }
    if input.len() == 2 {
        let (a, b) = (input[0], input[1]);
        input = (0..=4).map(|i| lerp(a, b, i as f32 / 4.0)).collect();
    }
    let streamline = 0.15 + (1.0 - options.streamline.clamp(0.0, 1.0)) * 0.85;
    let mut samples = vec![StrokePoint {
        point: input[0],
        vector: [0.0; 2],
        distance: 0.0,
        length: 0.0,
    }];
    let mut length = 0.0;
    let mut has_minimum_length = false;
    for (i, &p) in input.iter().enumerate().skip(1) {
        let prev = samples.last().unwrap().point;
        let point = if i == input.len() - 1 {
            p
        } else {
            lerp(prev, p, streamline)
        };
        let d = distance(prev, point);
        if d <= f32::EPSILON {
            continue;
        }
        length += d;
        if i < input.len() - 1 && !has_minimum_length {
            if length < size {
                continue;
            }
            has_minimum_length = true;
        }
        samples.push(StrokePoint {
            point,
            vector: mul(sub(prev, point), d.recip()),
            distance: d,
            length,
        });
    }
    if samples.len() == 1 {
        return freehand_outline(&[samples[0].point], options);
    }
    samples[0].vector = samples[1].vector;
    let simulate = |pressure: f32, d: f32| {
        let speed = (d / size).min(1.0);
        (pressure + (1.0 - speed - pressure) * (speed * 0.275)).min(1.0)
    };
    let mut pressure = 0.25;
    for (i, sample) in samples.iter().take(10).enumerate() {
        let next = if options.simulate_pressure {
            simulate(pressure, sample.distance)
        } else if i == 0 {
            0.25
        } else {
            0.5
        };
        pressure = (pressure + next) / 2.0;
    }
    let min_distance = (size * options.smoothing.clamp(0.0, 1.0)).powi(2);
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut prev_vector = samples[0].vector;
    let mut prev_left = samples[0].point;
    let mut prev_right = prev_left;
    let mut sharp = false;
    let mut end_radius = radius(pressure);
    let taper_length = size.max(length);
    for (i, sample) in samples.iter().enumerate() {
        let last = i == samples.len() - 1;
        if !last && length - sample.length < 3.0 {
            continue;
        }
        let p = if options.simulate_pressure {
            simulate(pressure, sample.distance)
        } else if i == 0 {
            0.25
        } else {
            0.5
        };
        let start_t = (sample.length / taper_length).min(1.0);
        let end_t = ((length - sample.length) / taper_length).min(1.0);
        let start = if options.taper_start {
            start_t * (2.0 - start_t)
        } else {
            1.0
        };
        let end = if options.taper_end {
            1.0 - (1.0 - end_t).powi(3)
        } else {
            1.0
        };
        let r = (radius(p) * start.min(end)).max(0.01);
        end_radius = r;
        let next = samples.get(i + 1).unwrap_or(sample).vector;
        let turn = dot(sample.vector, next);
        let sharp_next = turn < 0.0;
        if (dot(sample.vector, prev_vector) < 0.0 && !sharp) || sharp_next {
            let offset = mul(normal(prev_vector), r);
            for step in 0..=13 {
                let angle = PI * step as f32 / 13.0;
                prev_left = rotate(sub(sample.point, offset), sample.point, angle);
                prev_right = rotate(add(sample.point, offset), sample.point, -angle);
                left.push(prev_left);
                right.push(prev_right);
            }
            sharp = sharp_next;
            continue;
        }
        sharp = false;
        let offset = mul(
            normal(if last {
                sample.vector
            } else {
                lerp(next, sample.vector, turn)
            }),
            r,
        );
        let l = sub(sample.point, offset);
        let r = add(sample.point, offset);
        if last || i <= 1 || dot(sub(prev_left, l), sub(prev_left, l)) > min_distance {
            left.push(l);
            prev_left = l;
        }
        if last || i <= 1 || dot(sub(prev_right, r), sub(prev_right, r)) > min_distance {
            right.push(r);
            prev_right = r;
        }
        pressure = p;
        prev_vector = sample.vector;
    }
    let end = samples.last().unwrap().point;
    if options.taper_end {
        left.push(end);
    } else {
        let cap = add(
            end,
            mul(
                normal(mul(samples.last().unwrap().vector, -1.0)),
                end_radius,
            ),
        );
        for step in 1..29 {
            left.push(rotate(cap, end, PI * 3.0 * step as f32 / 29.0));
        }
    }
    let start_right = right.first().copied();
    left.extend(right.into_iter().rev());
    if options.taper_start {
        left.push(samples[0].point);
    } else if let Some(p) = start_right {
        for step in 1..=13 {
            left.push(rotate(p, samples[0].point, PI * step as f32 / 13.0));
        }
    }
    if let Some(&first) = left.first() {
        left.push(first);
    }
    left
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(points: &[[f32; 2]], t: u64) -> Stroke {
        Stroke {
            t0: t,
            t1: t + 100,
            fw: 1000,
            fh: 1000,
            on: None,
            pin: None,
            laser: false,
            pts: points
                .iter()
                .flat_map(|p| {
                    [
                        (p[0] * SCALE as f32).round() as i32,
                        (p[1] * SCALE as f32).round() as i32,
                        0,
                    ]
                })
                .collect(),
        }
    }

    #[test]
    fn shapes_are_measured_in_pixels() {
        assert_eq!(
            shape_of(&stroke(&[[0.5, 0.5], [0.505, 0.505]], 0)),
            Shape::Tap
        );
        assert_eq!(
            shape_of(&stroke(&[[0.1, 0.5], [0.8, 0.5]], 0)),
            Shape::Underline
        );
        assert_eq!(shape_of(&stroke(&[[0.2, 0.2], [0.5, 0.6]], 0)), Shape::Line);
        let circle: Vec<_> = (0..=32)
            .map(|i| {
                let a = 2.0 * PI * i as f32 / 32.0;
                [0.5 + 0.1 * a.cos(), 0.5 + 0.2 * a.sin()]
            })
            .collect();
        let mut s = stroke(&circle, 0);
        s.fh = 500;
        assert_eq!(shape_of(&s), Shape::Circle);
        s.pin = Some(1);
        assert_eq!(shape_of(&s), Shape::Pin);
    }

    #[test]
    fn arrow_points_toward_its_head_in_either_drawing_direction() {
        for shaft in [[[0.2, 0.8], [0.5, 0.5]], [[0.5, 0.5], [0.2, 0.8]]] {
            let marks = marks_of(&[
                stroke(&shaft, 0),
                stroke(&[[0.42, 0.5], [0.5, 0.5], [0.5, 0.58]], 200),
            ]);
            assert_eq!(marks.len(), 1);
            assert_eq!(marks[0].shape, Shape::Arrow);
            assert_eq!(marks[0].from, Some([0.2, 0.8]));
            assert_eq!(marks[0].to, Some([0.5, 0.5]));
            assert_eq!(
                describe_mark(&marks[0]),
                "drew an arrow from x 0.20, y 0.80 pointing to x 0.50, y 0.50"
            );
        }
    }

    #[test]
    fn pins_lasers_and_group_limits() {
        let mut strokes: Vec<_> = (0..8).map(|i| stroke(&[[0.5, 0.5]], i * 200)).collect();
        strokes[1].laser = true;
        let mut pin = stroke(&[[0.25, 0.4]], 1600);
        pin.pin = Some(1);
        strokes.push(pin);
        strokes.push(stroke(&[[0.5, 0.5]], 1700));
        strokes.push(stroke(&[[0.5, 0.5]], 4000));
        let marks = marks_of(&strokes);
        assert_eq!(
            marks.iter().map(|m| m.strokes.len()).collect::<Vec<_>>(),
            vec![6, 1, 1, 1, 1]
        );
        assert_eq!(marks[0].strokes, vec![0, 2, 3, 4, 5, 6]);
        assert_eq!(marks[2].shape, Shape::Pin);
        assert_eq!(marks[2].id, "m3");
        assert_eq!(describe_mark(&marks[2]), "pin 1 at x 0.25, y 0.40");
    }

    #[test]
    fn descriptions_include_group_count_and_label() {
        let mut strokes = vec![
            stroke(&[[0.1, 0.5], [0.8, 0.5]], 0),
            stroke(&[[0.2, 0.5], [0.7, 0.5]], 200),
        ];
        strokes[1].on = Some("Header".into());
        assert_eq!(
            marks_of(&strokes)[0].description,
            "underlined x 0.10–0.80 at y 0.50 (2 strokes) over “Header”"
        );
    }

    #[test]
    fn debounce_accumulates_scrolls_across_other_events() {
        let scroll = |t, dy| InteractionEvent::Scroll {
            t,
            x: 0.5,
            y: 0.5,
            dx: 0.0,
            dy,
            label: Some("Transcript".into()),
        };
        let point = |t| InteractionEvent::Point {
            t,
            x: 0.3,
            y: 0.12,
            label: Some("Header".into()),
        };
        let events = settled_events(&[
            scroll(0, 10.0),
            point(100),
            scroll(1000, 20.0),
            point(800),
            scroll(2600, -5.0),
        ]);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].timestamp_ms(), 800);
        assert!(matches!(
            events[1],
            InteractionEvent::Scroll {
                t: 1000,
                dy: 30.0,
                ..
            }
        ));
        assert_eq!(describe_event(&events[1]), "scrolled “Transcript” down");
        assert_eq!(
            describe_event(&events[0]),
            "pointed at “Header” (x 0.30, y 0.12)"
        );
    }

    #[test]
    fn outline_is_closed_finite_and_has_caps_and_taper() {
        let options = FreehandOptions {
            size: 10.0,
            thinning: 0.0,
            streamline: 0.0,
            ..Default::default()
        };
        assert!(freehand_outline(&[], &options).is_empty());
        for points in [
            vec![[0.0, 0.0]],
            vec![[0.0, 0.0]; 4],
            vec![[0.0, 0.0], [100.0, 0.0]],
            vec![[0.0, 0.0], [50.0, 0.0], [1.0, 1.0]],
        ] {
            let outline = freehand_outline(&points, &options);
            assert!(outline.len() > 10);
            assert_eq!(outline.first(), outline.last());
            assert!(outline.iter().flatten().all(|n| n.is_finite()));
        }
        let line = [[0.0, 0.0], [100.0, 0.0]];
        let plain = freehand_outline(&line, &options);
        let b = bounds(&plain);
        assert!(b[0] < -4.0 && b[2] > 104.0 && b[1] <= -4.9 && b[3] >= 4.9);
        let tapered = freehand_outline(
            &line,
            &FreehandOptions {
                taper_start: true,
                taper_end: true,
                ..options
            },
        );
        assert!(tapered[0][1].abs() < 0.1);
        assert!(tapered.contains(&[100.0, 0.0]));
    }

    #[test]
    fn summary_interleaves_speech_marks_and_events() {
        let strokes = vec![stroke(&[[0.5, 0.5]], 5000)];
        let mut marks = marks_of(&strokes);
        marks[0].frame = Some("frames/frame-01.png".into());
        let bundle = ScrawlBundle {
            version: 1,
            id: "test".into(),
            created_at_ms: 0,
            duration_ms: 14000,
            target: TargetInfo {
                mode: CaptureMode::Window,
                app: Some("App".into()),
                bundle_id: None,
                window_title: Some("Window".into()),
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
                text: "This row is too dense.".into(),
                segments: vec![TranscriptSegment {
                    t0: 3000,
                    t1: 4500,
                    text: "This row is too dense.".into(),
                }],
                ..Default::default()
            },
            note: Some("Simplify it.".into()),
            strokes,
            marks,
            events: vec![InteractionEvent::Key {
                t: 4000,
                shortcut: "⌘K".into(),
            }],
            keyframes: vec![Keyframe {
                index: 1,
                t: 5100,
                reason: KeyframeReason::Mark,
                file: "frames/frame-01.png".into(),
                event: "tapped x 0.50, y 0.50".into(),
                said: None,
                mark_id: Some("m1".into()),
                cursor: None,
            }],
            contact_sheet: "contact.png".into(),
            video: None,
            audio: None,
        };
        let summary = summary_markdown(&bundle, Some(std::path::Path::new("/recordings/test")));
        assert!(summary.starts_with("[scrawl 0:14 · App “Window”]"));
        assert!(summary.contains("Note: Simplify it."));
        assert!(summary.contains("Said: This row is too dense."));
        assert!(summary.find("0:03 “").unwrap() < summary.find("0:04 · pressed").unwrap());
        assert!(summary.find("0:04 · pressed").unwrap() < summary.find("0:05 ✎").unwrap());
        assert!(summary.contains("— frame #1"));
        assert!(summary.contains("Bundle: /recordings/test"));
    }
}
