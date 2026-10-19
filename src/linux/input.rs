//! Keys and mouse buttons straight from the kernel's input devices (`/dev/input/event*`), for a
//! Wayland session, where no app hears another's input. Reading them needs membership of the
//! `input` group (`sudo usermod -aG input $USER`, then log in again): the same trust macOS asks
//! for with Accessibility. Scrawl only keeps which modifiers and buttons are down and a few
//! keys (its shortcuts, Esc, Ctrl+Z); it never records what is typed.
//!
//! On X11 none of this is used: the X server answers the same questions (`super::x11`).

use std::collections::HashSet;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// Linux input event codes (`linux/input-event-codes.h`).
pub mod code {
    pub const KEY_ESC: u16 = 1;
    pub const KEY_LEFTCTRL: u16 = 29;
    pub const KEY_LEFTSHIFT: u16 = 42;
    pub const KEY_RIGHTSHIFT: u16 = 54;
    pub const KEY_LEFTALT: u16 = 56;
    pub const KEY_RIGHTCTRL: u16 = 97;
    pub const KEY_RIGHTALT: u16 = 100;
    pub const KEY_LEFTMETA: u16 = 125;
    pub const KEY_RIGHTMETA: u16 = 126;
    pub const BTN_LEFT: u16 = 0x110;
    pub const BTN_RIGHT: u16 = 0x111;
}

const EV_KEY: u16 = 0x01;

/// What is held down right now, across every keyboard and mouse.
#[derive(Clone, Debug, Default)]
pub struct Held {
    pub keys: HashSet<u16>,
}

impl Held {
    pub fn any(&self, codes: &[u16]) -> bool {
        codes.iter().any(|c| self.keys.contains(c))
    }
    pub fn shift(&self) -> bool {
        self.any(&[code::KEY_LEFTSHIFT, code::KEY_RIGHTSHIFT])
    }
    pub fn ctrl(&self) -> bool {
        self.any(&[code::KEY_LEFTCTRL, code::KEY_RIGHTCTRL])
    }
    pub fn alt(&self) -> bool {
        self.any(&[code::KEY_LEFTALT, code::KEY_RIGHTALT])
    }
    pub fn super_key(&self) -> bool {
        self.any(&[code::KEY_LEFTMETA, code::KEY_RIGHTMETA])
    }
    pub fn left(&self) -> bool {
        self.keys.contains(&code::BTN_LEFT)
    }
    pub fn right(&self) -> bool {
        self.keys.contains(&code::BTN_RIGHT)
    }
}

struct Reader {
    held: Mutex<Held>,
}

static READER: OnceLock<Option<Reader>> = OnceLock::new();

fn event_devices() -> Vec<std::path::PathBuf> {
    let mut list: Vec<_> = std::fs::read_dir("/dev/input")
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("event"))
                })
                .collect()
        })
        .unwrap_or_default();
    list.sort();
    list
}

fn open(path: &std::path::Path) -> Option<OwnedFd> {
    use std::os::unix::fs::OpenOptionsExt as _;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .ok()
        .map(OwnedFd::from)
}

/// `EVIOCGBIT(EV_KEY, len)`: which keys a device has.
fn key_bits(fd: &OwnedFd) -> Option<Vec<u8>> {
    const LEN: usize = 96;
    let mut bits = vec![0_u8; LEN];
    let request = (2_u64 << 30) | ((LEN as u64) << 16) | (u64::from(b'E') << 8) | (0x20 + 1);
    // SAFETY: the kernel writes at most `LEN` bytes into `bits`.
    let n = unsafe { libc::ioctl(fd.as_raw_fd(), request as _, bits.as_mut_ptr()) };
    (n > 0).then_some(bits)
}

fn has_key(bits: &[u8], code: u16) -> bool {
    bits.get(usize::from(code / 8))
        .is_some_and(|b| b & (1 << (code % 8)) != 0)
}

/// Keyboards and mice: devices with Shift or a left button.
fn interesting(fd: &OwnedFd) -> bool {
    key_bits(fd).is_some_and(|bits| {
        has_key(&bits, code::KEY_LEFTSHIFT) || has_key(&bits, code::BTN_LEFT)
    })
}

/// Whether Scrawl may read at least one keyboard or mouse.
pub fn devices_readable() -> bool {
    event_devices()
        .iter()
        .filter_map(|p| open(p))
        .any(|fd| interesting(&fd))
}

/// Starts reading every keyboard and mouse (once). `false` when none can be read.
pub fn start() -> bool {
    READER
        .get_or_init(|| {
            let fds: Vec<OwnedFd> = event_devices()
                .iter()
                .filter_map(|p| open(p))
                .filter(interesting)
                .collect();
            if fds.is_empty() {
                return None;
            }
            crate::capture::scrawl_log(format!("input devices: reading {}", fds.len()));
            std::thread::Builder::new()
                .name("scrawl-evdev".into())
                .spawn(move || read_loop(fds))
                .ok()?;
            Some(Reader {
                held: Mutex::new(Held::default()),
            })
        })
        .is_some()
}

/// What is held, when the devices are being read.
pub fn held() -> Option<Held> {
    READER.get()?.as_ref().map(|r| {
        r.held
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    })
}

fn read_loop(fds: Vec<OwnedFd>) {
    // struct input_event on 64-bit Linux: timeval (16), type (2), code (2), value (4).
    const SIZE: usize = 24;
    let mut polls: Vec<libc::pollfd> = fds
        .iter()
        .map(|fd| libc::pollfd {
            fd: fd.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        })
        .collect();
    let mut buffer = [0_u8; SIZE * 64];
    loop {
        // SAFETY: `polls` outlives the call and its length is passed with it.
        let ready = unsafe { libc::poll(polls.as_mut_ptr(), polls.len() as _, 1_000) };
        if ready < 0 {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        for poll in &mut polls {
            if poll.revents & libc::POLLIN == 0 {
                continue;
            }
            // SAFETY: reads into `buffer`, at most its length.
            let n = unsafe { libc::read(poll.fd, buffer.as_mut_ptr().cast(), buffer.len()) };
            if n <= 0 {
                continue;
            }
            for event in buffer[..n as usize].chunks_exact(SIZE) {
                let kind = u16::from_ne_bytes([event[16], event[17]]);
                let code = u16::from_ne_bytes([event[18], event[19]]);
                let value = i32::from_ne_bytes([event[20], event[21], event[22], event[23]]);
                if kind != EV_KEY || value == 2 {
                    continue;
                }
                let down = value == 1;
                if let Some(reader) = READER.get().and_then(Option::as_ref) {
                    let mut held = reader
                        .held
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    if down {
                        held.keys.insert(code);
                    } else {
                        held.keys.remove(&code);
                    }
                }
            }
        }
    }
}

/// The evdev codes of A–Z, on a US layout's positions.
const LETTERS: [u16; 26] = [
    30, 48, 46, 32, 18, 33, 34, 35, 23, 36, 37, 38, 50, 49, 24, 25, 16, 19, 31, 20, 22, 47, 17, 45,
    21, 44,
];

/// The Windows-style virtual key (what Scrawl stores shortcuts as on Linux) for an evdev code:
/// letters, digits and F1–F12 on a US layout's positions, plus Space, Enter, Tab, Backspace,
/// Delete and Esc.
pub fn vk_for_evdev_code(code: u16) -> Option<u16> {
    if let Some(i) = LETTERS.iter().position(|&c| c == code) {
        return Some(0x41 + i as u16);
    }
    Some(match code {
        2..=10 => 0x31 + code - 2,
        11 => 0x30,
        59..=68 => 0x70 + code - 59,
        87 => 0x7A,
        88 => 0x7B,
        57 => 0x20,
        28 => 0x0D,
        15 => 0x09,
        14 => 0x08,
        111 => 0x2E,
        code::KEY_ESC => 0x1B,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evdev_codes_map_to_virtual_keys() {
        assert_eq!(vk_for_evdev_code(31), Some(u16::from(b'S')));
        assert_eq!(vk_for_evdev_code(47), Some(u16::from(b'V')));
        assert_eq!(vk_for_evdev_code(44), Some(u16::from(b'Z')));
        assert_eq!(vk_for_evdev_code(2), Some(u16::from(b'1')));
        assert_eq!(vk_for_evdev_code(11), Some(u16::from(b'0')));
        assert_eq!(vk_for_evdev_code(59), Some(0x70));
        assert_eq!(vk_for_evdev_code(88), Some(0x7B));
        assert_eq!(vk_for_evdev_code(code::KEY_LEFTSHIFT), None);
    }
}
