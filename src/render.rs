//! PNG output and deterministic, CPU-rendered review images.
use crate::model::*;
use crate::sketch::{FreehandOptions, freehand_outline};
#[cfg(test)]
use std::io::BufReader;
use std::path::Path;

type Color = [u8; 4];
const RED: Color = [255, 59, 48, 255];
const WHITE: Color = [255, 255, 255, 255];
const TEXT: Color = [242, 242, 242, 255];
const MUTED: Color = [150, 150, 150, 255];

pub fn encode_png(frame: &RgbaFrame) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(
        frame.width > 0 && frame.height > 0,
        "PNG dimensions must be nonzero"
    );
    let expected = (frame.width as usize)
        .checked_mul(frame.height as usize)
        .and_then(|n| n.checked_mul(4));
    anyhow::ensure!(
        expected == Some(frame.rgba.len()),
        "Invalid RGBA buffer length"
    );
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, frame.width, frame.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&frame.rgba)?;
        writer.finish()?;
    }
    Ok(bytes)
}

pub fn save_png(frame: &RgbaFrame, path: &Path) -> anyhow::Result<()> {
    std::fs::write(path, encode_png(frame)?)?;
    Ok(())
}

#[cfg(test)]
pub fn load_png(path: &Path) -> anyhow::Result<RgbaFrame> {
    decode_png(BufReader::new(std::fs::File::open(path)?))
}

#[cfg(test)]
fn decode_png<R: std::io::BufRead + std::io::Seek>(source: R) -> anyhow::Result<RgbaFrame> {
    let mut decoder = png::Decoder::new(source);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info()?;
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| anyhow::anyhow!("PNG is too large"))?;
    let mut data = vec![0; size];
    let info = reader.next_frame(&mut data)?;
    anyhow::ensure!(
        info.bit_depth == png::BitDepth::Eight,
        "Unsupported PNG bit depth"
    );
    let data = &data[..info.buffer_size()];
    let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(data),
        png::ColorType::Rgb => {
            for p in data.chunks_exact(3) {
                rgba.extend_from_slice(&[p[0], p[1], p[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for &v in data {
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for p in data.chunks_exact(2) {
                rgba.extend_from_slice(&[p[0], p[0], p[0], p[1]]);
            }
        }
        png::ColorType::Indexed => anyhow::bail!("PNG palette was not expanded"),
    }
    Ok(RgbaFrame::new(0, info.width, info.height, rgba))
}

fn solid(width: u32, height: u32, color: Color) -> RgbaFrame {
    let mut rgba = vec![0; width as usize * height as usize * 4];
    for p in rgba.chunks_exact_mut(4) {
        p.copy_from_slice(&color);
    }
    RgbaFrame::new(0, width, height, rgba)
}

/// Source-over, retaining straight alpha even when the input is transparent.
fn blend(frame: &mut RgbaFrame, x: i32, y: i32, color: Color, coverage: f32) {
    if x < 0 || y < 0 || x >= frame.width as i32 || y >= frame.height as i32 {
        return;
    }
    let i = (y as usize * frame.width as usize + x as usize) * 4;
    let Some(dst) = frame.rgba.get_mut(i..i + 4) else {
        return;
    };
    let a = (color[3] as f32 / 255.0 * coverage).clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let da = dst[3] as f32 / 255.0;
    let out = a + da * (1.0 - a);
    for c in 0..3 {
        dst[c] = ((color[c] as f32 * a + dst[c] as f32 * da * (1.0 - a)) / out).round() as u8;
    }
    dst[3] = (out * 255.0).round() as u8;
}

fn opacity(mut color: Color, alpha: f32) -> Color {
    color[3] = (color[3] as f32 * alpha.clamp(0.0, 1.0)).round() as u8;
    color
}

fn rounded_distance(x: f32, y: f32, rect: [f32; 4], radius: f32) -> f32 {
    let [left, top, width, height] = rect;
    let r = radius.max(0.0).min(width * 0.5).min(height * 0.5);
    let qx = (x - left - width * 0.5).abs() - width * 0.5 + r;
    let qy = (y - top - height * 0.5).abs() - height * 0.5 + r;
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn rounded(frame: &mut RgbaFrame, rect: [f32; 4], radius: f32, color: Color) {
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    for py in (y.floor() as i32 - 1).max(0)..((y + h).ceil() as i32 + 1).min(frame.height as i32) {
        for px in (x.floor() as i32 - 1).max(0)..((x + w).ceil() as i32 + 1).min(frame.width as i32)
        {
            let coverage = (0.5 - rounded_distance(px as f32 + 0.5, py as f32 + 0.5, rect, radius))
                .clamp(0.0, 1.0);
            blend(frame, px, py, color, coverage);
        }
    }
}

fn circle(frame: &mut RgbaFrame, center: [f32; 2], radius: f32, color: Color) {
    rounded(
        frame,
        [
            center[0] - radius,
            center[1] - radius,
            radius * 2.0,
            radius * 2.0,
        ],
        radius,
        color,
    );
}

fn ring(frame: &mut RgbaFrame, center: [f32; 2], radius: f32, thickness: f32, color: Color) {
    let [x, y] = center;
    let extent = radius + thickness * 0.5 + 1.0;
    for py in
        ((y - extent).floor() as i32).max(0)..((y + extent).ceil() as i32).min(frame.height as i32)
    {
        for px in ((x - extent).floor() as i32).max(0)
            ..((x + extent).ceil() as i32).min(frame.width as i32)
        {
            let d = (px as f32 + 0.5 - x).hypot(py as f32 + 0.5 - y);
            blend(
                frame,
                px,
                py,
                color,
                (thickness * 0.5 + 0.5 - (d - radius).abs()).clamp(0.0, 1.0),
            );
        }
    }
}

/// Four vertical subpixel scanlines, with exact horizontal span coverage.
fn polygon(frame: &mut RgbaFrame, points: &[[f32; 2]], color: Color) {
    if points.len() < 3 || points.iter().flatten().any(|v| !v.is_finite()) {
        return;
    }
    let top = points
        .iter()
        .map(|p| p[1])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0) as usize;
    let bottom = points
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .min(frame.height as f32)
        .max(0.0) as usize;
    let left = points
        .iter()
        .map(|p| p[0])
        .fold(f32::INFINITY, f32::min)
        .floor()
        .max(0.0)
        .min(frame.width as f32) as usize;
    let right = points
        .iter()
        .map(|p| p[0])
        .fold(f32::NEG_INFINITY, f32::max)
        .ceil()
        .max(0.0)
        .min(frame.width as f32) as usize;
    if right <= left {
        return;
    }
    let mut coverage = vec![0.0f32; right - left];
    let mut crossings = Vec::with_capacity(points.len());
    for y in top..bottom {
        coverage.fill(0.0);
        for sample in 0..4 {
            let sy = y as f32 + (sample as f32 + 0.5) / 4.0;
            crossings.clear();
            for i in 0..points.len() {
                let a = points[i];
                let b = points[(i + 1) % points.len()];
                if (a[1] <= sy && b[1] > sy) || (b[1] <= sy && a[1] > sy) {
                    crossings.push(a[0] + (sy - a[1]) / (b[1] - a[1]) * (b[0] - a[0]));
                }
            }
            crossings.sort_by(f32::total_cmp);
            for span in crossings.chunks_exact(2) {
                let a = span[0].max(left as f32);
                let b = span[1].min(right as f32);
                if b <= a {
                    continue;
                }
                for x in a.floor() as usize..(b.ceil() as usize).min(right) {
                    coverage[x - left] +=
                        ((x as f32 + 1.0).min(b) - (x as f32).max(a)).max(0.0) * 0.25;
                }
            }
        }
        for (x, &c) in coverage.iter().enumerate() {
            blend(frame, (left + x) as i32, y as i32, color, c);
        }
    }
}

fn line(frame: &mut RgbaFrame, a: [f32; 2], b: [f32; 2], r0: f32, r1: f32, color: Color) {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let length = dx.hypot(dy);
    if length > 0.001 {
        let nx = -dy / length;
        let ny = dx / length;
        polygon(
            frame,
            &[
                [a[0] + nx * r0, a[1] + ny * r0],
                [b[0] + nx * r1, b[1] + ny * r1],
                [b[0] - nx * r1, b[1] - ny * r1],
                [a[0] - nx * r0, a[1] - ny * r0],
            ],
            color,
        );
    }
    circle(frame, a, r0, color);
    circle(frame, b, r1, color);
}

pub fn render_annotated_keyframe(
    base: &RgbaFrame,
    keyframe: &Keyframe,
    strokes: &[Stroke],
    marks: &[Mark],
    events: &[InteractionEvent],
) -> RgbaFrame {
    let mut frame = base.clone();
    frame.t_ms = keyframe.t;
    let w = frame.width as f32;
    let h = frame.height as f32;
    if w == 0.0 || h == 0.0 {
        return frame;
    }
    let size = (w.min(h) * 0.014).max(4.0);
    let active_mark = keyframe
        .mark_id
        .as_ref()
        .and_then(|id| marks.iter().find(|m| &m.id == id));
    // Historical marks go underneath the active mark, regardless of input ordering.
    for active_pass in [false, true] {
        for (index, stroke) in strokes.iter().enumerate() {
            if stroke.laser {
                continue;
            }
            let belongs = active_mark.is_some_and(|m| m.strokes.contains(&index));
            if !belongs && stroke.t0 > keyframe.t {
                continue;
            }
            let active = belongs
                || (keyframe.mark_id.is_none()
                    && keyframe.t.saturating_sub(stroke.t1) <= MARK_GAP_MS);
            if active != active_pass {
                continue;
            }
            let alpha = if active { 1.0 } else { 0.35 };
            let points: Vec<[f32; 2]> = stroke
                .timed_points()
                .into_iter()
                .filter(|&(_, _, t)| belongs || t <= keyframe.t)
                .map(|(x, y, _)| [x * w, y * h])
                .collect();
            if let Some(number) = stroke.pin {
                if let Some(&point) = points.first() {
                    draw_pin(
                        &mut frame,
                        point,
                        number,
                        (w / 1280.0).clamp(0.75, 2.5),
                        alpha,
                    );
                }
            } else if !points.is_empty() {
                for (diameter, color) in [
                    (size + 4.0, opacity(WHITE, 0.78 * alpha)),
                    (size, opacity(RED, alpha)),
                ] {
                    let outline = freehand_outline(
                        &points,
                        &FreehandOptions {
                            size: diameter,
                            ..Default::default()
                        },
                    );
                    polygon(&mut frame, &outline, color);
                }
            }
        }
    }
    for stroke in strokes.iter().filter(|s| s.laser) {
        draw_laser(&mut frame, stroke, keyframe.t);
    }
    let click = events
        .iter()
        .filter_map(|event| match event {
            InteractionEvent::Click { t, x, y, .. } if t.abs_diff(keyframe.t) <= 600 => {
                Some((t.abs_diff(keyframe.t), [*x, *y]))
            }
            _ => None,
        })
        .min_by_key(|(delta, _)| *delta)
        .map(|(_, point)| point);
    if let Some([cx, cy]) = keyframe
        .cursor
        .or(click)
        .filter(|p| p.iter().all(|v| v.is_finite()))
    {
        draw_cursor(&mut frame, [cx * w, cy * h]);
    }
    let label = format!("#{} · {}", keyframe.index, format_clock(keyframe.t));
    let badge_w = text_width(&label, 16.0, true) + 45.0;
    rounded(
        &mut frame,
        [16.0, 16.0, badge_w, 36.0],
        18.0,
        [255, 255, 255, 46],
    );
    rounded(
        &mut frame,
        [17.0, 17.0, badge_w - 2.0, 34.0],
        17.0,
        [20, 20, 22, 224],
    );
    circle(&mut frame, [31.0, 34.0], 4.0, RED);
    draw_text(&mut frame, &label, 43.0, 24.0, 16.0, WHITE, true);
    frame
}

fn draw_pin(frame: &mut RgbaFrame, point: [f32; 2], number: u32, scale: f32, alpha: f32) {
    let label = number.to_string();
    let radius = 13.0 * scale;
    circle(frame, point, radius + 2.5 * scale, opacity(WHITE, alpha));
    circle(frame, point, radius, opacity(RED, alpha));
    let mut size = 16.0 * scale;
    let width = text_width(&label, size, true);
    if width > radius * 1.65 {
        size *= radius * 1.65 / width;
    }
    let width = text_width(&label, size, true);
    draw_text(
        frame,
        &label,
        point[0] - width * 0.5,
        point[1] - size * 0.58,
        size,
        opacity(WHITE, alpha),
        true,
    );
}

fn draw_laser(frame: &mut RgbaFrame, stroke: &Stroke, time: u64) {
    let start = time.saturating_sub(LASER_MS);
    let points: Vec<_> = stroke
        .timed_points()
        .into_iter()
        .filter(|&(_, _, t)| t >= start && t <= time)
        .map(|(x, y, t)| {
            (
                [x * frame.width as f32, y * frame.height as f32],
                1.0 - time.saturating_sub(t) as f32 / LASER_MS as f32,
            )
        })
        .collect();
    // Multiple broad, low-opacity passes form the glow; the center stays sharp.
    for (radius, alpha, color) in [
        (11.0, 0.08, RED),
        (7.0, 0.16, RED),
        (3.2, 0.9, RED),
        (1.1, 1.0, WHITE),
    ] {
        if points.len() == 1 {
            circle(
                frame,
                points[0].0,
                radius,
                opacity(color, alpha * points[0].1),
            );
        }
        for pair in points.windows(2) {
            let (a, fa) = pair[0];
            let (b, fb) = pair[1];
            line(
                frame,
                a,
                b,
                radius * (0.2 + fa * 0.8),
                radius * (0.2 + fb * 0.8),
                opacity(color, alpha * (fa + fb) * 0.5),
            );
        }
    }
}

fn draw_cursor(frame: &mut RgbaFrame, p: [f32; 2]) {
    circle(frame, p, 19.0, [0, 136, 255, 77]);
    ring(frame, p, 20.0, 2.0, WHITE);
    ring(frame, p, 23.0, 2.0, [0, 136, 255, 255]);
    let arrow = [
        [0.0, 0.0],
        [0.0, 27.0],
        [6.5, 20.5],
        [12.0, 32.0],
        [17.0, 29.5],
        [11.5, 18.5],
        [21.0, 18.5],
    ];
    let outer: Vec<_> = arrow.iter().map(|q| [p[0] + q[0], p[1] + q[1]]).collect();
    for i in 0..outer.len() {
        line(
            frame,
            outer[i],
            outer[(i + 1) % outer.len()],
            1.25,
            1.25,
            WHITE,
        );
    }
    polygon(frame, &outer, [18, 18, 20, 255]);
}

pub fn render_contact_sheet(
    bundle: &ScrawlBundle,
    annotated_frames: &[(Keyframe, RgbaFrame)],
) -> RgbaFrame {
    const CELL: f32 = 640.0;
    const PAD: f32 = 24.0;
    const GAP: f32 = 20.0;
    let frames = &annotated_frames[..annotated_frames.len().min(MAX_KEYFRAMES)];
    let columns = if frames.len() <= 1 { 1 } else { 2 };
    let width = (PAD * 2.0 + CELL * columns as f32 + GAP * (columns - 1) as f32) as u32;
    let app = bundle.target.app.as_deref().unwrap_or("Display");
    let title = match bundle
        .target
        .window_title
        .as_deref()
        .filter(|s| !s.is_empty() && *s != app)
    {
        Some(window) => format!("{app} · {window}"),
        None => app.to_owned(),
    };
    let title_lines = wrap_text(&title, 20.0, width as f32 - PAD * 2.0, true);
    let summary = bundle
        .transcript
        .text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut summary_lines = wrap_text(&summary, 15.0, width as f32 - PAD * 2.0, false);
    if summary_lines.len() > 4 {
        summary_lines.truncate(4);
        if let Some(last) = summary_lines.last_mut() {
            while text_width(&format!("{last}…"), 15.0, false) > width as f32 - PAD * 2.0 {
                if last.pop().is_none() {
                    break;
                }
            }
            last.push('…');
        }
    }
    let header_height = 72.0
        + title_lines.len() as f32 * 26.0
        + summary_lines.len() as f32 * 21.0
        + if summary_lines.is_empty() { 0.0 } else { 10.0 };
    struct Card {
        image: [f32; 2],
        label: String,
        label_width: f32,
        event: Vec<String>,
        said: Vec<String>,
        height: f32,
    }
    let cards: Vec<_> = frames
        .iter()
        .map(|(key, image)| {
            let scale = (CELL / image.width.max(1) as f32).min(960.0 / image.height.max(1) as f32);
            let image_size = [image.width as f32 * scale, image.height as f32 * scale];
            let label = format!("[#{} · {}]", key.index, format_clock(key.t));
            let label_width = text_width(&label, 16.0, true) + 10.0;
            let event = wrap_text_indented(
                &key.event,
                16.0,
                CELL - 32.0,
                (CELL - 32.0 - label_width).max(30.0),
                false,
            );
            let said = key
                .said
                .as_deref()
                .filter(|s| !s.trim().is_empty())
                .map(|s| wrap_text(&format!("“{}”", s.trim()), 15.0, CELL - 32.0, false))
                .unwrap_or_default();
            let height = image_size[1]
                + 30.0
                + event.len().max(1) as f32 * 22.0
                + if said.is_empty() {
                    0.0
                } else {
                    8.0 + said.len() as f32 * 21.0
                };
            Card {
                image: image_size,
                label,
                label_width,
                event,
                said,
                height,
            }
        })
        .collect();
    let row_heights: Vec<f32> = cards
        .chunks(columns)
        .map(|row| row.iter().map(|c| c.height).fold(0.0, f32::max))
        .collect();
    let height = (header_height
        + row_heights.iter().sum::<f32>()
        + GAP * row_heights.len().saturating_sub(1) as f32
        + PAD)
        .ceil() as u32;
    let mut sheet = solid(width, height, [20, 20, 20, 255]);
    sheet.t_ms = bundle.duration_ms;
    circle(&mut sheet, [PAD + 4.0, 34.0], 4.0, RED);
    draw_text(&mut sheet, "SCRAWL", PAD + 18.0, 24.0, 16.0, WHITE, true);
    let duration = format_clock(bundle.duration_ms);
    let duration_width = text_width(&duration, 16.0, false);
    draw_text(
        &mut sheet,
        &duration,
        width as f32 - PAD - duration_width,
        24.0,
        16.0,
        MUTED,
        false,
    );
    let mut y = 58.0;
    for text in &title_lines {
        draw_text(&mut sheet, text, PAD, y, 20.0, TEXT, true);
        y += 26.0;
    }
    if !summary_lines.is_empty() {
        y += 10.0;
    }
    for text in &summary_lines {
        draw_text(&mut sheet, text, PAD, y, 15.0, MUTED, false);
        y += 21.0;
    }
    let mut row_y = header_height;
    for (row, row_height) in row_heights.iter().enumerate() {
        for col in 0..columns {
            let i = row * columns + col;
            let Some(card) = cards.get(i) else {
                continue;
            };
            let (_, image) = &frames[i];
            let x = PAD + col as f32 * (CELL + GAP);
            rounded(
                &mut sheet,
                [x, row_y, CELL, *row_height],
                10.0,
                [42, 42, 42, 255],
            );
            rounded(
                &mut sheet,
                [x + 1.0, row_y + 1.0, CELL - 2.0, *row_height - 2.0],
                9.0,
                [29, 29, 29, 255],
            );
            let image_rect = [
                x + (CELL - card.image[0]) * 0.5 + 1.0,
                row_y + 1.0,
                (card.image[0] - 2.0).max(0.0),
                (card.image[1] - 2.0).max(0.0),
            ];
            blit_scaled(&mut sheet, image, image_rect, 9.0);
            let mut cy = row_y + card.image[1] + 14.0;
            draw_text(&mut sheet, &card.label, x + 16.0, cy, 16.0, RED, true);
            for (line_index, text) in card.event.iter().enumerate() {
                let indent = if line_index == 0 {
                    card.label_width
                } else {
                    0.0
                };
                draw_text(&mut sheet, text, x + 16.0 + indent, cy, 16.0, TEXT, false);
                cy += 22.0;
            }
            if card.event.is_empty() {
                cy += 22.0;
            }
            if !card.said.is_empty() {
                cy += 8.0;
            }
            for text in &card.said {
                draw_text(&mut sheet, text, x + 16.0, cy, 15.0, MUTED, false);
                cy += 21.0;
            }
        }
        row_y += row_height + GAP;
    }
    sheet
}

fn blit_scaled(dst: &mut RgbaFrame, src: &RgbaFrame, rect: [f32; 4], radius: f32) {
    let [x, y, w, h] = rect;
    if src.width == 0 || src.height == 0 || w <= 0.0 || h <= 0.0 {
        return;
    }
    for py in (y.floor() as i32).max(0)..((y + h).ceil() as i32).min(dst.height as i32) {
        for px in (x.floor() as i32).max(0)..((x + w).ceil() as i32).min(dst.width as i32) {
            let coverage = (0.5 - rounded_distance(px as f32 + 0.5, py as f32 + 0.5, rect, radius))
                .clamp(0.0, 1.0);
            let sx = ((px as f32 + 0.5 - x) / w * src.width as f32 - 0.5)
                .clamp(0.0, src.width.saturating_sub(1) as f32);
            let sy = ((py as f32 + 0.5 - y) / h * src.height as f32 - 0.5)
                .clamp(0.0, src.height.saturating_sub(1) as f32);
            let x0 = sx.floor() as u32;
            let y0 = sy.floor() as u32;
            let fx = sx.fract();
            let fy = sy.fract();
            let mut accum = [0.0f32; 4];
            for (xx, yy, weight) in [
                (x0, y0, (1.0 - fx) * (1.0 - fy)),
                ((x0 + 1).min(src.width - 1), y0, fx * (1.0 - fy)),
                (x0, (y0 + 1).min(src.height - 1), (1.0 - fx) * fy),
                (
                    (x0 + 1).min(src.width - 1),
                    (y0 + 1).min(src.height - 1),
                    fx * fy,
                ),
            ] {
                let i = (yy as usize * src.width as usize + xx as usize) * 4;
                if let Some(p) = src.rgba.get(i..i + 4) {
                    let a = p[3] as f32 / 255.0;
                    for c in 0..3 {
                        accum[c] += p[c] as f32 * a * weight;
                    }
                    accum[3] += a * weight;
                }
            }
            if accum[3] > 0.0 {
                blend(
                    dst,
                    px,
                    py,
                    [
                        (accum[0] / accum[3]).round() as u8,
                        (accum[1] / accum[3]).round() as u8,
                        (accum[2] / accum[3]).round() as u8,
                        (accum[3] * 255.0).round() as u8,
                    ],
                    coverage,
                );
            }
        }
    }
}

/// Render at a stable design size, then scale to the requested capture geometry.
pub fn render_synthetic_demo_frame(width: u32, height: u32, t_ms: u64, phase: u32) -> RgbaFrame {
    let mut frame = solid(1280, 800, [22, 23, 26, 255]);
    rounded(&mut frame, [0.0, 0.0, 244.0, 800.0], 0.0, [28, 29, 33, 255]);
    rounded(&mut frame, [244.0, 0.0, 1.0, 800.0], 0.0, [48, 49, 54, 255]);
    for (x, color) in [
        (22.0, [255, 95, 87, 255]),
        (42.0, [254, 188, 46, 255]),
        (62.0, [40, 200, 64, 255]),
    ] {
        circle(&mut frame, [x, 24.0], 6.0, color);
    }
    draw_text(&mut frame, "Workspace", 20.0, 67.0, 20.0, TEXT, true);
    draw_text(
        &mut frame,
        "+  New session",
        20.0,
        116.0,
        16.0,
        MUTED,
        false,
    );
    draw_text(&mut frame, "SESSIONS", 20.0, 176.0, 11.0, MUTED, true);
    for (i, title) in [
        "Review tool activity",
        "Update settings panel",
        "Keyboard navigation",
        "Fix scroll position",
    ]
    .iter()
    .enumerate()
    {
        let y = 209.0 + i as f32 * 48.0;
        if i == 0 {
            rounded(
                &mut frame,
                [10.0, y - 8.0, 224.0, 38.0],
                7.0,
                [47, 48, 55, 255],
            );
        }
        draw_text(
            &mut frame,
            title,
            22.0,
            y,
            14.0,
            if i == 0 { TEXT } else { MUTED },
            false,
        );
    }
    draw_text(&mut frame, "Settings", 22.0, 754.0, 14.0, MUTED, false);
    rounded(
        &mut frame,
        [245.0, 0.0, 1035.0, 64.0],
        0.0,
        [26, 27, 31, 255],
    );
    draw_text(
        &mut frame,
        "Review tool activity",
        282.0,
        23.0,
        17.0,
        TEXT,
        true,
    );
    draw_text(&mut frame, "Share", 1169.0, 25.0, 14.0, MUTED, false);
    rounded(
        &mut frame,
        [292.0, 100.0, 928.0, 72.0],
        12.0,
        [36, 38, 43, 255],
    );
    draw_text(
        &mut frame,
        "Check the tool results and explain what changed.",
        313.0,
        126.0,
        18.0,
        TEXT,
        false,
    );
    circle(&mut frame, [304.0, 210.0], 10.0, [83, 114, 229, 255]);
    draw_text(&mut frame, "Assistant", 326.0, 200.0, 16.0, TEXT, true);
    draw_text(
        &mut frame,
        "I’ll inspect the files and compare the output.",
        294.0,
        242.0,
        17.0,
        [204, 206, 212, 255],
        false,
    );
    let expanded = phase >= 1;
    let band_y = if expanded { 304.0 } else { 286.0 };
    rounded(
        &mut frame,
        [290.0, band_y, 930.0, 48.0],
        8.0,
        [43, 45, 51, 255],
    );
    draw_text(
        &mut frame,
        if expanded {
            "−  Used 12 tools"
        } else {
            "+  Used 12 tools"
        },
        309.0,
        band_y + 14.0,
        16.0,
        TEXT,
        true,
    );
    draw_text(
        &mut frame,
        "12.4s",
        1145.0,
        band_y + 15.0,
        14.0,
        MUTED,
        false,
    );
    if expanded {
        for (i, (name, detail)) in [
            ("Read", "src/components/tool-panel.tsx"),
            ("Search", "tool results · 8 matches"),
            ("Read", "src/styles/layout.css"),
            ("Run", "Checks passed"),
        ]
        .iter()
        .enumerate()
        {
            let y = band_y + 60.0 + i as f32 * 43.0;
            circle(&mut frame, [313.0, y + 10.0], 3.0, [105, 190, 139, 255]);
            draw_text(&mut frame, name, 328.0, y, 14.0, TEXT, true);
            draw_text(&mut frame, detail, 402.0, y, 14.0, MUTED, false);
            rounded(
                &mut frame,
                [310.0, y + 31.0, 888.0, 1.0],
                0.0,
                [42, 44, 49, 255],
            );
        }
    }
    let response_y = if expanded { 569.0 } else { 363.0 };
    draw_text(
        &mut frame,
        "The results are ready.",
        294.0,
        response_y,
        18.0,
        TEXT,
        true,
    );
    draw_text(
        &mut frame,
        "Open a tool row to see its input and output.",
        294.0,
        response_y + 34.0,
        16.0,
        MUTED,
        false,
    );
    rounded(
        &mut frame,
        [284.0, 686.0, 946.0, 88.0],
        15.0,
        [64, 66, 74, 255],
    );
    rounded(
        &mut frame,
        [285.0, 687.0, 944.0, 86.0],
        14.0,
        [31, 33, 38, 255],
    );
    draw_text(
        &mut frame,
        "Ask a follow-up…",
        308.0,
        708.0,
        16.0,
        MUTED,
        false,
    );
    draw_text(&mut frame, "+", 309.0, 746.0, 16.0, MUTED, false);
    draw_text(
        &mut frame,
        "Local workspace",
        341.0,
        747.0,
        12.0,
        MUTED,
        false,
    );
    circle(&mut frame, [1200.0, 746.0], 13.0, [224, 226, 231, 255]);
    draw_text(
        &mut frame,
        "↑",
        1193.0,
        736.0,
        17.0,
        [28, 29, 33, 255],
        true,
    );
    if phase >= 2 {
        ring(&mut frame, [1200.0, 746.0], 16.0, 2.0, [93, 161, 255, 255]);
    }
    if width == 1280 && height == 800 {
        frame.t_ms = t_ms;
        return frame;
    }
    let mut out = solid(width, height, [22, 23, 26, 255]);
    blit_scaled(
        &mut out,
        &frame,
        [0.0, 0.0, width as f32, height as f32],
        0.0,
    );
    out.t_ms = t_ms;
    out
}

fn wrap_text(text: &str, size: f32, width: f32, bold: bool) -> Vec<String> {
    wrap_text_indented(text, size, width, width, bold)
}

fn wrap_text_indented(
    text: &str,
    size: f32,
    width: f32,
    first_width: f32,
    bold: bool,
) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            let available = if lines.is_empty() { first_width } else { width };
            if text_width(&candidate, size, bold) <= available {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            // Character boundaries avoid slicing inside UTF-8, including emoji.
            for ch in word.chars() {
                let mut candidate = line.clone();
                candidate.push(ch);
                let available = if lines.is_empty() { first_width } else { width };
                if !line.is_empty() && text_width(&candidate, size, bold) > available {
                    lines.push(std::mem::take(&mut line));
                }
                line.push(ch);
            }
        }
        if !line.is_empty() {
            lines.push(line);
        }
    }
    lines
}

#[cfg(target_os = "macos")]
mod native_text {
    use core_foundation::{
        attributed_string::CFMutableAttributedString,
        base::{CFRange, TCFType},
        boolean::CFBoolean,
        string::CFString,
    };
    use core_graphics::{
        base::kCGImageAlphaNone,
        color_space::CGColorSpace,
        context::{CGContext, CGTextDrawingMode},
    };
    use core_text::{
        font::{self, CTFont},
        line::CTLine,
        string_attributes::{kCTFontAttributeName, kCTForegroundColorFromContextAttributeName},
    };

    pub fn line(text: &str, size: f32, bold: bool) -> Option<(CTFont, CTLine)> {
        let font = font::new_from_name(
            if bold {
                "HelveticaNeue-Bold"
            } else {
                "HelveticaNeue"
            },
            size as f64,
        )
        .ok()?;
        let mut attributed = CFMutableAttributedString::new();
        attributed.replace_str(&CFString::new(text), CFRange::init(0, 0));
        unsafe {
            attributed.set_attribute(
                CFRange::init(0, attributed.char_len()),
                kCTFontAttributeName,
                &font,
            );
            attributed.set_attribute(
                CFRange::init(0, attributed.char_len()),
                kCTForegroundColorFromContextAttributeName,
                &CFBoolean::true_value(),
            );
        }
        let line = CTLine::new_with_attributed_string(attributed.as_concrete_TypeRef());
        Some((font, line))
    }

    pub fn mask(text: &str, size: f32, bold: bool) -> Option<(usize, usize, Vec<u8>)> {
        let (font, line) = line(text, size, bold)?;
        let width = line.get_typographic_bounds().width.ceil().max(1.0) as usize + 4;
        let height = (font.ascent() + font.descent() + font.leading())
            .ceil()
            .max(1.0) as usize
            + 4;
        let space = CGColorSpace::create_device_gray();
        let mut ctx = CGContext::create_bitmap_context(
            None,
            width,
            height,
            8,
            width,
            &space,
            kCGImageAlphaNone,
        );
        ctx.data().fill(0);
        ctx.set_should_antialias(true);
        ctx.set_gray_fill_color(1.0, 1.0);
        ctx.set_text_drawing_mode(CGTextDrawingMode::CGTextFill);
        ctx.set_text_position(2.0, height as f64 - 2.0 - font.ascent());
        line.draw(&ctx);
        ctx.flush();
        Some((width, height, ctx.data().to_vec()))
    }
}

fn text_width(text: &str, size: f32, bold: bool) -> f32 {
    #[cfg(target_os = "macos")]
    if let Some((_, line)) = native_text::line(text, size, bold) {
        return line.get_typographic_bounds().width as f32;
    }
    let _ = bold;
    text.chars().count() as f32 * size * 6.0 / 7.0
}

fn draw_text(
    frame: &mut RgbaFrame,
    text: &str,
    x: f32,
    y: f32,
    size: f32,
    color: Color,
    bold: bool,
) {
    if text.is_empty() || size <= 0.0 {
        return;
    }
    #[cfg(target_os = "macos")]
    if let Some((width, height, pixels)) = native_text::mask(text, size, bold) {
        for py in 0..height {
            for px in 0..width {
                blend(
                    frame,
                    x.round() as i32 + px as i32 - 2,
                    y.round() as i32 + py as i32 - 2,
                    color,
                    pixels[py * width + px] as f32 / 255.0,
                );
            }
        }
        return;
    }
    let _ = bold;
    let scale = size / 7.0;
    for (index, ch) in text.chars().enumerate() {
        let rows = glyph(ch);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..5 {
                if bits & (1 << (4 - col)) != 0 {
                    rounded(
                        frame,
                        [
                            x + (index * 6 + col) as f32 * scale,
                            y + row as f32 * scale,
                            scale,
                            scale,
                        ],
                        0.0,
                        color,
                    );
                }
            }
        }
    }
}

/// Portable 5×7 fallback. Unsupported Unicode is a visible replacement glyph.
fn glyph(ch: char) -> [u8; 7] {
    match ch.to_ascii_uppercase() {
        'A' => [14, 17, 17, 31, 17, 17, 17],
        'B' => [30, 17, 17, 30, 17, 17, 30],
        'C' => [14, 17, 16, 16, 16, 17, 14],
        'D' => [30, 17, 17, 17, 17, 17, 30],
        'E' => [31, 16, 16, 30, 16, 16, 31],
        'F' => [31, 16, 16, 30, 16, 16, 16],
        'G' => [14, 17, 16, 23, 17, 17, 15],
        'H' => [17, 17, 17, 31, 17, 17, 17],
        'I' => [14, 4, 4, 4, 4, 4, 14],
        'J' => [7, 2, 2, 2, 18, 18, 12],
        'K' => [17, 18, 20, 24, 20, 18, 17],
        'L' => [16, 16, 16, 16, 16, 16, 31],
        'M' => [17, 27, 21, 21, 17, 17, 17],
        'N' => [17, 25, 21, 19, 17, 17, 17],
        'O' => [14, 17, 17, 17, 17, 17, 14],
        'P' => [30, 17, 17, 30, 16, 16, 16],
        'Q' => [14, 17, 17, 17, 21, 18, 13],
        'R' => [30, 17, 17, 30, 20, 18, 17],
        'S' => [15, 16, 16, 14, 1, 1, 30],
        'T' => [31, 4, 4, 4, 4, 4, 4],
        'U' => [17, 17, 17, 17, 17, 17, 14],
        'V' => [17, 17, 17, 17, 17, 10, 4],
        'W' => [17, 17, 17, 21, 21, 21, 10],
        'X' => [17, 17, 10, 4, 10, 17, 17],
        'Y' => [17, 17, 10, 4, 4, 4, 4],
        'Z' => [31, 1, 2, 4, 8, 16, 31],
        '0' => [14, 17, 19, 21, 25, 17, 14],
        '1' => [4, 12, 4, 4, 4, 4, 14],
        '2' => [14, 17, 1, 2, 4, 8, 31],
        '3' => [30, 1, 1, 14, 1, 1, 30],
        '4' => [2, 6, 10, 18, 31, 2, 2],
        '5' => [31, 16, 16, 30, 1, 1, 30],
        '6' => [14, 16, 16, 30, 17, 17, 14],
        '7' => [31, 1, 2, 4, 8, 8, 8],
        '8' => [14, 17, 17, 14, 17, 17, 14],
        '9' => [14, 17, 17, 15, 1, 1, 14],
        ' ' | '\t' | '\n' | '\r' => [0; 7],
        '.' => [0, 0, 0, 0, 0, 12, 12],
        ',' => [0, 0, 0, 0, 4, 4, 8],
        ':' => [0, 12, 12, 0, 12, 12, 0],
        ';' => [0, 12, 12, 0, 4, 4, 8],
        '#' => [10, 31, 10, 10, 31, 10, 0],
        '·' | '•' => [0, 0, 0, 4, 0, 0, 0],
        '-' | '–' | '—' | '−' => [0, 0, 0, 31, 0, 0, 0],
        '_' => [0, 0, 0, 0, 0, 0, 31],
        '+' => [0, 4, 4, 31, 4, 4, 0],
        '/' => [1, 2, 2, 4, 8, 8, 16],
        '\\' => [16, 8, 8, 4, 2, 2, 1],
        '[' => [14, 8, 8, 8, 8, 8, 14],
        ']' => [14, 2, 2, 2, 2, 2, 14],
        '(' => [2, 4, 8, 8, 8, 4, 2],
        ')' => [8, 4, 2, 2, 2, 4, 8],
        '{' => [2, 4, 4, 8, 4, 4, 2],
        '}' => [8, 4, 4, 2, 4, 4, 8],
        '\'' | '’' | '‘' | '`' => [4, 4, 8, 0, 0, 0, 0],
        '"' | '“' | '”' => [10, 10, 20, 0, 0, 0, 0],
        '!' => [4, 4, 4, 4, 4, 0, 4],
        '?' => [14, 17, 1, 2, 4, 0, 4],
        '=' => [0, 0, 31, 0, 31, 0, 0],
        '<' => [1, 2, 4, 8, 4, 2, 1],
        '>' => [16, 8, 4, 2, 4, 8, 16],
        '↑' => [4, 14, 21, 4, 4, 4, 4],
        '↓' => [4, 4, 4, 4, 21, 14, 4],
        '→' => [0, 4, 2, 31, 2, 4, 0],
        '←' => [0, 4, 8, 31, 8, 4, 0],
        '✎' | '✏' => [3, 7, 6, 12, 24, 16, 31],
        '…' => [0, 0, 0, 0, 0, 0, 21],
        '×' => [0, 17, 10, 4, 10, 17, 0],
        '*' => [0, 21, 14, 31, 14, 21, 0],
        '%' => [25, 25, 2, 4, 8, 19, 19],
        '&' => [12, 18, 20, 8, 21, 18, 13],
        '@' => [14, 17, 23, 21, 23, 16, 14],
        '|' => [4; 7],
        _ => [31, 17, 5, 4, 4, 0, 4],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn key(index: usize) -> Keyframe {
        Keyframe {
            index,
            t: 4000,
            reason: KeyframeReason::Mark,
            file: format!("frames/{index}.png"),
            event: "✎ circled region x 0.42–0.58, y 0.26–0.34 over “Band header”".into(),
            said: Some("When I click Used 12 tools…".into()),
            mark_id: None,
            cursor: None,
        }
    }

    fn bundle() -> ScrawlBundle {
        ScrawlBundle {
            version: 1,
            id: "render-test".into(),
            created_at_ms: 0,
            duration_ms: 8000,
            target: TargetInfo {
                mode: CaptureMode::Window,
                app: Some("Demo".into()),
                bundle_id: None,
                window_title: Some("Tool activity".into()),
                window_id: None,
                display_id: 1,
                frame: TargetFrame {
                    x: 0.0,
                    y: 0.0,
                    width: 320.0,
                    height: 200.0,
                    scale: 1.0,
                },
            },
            transcript: TranscriptData {
                text: "Click the tool header and inspect the layout shift.".into(),
                ..Default::default()
            },
            note: None,
            strokes: vec![],
            marks: vec![],
            events: vec![],
            keyframes: vec![],
            contact_sheet: "contact-sheet.png".into(),
            video: None,
            audio: None,
        }
    }

    #[test]
    fn png_round_trip_preserves_rgba_including_alpha() {
        let frame = RgbaFrame::new(
            123,
            2,
            2,
            vec![
                255, 59, 48, 255, 3, 5, 7, 0, 23, 42, 91, 128, 255, 255, 255, 255,
            ],
        );
        let result = decode_png(Cursor::new(encode_png(&frame).unwrap())).unwrap();
        assert_eq!((result.width, result.height), (2, 2));
        assert_eq!(result.rgba, frame.rgba);
    }

    #[test]
    fn active_ink_paints_red_but_future_ink_is_hidden() {
        let base = solid(320, 200, [20, 20, 20, 255]);
        let stroke = Stroke {
            t0: 3500,
            t1: 3900,
            fw: 320,
            fh: 200,
            on: None,
            pin: None,
            laser: false,
            pts: vec![1500, 6000, 0, 5000, 6000, 200, 8500, 6000, 400],
        };
        let result = render_annotated_keyframe(&base, &key(1), &[stroke.clone()], &[], &[]);
        let red = result
            .rgba
            .chunks_exact(4)
            .enumerate()
            .filter(|(i, p)| i / 320 > 100 && *p == RED)
            .count();
        assert!(
            red > 80,
            "Expected opaque red stroke pixels below the badge, got {red}"
        );
        let future = Stroke {
            t0: 5000,
            t1: 5400,
            ..stroke
        };
        let result = render_annotated_keyframe(&base, &key(1), &[future], &[], &[]);
        assert_eq!(&result.rgba[100 * 320 * 4..], &base.rgba[100 * 320 * 4..]);
        assert!(base.rgba.chunks_exact(4).all(|p| p == [20, 20, 20, 255]));
    }

    #[test]
    fn contact_sheets_support_one_three_and_four_frames() {
        for count in [1, 3, 4] {
            let frames: Vec<_> = (1..=count)
                .map(|i| (key(i), solid(320, 200, [40 + i as u8 * 10, 60, 80, 255])))
                .collect();
            let sheet = render_contact_sheet(&bundle(), &frames);
            assert_eq!(sheet.width, if count == 1 { 688 } else { 1348 });
            assert!(sheet.height > if count == 1 { 500 } else { 1000 });
            assert_eq!(
                sheet.rgba.len(),
                sheet.width as usize * sheet.height as usize * 4
            );
            for i in 1..=count {
                assert!(
                    sheet
                        .rgba
                        .chunks_exact(4)
                        .any(|p| p == [40 + i as u8 * 10, 60, 80, 255]),
                    "Missing card {i}"
                );
            }
            assert!(sheet.rgba.chunks_exact(4).all(|p| p[3] == 255));
        }
    }

    #[test]
    fn unicode_wrapping_and_empty_sheet_are_safe() {
        let lines = wrap_text(
            "✎ · – “hello” 🦀 漢字 longwordwithoutspaces",
            16.0,
            80.0,
            false,
        );
        assert!(lines.len() > 2);
        assert!(lines.iter().all(|s| text_width(s, 16.0, false) <= 81.0));
        let sheet = render_contact_sheet(&bundle(), &[]);
        assert!(sheet.width > 0 && sheet.height > 0);
    }
}
