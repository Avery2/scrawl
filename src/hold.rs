//! The capture shortcut. Tap ⌥⇧S to record the screen at once; the recording's capsule shows
//! Area · Window · Screen · iPhone as chips for a few seconds (`Reveal`), so the target can still
//! change. Hold it and the picker opens instead, the same one as the menu's Choose What to
//! Record…, and stays open after the keys come up: click a chip or a window, or drag an area.

/// ⌥⇧ released sooner than this after the press is a tap: record the screen, no picker.
pub const TAP_MS: u64 = 250;
/// A press this soon after a recording starts is a double press, not "finish".
pub const DOUBLE_PRESS_MS: u64 = 400;

/// What Scrawl is doing when the shortcut goes down.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity {
    /// Nothing recording (a finished capture's island may be showing).
    Free,
    /// The picker is open.
    Picker,
    Recording {
        elapsed_ms: u64,
    },
    Processing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// A tap: record the display under the pointer, auto-cropped.
    RecordScreen,
    /// A hold: open the picker.
    OpenPicker,
    /// Esc before a tap or hold was decided: nothing happens.
    Cancel,
    /// The picker is open: record what it is on.
    RecordSelected,
    Finish,
}

/// Tells a tap of the shortcut from a hold.
#[derive(Debug, Default)]
pub struct Shortcut {
    /// When ⌥⇧S went down, until it's a tap or a hold.
    pending_since_ms: Option<u64>,
}

impl Shortcut {
    /// Whether ⌥⇧S went down and it isn't yet known whether it's a tap or a hold.
    pub fn is_pending(&self) -> bool {
        self.pending_since_ms.is_some()
    }

    /// The shortcut went down.
    pub fn press(&mut self, now_ms: u64, activity: Activity) -> Action {
        if self.is_pending() {
            // Pressed again before the first was decided: as good as a hold.
            self.pending_since_ms = None;
            return Action::OpenPicker;
        }
        match activity {
            Activity::Recording { elapsed_ms } if elapsed_ms < DOUBLE_PRESS_MS => Action::None,
            Activity::Recording { .. } => Action::Finish,
            Activity::Processing => Action::None,
            Activity::Picker => Action::RecordSelected,
            Activity::Free => {
                self.pending_since_ms = Some(now_ms);
                Action::None
            }
        }
    }

    /// Polled every few ms while pending, with whether the shortcut's modifiers are all still
    /// down.
    pub fn tick(&mut self, now_ms: u64, held: bool) -> Action {
        let Some(since_ms) = self.pending_since_ms else {
            return Action::None;
        };
        if !held {
            self.pending_since_ms = None;
            Action::RecordScreen
        } else if now_ms.saturating_sub(since_ms) >= TAP_MS {
            self.pending_since_ms = None;
            Action::OpenPicker
        } else {
            Action::None
        }
    }

    pub fn escape(&mut self) -> Action {
        if self.pending_since_ms.take().is_some() {
            Action::Cancel
        } else {
            Action::None
        }
    }
}

/// How long a tap-started recording shows its switch chips when nothing else happens.
pub const REVEAL_MS: u64 = 3_500;
/// The chips stay at least this long even if work starts at once, so they are seen.
pub const REVEAL_MIN_MS: u64 = 900;
/// After the pointer leaves the capsule the chips stay this much longer.
pub const REVEAL_LINGER_MS: u64 = 1_200;

/// The switch chips a tap-started recording shows in its capsule (Area · Window · Screen ·
/// iPhone), so a tap teaches that the target can change. They fold back into the recording pill
/// after `REVEAL_MS`, or once the user starts working (a click, scroll or stroke), but not while
/// the pointer is on the capsule.
#[derive(Debug, Default)]
pub struct Reveal {
    open: Option<RevealOpen>,
}

#[derive(Clone, Copy, Debug)]
struct RevealOpen {
    since_ms: u64,
    /// Activity count when the chips appeared; more than this means the user got to work.
    baseline: usize,
    hovered_at_ms: Option<u64>,
}

impl Reveal {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn show(&mut self, now_ms: u64, activity: usize) {
        self.open = Some(RevealOpen {
            since_ms: now_ms,
            baseline: activity,
            hovered_at_ms: None,
        });
    }

    pub fn close(&mut self) {
        self.open = None;
    }

    /// Polled every frame; returns true on the frame the chips fold away.
    pub fn tick(&mut self, now_ms: u64, hovered: bool, activity: usize) -> bool {
        let Some(open) = self.open.as_mut() else {
            return false;
        };
        if hovered {
            open.hovered_at_ms = Some(now_ms);
            return false;
        }
        let age = now_ms.saturating_sub(open.since_ms);
        if age < REVEAL_MIN_MS {
            return false;
        }
        let linger_until = open.hovered_at_ms.map_or(0, |at| at + REVEAL_LINGER_MS);
        let deadline = (open.since_ms + REVEAL_MS).max(linger_until);
        let working = activity > open.baseline && now_ms >= linger_until;
        if now_ms >= deadline || working {
            self.open = None;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREE: Activity = Activity::Free;

    #[test]
    fn a_tap_records_the_screen_without_opening_the_picker() {
        let mut s = Shortcut::default();
        assert_eq!(s.press(0, FREE), Action::None);
        assert_eq!(s.tick(8, true), Action::None);
        assert_eq!(s.tick(120, true), Action::None);
        assert_eq!(s.tick(128, false), Action::RecordScreen);
        assert!(!s.is_pending());
        assert_eq!(s.tick(200, false), Action::None);
    }

    #[test]
    fn holding_opens_the_picker_and_letting_go_does_nothing_more() {
        let mut s = Shortcut::default();
        s.press(0, FREE);
        assert_eq!(s.tick(240, true), Action::None);
        assert_eq!(s.tick(250, true), Action::OpenPicker);
        assert!(!s.is_pending());
        assert_eq!(s.tick(900, true), Action::None);
        assert_eq!(s.tick(1_400, false), Action::None);
        // Another S while it's up records what the picker is on, like the menu's picker.
        assert_eq!(s.press(1_500, Activity::Picker), Action::RecordSelected);
    }

    #[test]
    fn a_quick_second_press_opens_the_picker() {
        let mut s = Shortcut::default();
        s.press(0, FREE);
        assert_eq!(s.press(150, FREE), Action::OpenPicker);
        assert_eq!(s.tick(160, false), Action::None);
    }

    #[test]
    fn esc_before_a_tap_or_hold_is_decided_cancels_it() {
        let mut s = Shortcut::default();
        s.press(0, FREE);
        assert_eq!(s.escape(), Action::Cancel);
        assert_eq!(s.tick(100, false), Action::None);
        assert_eq!(s.tick(400, true), Action::None);
        assert_eq!(s.escape(), Action::None);
    }

    #[test]
    fn a_tap_while_recording_finishes_but_not_a_double_press() {
        let mut s = Shortcut::default();
        let at = |elapsed_ms| Activity::Recording { elapsed_ms };
        assert_eq!(s.press(0, at(0)), Action::None);
        assert_eq!(s.press(0, at(399)), Action::None);
        assert_eq!(s.press(0, at(400)), Action::Finish);
        assert_eq!(s.press(0, at(60_000)), Action::Finish);
        // Releasing ⌥⇧ after finishing doesn't start anything.
        assert!(!s.is_pending());
        assert_eq!(s.tick(100, false), Action::None);
        assert_eq!(s.press(0, Activity::Processing), Action::None);
        assert!(!s.is_pending());
    }

    #[test]
    fn reveal_folds_away_after_its_time_when_ignored() {
        let mut r = Reveal::default();
        assert!(!r.tick(0, false, 0));
        r.show(1_000, 0);
        assert!(r.is_open());
        assert!(!r.tick(1_000 + REVEAL_MS - 1, false, 0));
        assert!(r.tick(1_000 + REVEAL_MS, false, 0));
        assert!(!r.is_open());
        assert!(!r.tick(1_000 + REVEAL_MS + 8, false, 0));
    }

    #[test]
    fn reveal_folds_away_once_work_starts_but_is_seen_first() {
        let mut r = Reveal::default();
        r.show(0, 3);
        // A click right away: the chips stay for their minimum.
        assert!(!r.tick(100, false, 4));
        assert!(!r.tick(REVEAL_MIN_MS - 1, false, 4));
        assert!(r.tick(REVEAL_MIN_MS, false, 4));
        // No new activity: only the timeout folds it.
        r.show(10_000, 7);
        assert!(!r.tick(10_000 + REVEAL_MIN_MS + 500, false, 7));
        assert!(r.tick(10_000 + REVEAL_MIN_MS + 600, false, 8));
    }

    #[test]
    fn reveal_stays_while_hovered_and_lingers_after() {
        let mut r = Reveal::default();
        r.show(0, 0);
        assert!(!r.tick(3_000, true, 0));
        // Hovered past the timeout: still open.
        assert!(!r.tick(6_000, true, 5));
        // Left at 6 s: lingers, even with activity, then folds.
        assert!(!r.tick(6_000 + REVEAL_LINGER_MS - 1, false, 5));
        assert!(r.tick(6_000 + REVEAL_LINGER_MS, false, 5));
        // Choosing a chip closes it outright.
        r.show(20_000, 0);
        r.close();
        assert!(!r.is_open());
    }

    #[test]
    fn in_the_picker_the_shortcut_records_its_selection() {
        let mut s = Shortcut::default();
        assert_eq!(s.press(0, Activity::Picker), Action::RecordSelected);
        assert!(!s.is_pending());
    }
}
