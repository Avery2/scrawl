//! First-run onboarding: a welcome, how Scrawl works, each permission one at a time, and a
//! first capture. The window is SwiftUI in `scrawl-helper onboarding`; this module decides which
//! step it shows (`Flow`) and remembers progress in `~/.config/scrawl/config.json`
//! (`"onboarding"`, see `Progress`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    ScreenRecording,
    Microphone,
    InputMonitoring,
    Accessibility,
    Camera,
}

impl Permission {
    /// A first capture needs these. Accessibility only serves ⌥⇧V's paste, Camera the iPhone.
    pub fn required(self) -> bool {
        matches!(
            self,
            Self::ScreenRecording | Self::Microphone | Self::InputMonitoring
        )
    }

    pub fn step(self) -> Step {
        match self {
            Self::ScreenRecording => Step::ScreenRecording,
            Self::Microphone => Step::Microphone,
            Self::InputMonitoring => Step::InputMonitoring,
            Self::Accessibility => Step::Accessibility,
            Self::Camera => Step::Camera,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Step {
    Welcome,
    /// How it works: tap ⌥⇧S to record, hold to switch.
    Record,
    /// How it works: talk, draw with ⇧, clicks.
    Draw,
    /// How it works: tap again to finish, paste.
    Finish,
    ScreenRecording,
    Microphone,
    InputMonitoring,
    Accessibility,
    Camera,
    TryIt,
    /// After the first capture: it's on your clipboard.
    Done,
}

impl Step {
    const TOUR: [Step; 11] = [
        Step::Welcome,
        Step::Record,
        Step::Draw,
        Step::Finish,
        Step::ScreenRecording,
        Step::Microphone,
        Step::InputMonitoring,
        Step::Accessibility,
        Step::Camera,
        Step::TryIt,
        Step::Done,
    ];

    pub fn permission(self) -> Option<Permission> {
        match self {
            Self::ScreenRecording => Some(Permission::ScreenRecording),
            Self::Microphone => Some(Permission::Microphone),
            Self::InputMonitoring => Some(Permission::InputMonitoring),
            Self::Accessibility => Some(Permission::Accessibility),
            Self::Camera => Some(Permission::Camera),
            _ => None,
        }
    }

    fn position(self) -> usize {
        Self::TOUR.iter().position(|s| *s == self).unwrap_or(0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    #[default]
    NotDetermined,
    Denied,
    Granted,
}

/// What macOS has granted Scrawl, from a fresh `scrawl-helper permissions`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants {
    pub statuses: BTreeMap<Permission, Status>,
    /// Screen recording is granted, but this running copy of Scrawl can't use it until it
    /// relaunches.
    pub screen_needs_relaunch: bool,
}

impl Grants {
    pub fn status(&self, permission: Permission) -> Status {
        self.statuses.get(&permission).copied().unwrap_or_default()
    }

    pub fn granted(&self, permission: Permission) -> bool {
        self.status(permission) == Status::Granted
    }

    /// Granted and usable now (screen recording may need a relaunch first).
    fn usable(&self, permission: Permission) -> bool {
        self.granted(permission)
            && !(permission == Permission::ScreenRecording && self.screen_needs_relaunch)
    }
}

/// A granted permission's step shows its check this long before moving on by itself.
pub const AUTO_ADVANCE_MS: u64 = 800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The whole tour, first launch or "Welcome to Scrawl…".
    Tour,
    /// One permission that was granted before and is missing now; closes once it's back.
    Fix,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// The primary button.
    Continue,
    Back,
    /// "Later" / "Skip for now" on a permission step.
    Later,
    Relaunch,
    /// The window was closed.
    Closed,
    Grants {
        grants: Grants,
        now_ms: u64,
    },
    CaptureStarted,
    CaptureEnded {
        completed: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Ask macOS (the system prompt), the first time the button is pressed.
    Request(Permission),
    /// Already asked or denied: open its System Settings pane.
    OpenSettings(Permission),
    /// Relaunch Scrawl; the tour resumes at `resume` (a fix has nothing to resume).
    Relaunch {
        resume: Option<Step>,
    },
    Hide,
    Show,
    Close,
}

/// What the window shows, sent to the helper as JSON.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub step: Step,
    /// Position among the steps still to be shown, for the page dots.
    pub index: usize,
    pub count: usize,
    pub status: Status,
    /// The button was pressed once already: it now opens System Settings.
    pub requested: bool,
    pub relaunch: bool,
    /// Required permissions that are still off, for the last step.
    pub missing: Vec<Permission>,
    pub fix: bool,
}

#[derive(Clone, Debug)]
pub struct Flow {
    mode: Mode,
    step: Step,
    grants: Option<Grants>,
    skipped: BTreeSet<Permission>,
    requested: BTreeSet<Permission>,
    granted_at: Option<u64>,
    hidden: bool,
}

impl Flow {
    /// The tour from `start`, passing permissions already granted.
    pub fn tour(start: Step, grants: Option<Grants>) -> Self {
        let mut flow = Self {
            mode: Mode::Tour,
            step: start,
            grants,
            skipped: BTreeSet::new(),
            requested: BTreeSet::new(),
            granted_at: None,
            hidden: false,
        };
        flow.step = flow.forward_from(start);
        flow
    }

    pub fn fix(permission: Permission, grants: Option<Grants>) -> Self {
        Self {
            mode: Mode::Fix,
            step: permission.step(),
            grants,
            skipped: BTreeSet::new(),
            requested: BTreeSet::new(),
            granted_at: None,
            hidden: false,
        }
    }

    pub fn step(&self) -> Step {
        self.step
    }

    #[cfg(test)]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn on(&mut self, input: Input) -> Effect {
        match input {
            Input::Continue => self.primary(),
            Input::Back => {
                self.step = self.back_from(self.step);
                self.granted_at = None;
                Effect::None
            }
            Input::Later => match self.step.permission() {
                Some(permission) => {
                    self.skipped.insert(permission);
                    self.advance()
                }
                None => Effect::None,
            },
            Input::Relaunch => self.relaunch(),
            Input::Closed => Effect::Close,
            Input::Grants { grants, now_ms } => {
                for permission in grants.statuses.keys() {
                    if grants.granted(*permission) {
                        self.skipped.remove(permission);
                    }
                }
                self.grants = Some(grants);
                self.tick(now_ms)
            }
            Input::CaptureStarted if !self.hidden => {
                self.hidden = true;
                Effect::Hide
            }
            Input::CaptureStarted => Effect::None,
            Input::CaptureEnded { completed } => {
                if completed && self.step == Step::TryIt {
                    self.step = Step::Done;
                }
                if std::mem::take(&mut self.hidden) {
                    Effect::Show
                } else {
                    Effect::None
                }
            }
        }
    }

    pub fn view(&self) -> View {
        let shown: Vec<Step> = Step::TOUR
            .into_iter()
            .filter(|s| *s != Step::Done)
            .filter(|s| *s == self.step || !self.passes(*s))
            .collect();
        let permission = self.step.permission();
        View {
            step: self.step,
            index: shown.iter().position(|s| *s == self.step).unwrap_or(0),
            count: shown.len(),
            status: permission.map_or(Status::NotDetermined, |p| self.status(p)),
            requested: permission.is_some_and(|p| self.requested.contains(&p)),
            relaunch: permission == Some(Permission::ScreenRecording)
                && self
                    .grants
                    .as_ref()
                    .is_some_and(|g| g.screen_needs_relaunch),
            missing: self.missing(),
            fix: self.mode == Mode::Fix,
        }
    }

    /// Required permissions that are off: from the grants once known, else the ones skipped.
    pub fn missing(&self) -> Vec<Permission> {
        match &self.grants {
            Some(grants) => [
                Permission::ScreenRecording,
                Permission::Microphone,
                Permission::InputMonitoring,
            ]
            .into_iter()
            .filter(|p| !grants.granted(*p))
            .collect(),
            None => self
                .skipped
                .iter()
                .copied()
                .filter(|p| p.required())
                .collect(),
        }
    }

    fn status(&self, permission: Permission) -> Status {
        self.grants
            .as_ref()
            .map_or(Status::NotDetermined, |g| g.status(permission))
    }

    fn usable(&self, permission: Permission) -> bool {
        self.grants.as_ref().is_some_and(|g| g.usable(permission))
    }

    /// A permission step the tour passes because it's already granted.
    fn passes(&self, step: Step) -> bool {
        step.permission().is_some_and(|p| self.usable(p))
    }

    fn forward_from(&self, mut step: Step) -> Step {
        while self.passes(step) {
            step = Step::TOUR[(step.position() + 1).min(Step::TOUR.len() - 1)];
        }
        step
    }

    fn back_from(&self, step: Step) -> Step {
        let mut i = step.position();
        while i > 0 {
            i -= 1;
            if !self.passes(Step::TOUR[i]) {
                return Step::TOUR[i];
            }
        }
        step
    }

    fn advance(&mut self) -> Effect {
        self.granted_at = None;
        if self.mode == Mode::Fix {
            return Effect::Close;
        }
        let next = Step::TOUR[(self.step.position() + 1).min(Step::TOUR.len() - 1)];
        self.step = self.forward_from(next);
        Effect::None
    }

    fn primary(&mut self) -> Effect {
        match self.step {
            Step::TryIt | Step::Done => Effect::Close,
            step => match step.permission() {
                None => self.advance(),
                Some(permission) if self.usable(permission) => self.advance(),
                Some(Permission::ScreenRecording)
                    if self
                        .grants
                        .as_ref()
                        .is_some_and(|g| g.screen_needs_relaunch) =>
                {
                    self.relaunch()
                }
                Some(permission) => {
                    if self.requested.insert(permission)
                        && self.status(permission) != Status::Denied
                    {
                        Effect::Request(permission)
                    } else {
                        Effect::OpenSettings(permission)
                    }
                }
            },
        }
    }

    fn relaunch(&self) -> Effect {
        if self.step != Step::ScreenRecording {
            return Effect::None;
        }
        let resume = match self.mode {
            Mode::Tour => Some(Step::TOUR[self.step.position() + 1]),
            Mode::Fix => None,
        };
        Effect::Relaunch { resume }
    }

    fn tick(&mut self, now_ms: u64) -> Effect {
        let Some(permission) = self.step.permission() else {
            self.granted_at = None;
            return Effect::None;
        };
        if !self.usable(permission) {
            self.granted_at = None;
            return Effect::None;
        }
        let since = *self.granted_at.get_or_insert(now_ms);
        if now_ms.saturating_sub(since) >= AUTO_ADVANCE_MS {
            self.advance()
        } else {
            Effect::None
        }
    }
}

/// Onboarding progress, under `"onboarding"` in `~/.config/scrawl/config.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// The tour was finished or closed; it doesn't open by itself again.
    #[serde(default)]
    pub seen: bool,
    /// Where to pick up after "Relaunch Scrawl".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<Step>,
    /// Every permission seen granted, so a later loss reopens just that step.
    #[serde(default)]
    pub granted: BTreeSet<Permission>,
}

impl Progress {
    pub fn load() -> Self {
        Self::load_from(&crate::handoff::default_config_path())
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .and_then(|config| config.get("onboarding").cloned())
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let _ = self.save_to(&crate::handoff::default_config_path());
    }

    /// Writes `"onboarding"`, keeping the rest of the config as it is.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        let mut config = std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .filter(serde_json::Value::is_object)
            .unwrap_or_else(|| serde_json::json!({}));
        config["onboarding"] = serde_json::to_value(self)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(&config)?)?;
        Ok(())
    }

    /// Remembers what's granted now; returns whether anything new was.
    pub fn note(&mut self, grants: &Grants) -> bool {
        let before = self.granted.len();
        self.granted.extend(
            grants
                .statuses
                .iter()
                .filter(|(_, status)| **status == Status::Granted)
                .map(|(permission, _)| *permission),
        );
        self.granted.len() != before
    }
}

/// What opens at launch: the tour until it's been seen (resuming after a relaunch), else the
/// step for a required permission that was granted once and is gone now, else nothing.
pub fn launch_plan(progress: &Progress, grants: &Grants) -> Option<Flow> {
    if !progress.seen {
        let start = progress.resume.unwrap_or(Step::Welcome);
        return Some(Flow::tour(start, Some(grants.clone())));
    }
    [
        Permission::ScreenRecording,
        Permission::Microphone,
        Permission::InputMonitoring,
    ]
    .into_iter()
    .find(|p| progress.granted.contains(p) && !grants.granted(*p))
    .map(|p| Flow::fix(p, Some(grants.clone())))
}

/// From the welcome window's helper process and the permission poller.
#[derive(Clone, Debug)]
pub enum Event {
    /// Grants read at launch, to decide whether onboarding opens (`launch_plan`).
    Launch(Grants),
    Grants(Grants),
    /// A button: `continue`, `back`, `later`, `relaunch` or `close`.
    Action(String),
    Exited,
}

/// Reads what's granted now: a fresh helper for each permission, plus whether this process
/// can use a Screen Recording grant yet.
pub fn read_grants() -> Option<Grants> {
    let statuses = crate::helper::permissions()
        .inspect_err(|error| crate::capture::scrawl_log(format!("permissions: {error:#}")))
        .ok()?;
    let screen = statuses.get(&Permission::ScreenRecording) == Some(&Status::Granted);
    Some(Grants {
        statuses,
        screen_needs_relaunch: screen && !crate::capture::screen_capture_granted(),
    })
}

/// The open welcome window: the `Flow`, the helper drawing it, and a poller re-reading the
/// grants every 0.5 s so a step's check appears by itself.
pub struct Session {
    flow: Flow,
    child: std::process::Child,
    stdin: Option<std::process::ChildStdin>,
    polling: std::sync::Arc<std::sync::atomic::AtomicBool>,
    last_view: String,
    clock: std::time::Instant,
}

impl Session {
    pub fn open(
        flow: Flow,
        shortcut: &str,
        events: impl Fn(Event) + Send + Sync + Clone + 'static,
    ) -> anyhow::Result<Self> {
        use std::io::BufRead;
        use std::process::{Command, Stdio};
        use std::sync::atomic::{AtomicBool, Ordering};

        let mut command = Command::new(crate::helper::path()?);
        command
            .args(["onboarding", "--shortcut", shortcut, "--paste", "⌥⇧V"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let dev_icon = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/app-icon.png");
        if Path::new(dev_icon).is_file() {
            command.args(["--icon", dev_icon]);
        }
        let mut child = command.spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();

        let reader_events = events.clone();
        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                for line in std::io::BufReader::new(stdout)
                    .lines()
                    .map_while(Result::ok)
                {
                    let action = serde_json::from_str::<serde_json::Value>(&line)
                        .ok()
                        .and_then(|v| v.get("action")?.as_str().map(str::to_owned));
                    if let Some(action) = action {
                        reader_events(Event::Action(action));
                    }
                }
            }
            reader_events(Event::Exited);
        });

        let polling = std::sync::Arc::new(AtomicBool::new(true));
        let alive = std::sync::Arc::clone(&polling);
        std::thread::spawn(move || {
            while alive.load(Ordering::Acquire) {
                if let Some(grants) = read_grants()
                    && alive.load(Ordering::Acquire)
                {
                    events(Event::Grants(grants));
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        });

        let mut session = Self {
            flow,
            child,
            stdin,
            polling,
            last_view: String::new(),
            clock: std::time::Instant::now(),
        };
        session.sync();
        Ok(session)
    }

    /// Feeds the flow and carries out what the window can do itself. Returns what needs the
    /// app: `Close` or `Relaunch`.
    pub fn input(&mut self, input: Input) -> Effect {
        let effect = self.flow.on(input);
        crate::capture::scrawl_log(format!("onboarding: {:?} -> {effect:?}", self.flow.step()));
        let result = match effect {
            Effect::Request(Permission::ScreenRecording) => {
                crate::capture::request_screen_capture();
                Effect::None
            }
            Effect::Request(Permission::InputMonitoring) => {
                crate::capture::request_listen_access();
                Effect::None
            }
            Effect::Request(permission) => {
                self.send(&serde_json::json!({"type": "request", "permission": permission}));
                Effect::None
            }
            Effect::OpenSettings(permission) => {
                self.send(&serde_json::json!({"type": "open-settings", "permission": permission}));
                Effect::None
            }
            Effect::Hide => {
                self.send(&serde_json::json!({"type": "hide"}));
                Effect::None
            }
            Effect::Show => {
                self.send(&serde_json::json!({"type": "show"}));
                Effect::None
            }
            other => other,
        };
        self.sync();
        result
    }

    /// An action string from the helper as an `Input`.
    pub fn action(&mut self, action: &str) -> Effect {
        let input = match action {
            "continue" => Input::Continue,
            "back" => Input::Back,
            "later" => Input::Later,
            "relaunch" => Input::Relaunch,
            "close" => Input::Closed,
            _ => return Effect::None,
        };
        self.input(input)
    }

    pub fn grants(&mut self, grants: Grants) -> Effect {
        let now_ms = self.clock.elapsed().as_millis() as u64;
        self.input(Input::Grants { grants, now_ms })
    }

    pub fn show(&mut self) {
        self.send(&serde_json::json!({"type": "show"}));
    }

    fn sync(&mut self) {
        let mut view = serde_json::to_value(self.flow.view()).unwrap_or_default();
        view["type"] = "state".into();
        let text = view.to_string();
        if text != self.last_view {
            self.send(&view);
            self.last_view = text;
        }
    }

    fn send(&mut self, message: &serde_json::Value) {
        use std::io::Write;
        if let Some(stdin) = self.stdin.as_mut()
            && writeln!(stdin, "{message}")
                .and_then(|()| stdin.flush())
                .is_err()
        {
            self.stdin = None;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.polling
            .store(false, std::sync::atomic::Ordering::Release);
        self.send(&serde_json::json!({"type": "close"}));
        self.stdin = None;
        let _ = self.child.wait();
    }
}

/// Starts Scrawl again a moment after this copy quits (after a Screen Recording grant).
pub fn relaunch() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let bundle = exe
        .ancestors()
        .find(|p| p.extension().is_some_and(|ext| ext == "app"));
    let mut command = std::process::Command::new("/bin/sh");
    match bundle {
        Some(app) => command
            .args(["-c", "sleep 1; exec /usr/bin/open \"$0\""])
            .arg(app),
        None => command
            .args(["-c", "sleep 1; exec \"$0\" \"$@\""])
            .arg(&exe)
            .args(std::env::args_os().skip(1)),
    };
    let _ = command.spawn();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grants(granted: &[Permission]) -> Grants {
        Grants {
            statuses: granted.iter().map(|p| (*p, Status::Granted)).collect(),
            screen_needs_relaunch: false,
        }
    }

    fn tick(flow: &mut Flow, g: &Grants, now_ms: u64) -> Effect {
        flow.on(Input::Grants {
            grants: g.clone(),
            now_ms,
        })
    }

    #[test]
    fn the_tour_explains_then_asks_for_each_permission_in_order() {
        let none = grants(&[]);
        let mut flow = Flow::tour(Step::Welcome, Some(none.clone()));
        let mut seen = vec![flow.step()];
        while flow.step() != Step::TryIt {
            let effect = if flow.step().permission().is_some() {
                flow.on(Input::Later)
            } else {
                flow.on(Input::Continue)
            };
            assert_eq!(effect, Effect::None);
            seen.push(flow.step());
        }
        assert_eq!(
            seen,
            [
                Step::Welcome,
                Step::Record,
                Step::Draw,
                Step::Finish,
                Step::ScreenRecording,
                Step::Microphone,
                Step::InputMonitoring,
                Step::Accessibility,
                Step::Camera,
                Step::TryIt
            ]
        );
        assert_eq!(
            flow.view().missing,
            [
                Permission::ScreenRecording,
                Permission::Microphone,
                Permission::InputMonitoring
            ]
        );
        assert_eq!(flow.on(Input::Back), Effect::None);
        assert_eq!(flow.step(), Step::Camera);
    }

    #[test]
    fn granted_permissions_are_passed_and_left_out_of_the_dots() {
        let some = grants(&[Permission::ScreenRecording, Permission::InputMonitoring]);
        let mut flow = Flow::tour(Step::Finish, Some(some));
        assert_eq!(flow.view().count, 8);
        flow.on(Input::Continue);
        assert_eq!(flow.step(), Step::Microphone);
        assert_eq!(flow.view().index, 4);
        flow.on(Input::Later);
        assert_eq!(flow.step(), Step::Accessibility);
        // Back passes the granted steps too.
        flow.on(Input::Back);
        assert_eq!(flow.step(), Step::Microphone);
        flow.on(Input::Back);
        assert_eq!(flow.step(), Step::Finish);
        // A resumed tour that starts on a granted step moves past it.
        let all = grants(&[
            Permission::ScreenRecording,
            Permission::Microphone,
            Permission::InputMonitoring,
            Permission::Accessibility,
            Permission::Camera,
        ]);
        assert_eq!(Flow::tour(Step::Microphone, Some(all)).step(), Step::TryIt);
    }

    #[test]
    fn the_button_asks_once_then_opens_settings_and_the_check_moves_on() {
        let none = grants(&[]);
        let mut flow = Flow::tour(Step::Microphone, Some(none.clone()));
        assert_eq!(
            flow.on(Input::Continue),
            Effect::Request(Permission::Microphone)
        );
        assert!(flow.view().requested);
        assert_eq!(
            flow.on(Input::Continue),
            Effect::OpenSettings(Permission::Microphone)
        );
        assert_eq!(tick(&mut flow, &none, 1_000), Effect::None);
        // Granted: the check shows, then the step moves on by itself.
        let mic = grants(&[Permission::Microphone]);
        assert_eq!(tick(&mut flow, &mic, 1_500), Effect::None);
        assert_eq!(flow.view().status, Status::Granted);
        assert_eq!(tick(&mut flow, &mic, 2_000), Effect::None);
        assert_eq!(flow.step(), Step::Microphone);
        assert_eq!(tick(&mut flow, &mic, 1_500 + AUTO_ADVANCE_MS), Effect::None);
        assert_eq!(flow.step(), Step::InputMonitoring);
        // Continue on a granted step moves on at once.
        let mut flow = Flow::tour(Step::Microphone, Some(none));
        tick(&mut flow, &mic, 0);
        assert_eq!(flow.on(Input::Continue), Effect::None);
        assert_eq!(flow.step(), Step::InputMonitoring);
    }

    #[test]
    fn a_denied_permission_goes_straight_to_settings() {
        let mut denied = grants(&[]);
        denied.statuses.insert(Permission::Camera, Status::Denied);
        let mut flow = Flow::tour(Step::Camera, Some(denied));
        assert_eq!(
            flow.on(Input::Continue),
            Effect::OpenSettings(Permission::Camera)
        );
    }

    #[test]
    fn screen_recording_granted_but_not_usable_offers_a_relaunch_that_resumes_next() {
        let mut stale = grants(&[Permission::ScreenRecording]);
        stale.screen_needs_relaunch = true;
        let mut flow = Flow::tour(Step::ScreenRecording, Some(stale.clone()));
        assert_eq!(flow.step(), Step::ScreenRecording);
        assert_eq!(tick(&mut flow, &stale, 0), Effect::None);
        assert_eq!(tick(&mut flow, &stale, 5_000), Effect::None);
        assert!(flow.view().relaunch);
        let relaunch = Effect::Relaunch {
            resume: Some(Step::Microphone),
        };
        assert_eq!(flow.on(Input::Continue), relaunch);
        assert_eq!(flow.on(Input::Relaunch), relaunch);
        // A fix just relaunches; elsewhere there's nothing to relaunch for.
        let mut flow = Flow::fix(Permission::ScreenRecording, Some(stale.clone()));
        assert_eq!(flow.on(Input::Continue), Effect::Relaunch { resume: None });
        let mut flow = Flow::tour(Step::Microphone, Some(stale));
        assert_eq!(flow.on(Input::Relaunch), Effect::None);
    }

    #[test]
    fn a_capture_hides_the_window_and_finishing_from_try_it_shows_done() {
        let mut flow = Flow::tour(Step::TryIt, Some(grants(&[])));
        assert_eq!(flow.on(Input::CaptureStarted), Effect::Hide);
        assert_eq!(flow.on(Input::CaptureStarted), Effect::None);
        assert_eq!(
            flow.on(Input::CaptureEnded { completed: true }),
            Effect::Show
        );
        assert_eq!(flow.step(), Step::Done);
        assert_eq!(flow.on(Input::Continue), Effect::Close);
        // A cancelled capture comes back to the same step.
        let mut flow = Flow::tour(Step::TryIt, None);
        flow.on(Input::CaptureStarted);
        assert_eq!(
            flow.on(Input::CaptureEnded { completed: false }),
            Effect::Show
        );
        assert_eq!(flow.step(), Step::TryIt);
        // Recording from the middle of the tour returns to it.
        let mut flow = Flow::tour(Step::Draw, None);
        flow.on(Input::CaptureStarted);
        flow.on(Input::CaptureEnded { completed: true });
        assert_eq!(flow.step(), Step::Draw);
        assert_eq!(flow.on(Input::Closed), Effect::Close);
    }

    #[test]
    fn a_fix_shows_one_step_and_closes_once_granted_or_put_off() {
        let none = grants(&[]);
        let mut flow = Flow::fix(Permission::InputMonitoring, Some(none.clone()));
        assert!(flow.view().fix);
        assert_eq!(
            flow.on(Input::Continue),
            Effect::Request(Permission::InputMonitoring)
        );
        let back = grants(&[Permission::InputMonitoring]);
        tick(&mut flow, &back, 0);
        assert_eq!(tick(&mut flow, &back, AUTO_ADVANCE_MS), Effect::Close);
        let mut flow = Flow::fix(Permission::Microphone, Some(none));
        assert_eq!(flow.on(Input::Later), Effect::Close);
    }

    #[test]
    fn launch_shows_the_tour_once_then_only_a_lost_permission() {
        let all = grants(&[
            Permission::ScreenRecording,
            Permission::Microphone,
            Permission::InputMonitoring,
        ]);
        let mut progress = Progress::default();
        assert_eq!(
            launch_plan(&progress, &all).map(|f| (f.mode(), f.step())),
            Some((Mode::Tour, Step::Welcome))
        );
        // After "Relaunch Scrawl": back in the tour where it left off.
        progress.resume = Some(Step::Microphone);
        assert_eq!(
            launch_plan(&progress, &grants(&[])).map(|f| f.step()),
            Some(Step::Microphone)
        );
        progress.resume = None;
        progress.seen = true;
        assert!(progress.note(&all));
        assert!(!progress.note(&all));
        assert!(launch_plan(&progress, &all).is_none());
        // Input Monitoring went away: just that step.
        let lost = grants(&[Permission::ScreenRecording, Permission::Microphone]);
        let flow = launch_plan(&progress, &lost).expect("a fix");
        assert_eq!(
            (flow.mode(), flow.step()),
            (Mode::Fix, Step::InputMonitoring)
        );
        // Never granted (put off in the tour): no nagging at launch.
        progress.granted.remove(&Permission::InputMonitoring);
        assert!(launch_plan(&progress, &lost).is_none());
    }

    #[test]
    fn progress_is_kept_beside_the_rest_of_the_config() {
        let dir = std::env::temp_dir().join(format!("scrawl-onboarding-{}", std::process::id()));
        let path = dir.join("config.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&path, r#"{"agentHost":"studio","autocrop":false}"#).unwrap();
        assert_eq!(Progress::load_from(&path), Progress::default());
        let progress = Progress {
            seen: true,
            resume: Some(Step::Microphone),
            granted: [Permission::Microphone].into_iter().collect(),
        };
        progress.save_to(&path).unwrap();
        assert_eq!(Progress::load_from(&path), progress);
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["agentHost"], "studio");
        assert_eq!(config["autocrop"], false);
        assert_eq!(config["onboarding"]["resume"], "microphone");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn helper_statuses_parse_into_grants() {
        let statuses: BTreeMap<Permission, Status> = serde_json::from_str(
            r#"{"screen-recording":"granted","microphone":"denied","camera":"not-determined"}"#,
        )
        .unwrap();
        let g = Grants {
            statuses,
            screen_needs_relaunch: false,
        };
        assert!(g.granted(Permission::ScreenRecording));
        assert_eq!(g.status(Permission::Microphone), Status::Denied);
        assert_eq!(g.status(Permission::Accessibility), Status::NotDetermined);
    }
}
