//! Line icons embedded in the binary and served to GPUI's `svg()` element.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};

pub const AREA: &str = "icons/area.svg";
pub const WINDOW: &str = "icons/window.svg";
pub const SCREEN: &str = "icons/screen.svg";
pub const IPHONE: &str = "icons/iphone.svg";

const ICONS: [(&str, &[u8]); 4] = [
    (AREA, include_bytes!("../assets/icons/area.svg")),
    (WINDOW, include_bytes!("../assets/icons/window.svg")),
    (SCREEN, include_bytes!("../assets/icons/screen.svg")),
    (IPHONE, include_bytes!("../assets/icons/iphone.svg")),
];

pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(ICONS
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(ICONS
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_a_stroked_svg() {
        for (name, bytes) in ICONS {
            let svg = std::str::from_utf8(bytes).unwrap();
            assert!(svg.starts_with("<svg"), "{name}");
            assert!(svg.contains("stroke-width=\"2.5\""), "{name}");
            assert!(Assets.load(name).unwrap().is_some());
        }
        assert!(Assets.load("icons/missing.svg").unwrap().is_none());
        assert_eq!(Assets.list("icons/").unwrap().len(), 4);
    }
}
