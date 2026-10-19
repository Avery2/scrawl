//! The clipboard: text and PNG through `arboard`'s X11 selections. On a Wayland session that is
//! XWayland's clipboard, which GNOME and KDE hand to Wayland apps too. The clipboard is kept
//! open for the life of the process: on Linux the app that copied must stay around to hand the
//! data over.

use std::sync::Mutex;

use anyhow::{Context, Result, anyhow};

static CLIPBOARD: Mutex<Option<arboard::Clipboard>> = Mutex::new(None);

fn with<T>(f: impl FnOnce(&mut arboard::Clipboard) -> Result<T, arboard::Error>) -> Result<T> {
    let mut slot = CLIPBOARD
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.is_none() {
        *slot = Some(arboard::Clipboard::new().map_err(|e| anyhow!("{e}"))?);
    }
    let clipboard = slot.as_mut().context("No clipboard")?;
    f(clipboard).map_err(|e| anyhow!("Clipboard: {e}"))
}

pub fn set_text(text: &str) -> Result<()> {
    with(|c| c.set_text(text.to_owned()))
}

pub fn set_png(png_bytes: &[u8]) -> Result<()> {
    let decoder = png::Decoder::new(std::io::Cursor::new(png_bytes));
    let mut reader = decoder.read_info().context("Not a PNG")?;
    let mut buffer = vec![0; reader.output_buffer_size().context("PNG too large")?];
    let info = reader.next_frame(&mut buffer)?;
    let rgba: Vec<u8> = match info.color_type {
        png::ColorType::Rgba => buffer[..info.buffer_size()].to_vec(),
        png::ColorType::Rgb => buffer[..info.buffer_size()]
            .chunks_exact(3)
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        other => anyhow::bail!("Unsupported PNG colour type {other:?}"),
    };
    with(|c| {
        c.set_image(arboard::ImageData {
            width: info.width as usize,
            height: info.height as usize,
            bytes: rgba.into(),
        })
    })
}
