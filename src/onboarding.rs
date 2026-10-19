//! First-run onboarding: a welcome, one screen on how Scrawl works, one screen with every
//! permission it needs, and a first capture. The window is SwiftUI in `scrawl-helper onboarding`;
//! this module decides what it shows (`Flow`) and remembers progress in
//! `~/.config/scrawl/config.json` (`"onboarding"`, see `Progress`).
//!
//! Screen Recording, Microphone and Input Monitoring are required and asked for together on the
//! permissions screen. Accessibility (⌥⇧V's paste) and Camera (the cabled iPhone) are asked for
//! the first time they're used, in the same window with just that row (`Flow::fix`). Until the
//! required ones are on, ⌥⇧S starts nothing: it brings this window forward once the keys are up
//! (`Gate`). A Screen Recording grant only reaches a running copy of Scrawl after it relaunches,
//! so the permissions screen relaunches at most once, at the end, and the tour picks up after it.

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
    /// What a capture needs. Accessibility only serves ⌥⇧V's paste and Camera the iPhone; both
    /// are asked for when first used.
    pub const REQUIRED: [Permission; 3] = [
        Permission::ScreenRecording,
        Permission::Microphone,
        Permission::InputMonitoring,
    ];

    pub fn required(self) -> bool {
        Self::REQUIRED.contains(&self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Step {
    Welcome,
    /// Tap ⌥⇧S to record, talk and draw with ⇧, tap again to finish and paste.
    #[serde(alias = "record", alias = "draw", alias = "finish")]
    HowItWorks,
    /// Every permission on one screen.
    #[serde(
        alias = "screen-recording",
        alias = "microphone",
        alias = "input-monitoring",
        alias = "accessibility",
        alias = "camera"
    )]
    Permissions,
    TryIt,
    /// After the first capture: it's on your clipboard.
    Done,
}

impl Step {
    const TOUR: [Step; 5] = [
        Step::Welcome,
        Step::HowItWorks,
        Step::Permissions,
        Step::TryIt,
        Step::Done,
    ];

    fn position(self) -> usize {
        Self::TOUR.iter().position(|s| *s == self).unwrap_or(0)
    }

    fn next(self) -> Step {
        Self::TOUR[(self.position() + 1).min(Self::TOUR.len() - 1)]
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

/// Once everything a fix asks for is on, its window shows the checks this long, then closes.
pub const AUTO_ADVANCE_MS: u64 = 800;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The whole tour, first launch or "Welcome to Scrawl…".
    Tour,
    /// Just the permissions screen, for permissions that are missing now: required ones that
    /// went away or that ⌥⇧S found off, or Accessibility or Camera on first use. Closes once
    /// they're on.
    Fix,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// The primary button.
    Continue,
    Back,
    /// "Set up later" / "Not now".
    Later,
    /// A permission row's button.
    Grant(Permission),
    Relaunch,
    /// The window was closed.
    Closed,
    /// "Open Settings" on the last screen.
    Settings,
    Grants {
        grants: Grants,
        now_ms: u64,
    },
    /// ⌥⇧S was pressed while a required permission is off.
    Blocked,
    CaptureStarted,
    CaptureEnded {
        completed: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    None,
    /// Ask macOS (the system prompt), the first time a row's button is pressed.
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
    /// Close the tour and open the Settings window.
    Settings,
}

/// One row on the permissions screen.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Row {
    pub permission: Permission,
    pub status: Status,
    /// The button was pressed once already: it now opens System Settings.
    pub requested: bool,
    pub required: bool,
    /// Granted, but it only takes effect when Scrawl reopens.
    pub relaunch: bool,
}

/// What the window shows, sent to the helper as JSON.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub step: Step,
    /// Position among the steps still to be shown, for the page dots.
    pub index: usize,
    pub count: usize,
    pub rows: Vec<Row>,
    /// Everything the permissions screen needs is granted: Continue is enabled.
    pub ready: bool,
    /// Continue relaunches Scrawl first.
    pub relaunch: bool,
    pub fix: bool,
}

#[derive(Clone, Debug)]
pub struct Flow {
    mode: Mode,
    step: Step,
    grants: Option<Grants>,
    /// The rows on the permissions screen.
    asks: Vec<Permission>,
    requested: BTreeSet<Permission>,
    ready_at: Option<u64>,
    hidden: bool,
}

impl Flow {
    /// The tour from `start`. The permissions screen is passed when everything on it is usable,
    /// and a later step never shows before it's done.
    pub fn tour(start: Step, grants: Option<Grants>) -> Self {
        let mut flow = Self {
            mode: Mode::Tour,
            step: start,
            grants,
            asks: Permission::REQUIRED.to_vec(),
            requested: BTreeSet::new(),
            ready_at: None,
            hidden: false,
        };
        flow.step = if start.position() > Step::Permissions.position()
            && flow.grants.is_some()
            && !flow.done()
        {
            Step::Permissions
        } else {
            flow.forward_from(start)
        };
        flow
    }

    /// The permissions screen with just `asks`.
    pub fn fix(asks: Vec<Permission>, grants: Option<Grants>) -> Self {
        Self {
            mode: Mode::Fix,
            step: Step::Permissions,
            grants,
            asks,
            requested: BTreeSet::new(),
            ready_at: None,
            hidden: false,
        }
    }

    pub fn step(&self) -> Step {
        self.step
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Where a tour picks up if Scrawl quits now (macOS's own "Quit & Reopen" included).
    pub fn resume_point(&self) -> Option<Step> {
        (self.mode == Mode::Tour && self.step != Step::Done).then_some(self.step)
    }

    pub fn on(&mut self, input: Input) -> Effect {
        match input {
            Input::Continue => self.primary(),
            Input::Back => {
                self.step = self.back_from(self.step);
                Effect::None
            }
            Input::Later | Input::Closed => Effect::Close,
            Input::Settings if self.step == Step::Done => Effect::Settings,
            Input::Settings => Effect::None,
            Input::Grant(permission) => self.grant(permission),
            Input::Relaunch => self.relaunch(),
            Input::Grants { grants, now_ms } => {
                self.grants = Some(grants);
                self.tick(now_ms)
            }
            Input::Blocked => {
                if self.mode == Mode::Tour && self.step.position() < Step::Permissions.position() {
                    self.step = Step::Permissions;
                }
                Effect::Show
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
        View {
            step: self.step,
            index: shown.iter().position(|s| *s == self.step).unwrap_or(0),
            count: shown.len(),
            rows: self
                .asks
                .iter()
                .map(|&permission| Row {
                    permission,
                    status: self.status(permission),
                    requested: self.requested.contains(&permission),
                    required: permission.required(),
                    relaunch: permission == Permission::ScreenRecording && self.screen_relaunch(),
                })
                .collect(),
            ready: self.ready(),
            relaunch: self.ready() && self.screen_relaunch(),
            fix: self.mode == Mode::Fix,
        }
    }

    fn status(&self, permission: Permission) -> Status {
        self.grants
            .as_ref()
            .map_or(Status::NotDetermined, |g| g.status(permission))
    }

    fn granted(&self, permission: Permission) -> bool {
        self.status(permission) == Status::Granted
    }

    fn usable(&self, permission: Permission) -> bool {
        self.grants.as_ref().is_some_and(|g| g.usable(permission))
    }

    /// What must be granted before Continue: a tour's required rows, every row of a fix.
    fn needed(&self) -> impl Iterator<Item = Permission> + '_ {
        self.asks
            .iter()
            .copied()
            .filter(|p| self.mode == Mode::Fix || p.required())
    }

    fn ready(&self) -> bool {
        self.needed().all(|p| self.granted(p))
    }

    /// Screen Recording is on the screen and granted, but only reaches Scrawl after a relaunch.
    fn screen_relaunch(&self) -> bool {
        self.asks.contains(&Permission::ScreenRecording)
            && self.granted(Permission::ScreenRecording)
            && self
                .grants
                .as_ref()
                .is_some_and(|g| g.screen_needs_relaunch)
    }

    /// Everything needed is granted and usable in this process.
    fn done(&self) -> bool {
        self.needed().all(|p| self.usable(p))
    }

    /// A step the tour passes: the permissions screen once everything on it works.
    fn passes(&self, step: Step) -> bool {
        step == Step::Permissions && self.done()
    }

    fn forward_from(&self, mut step: Step) -> Step {
        while self.passes(step) {
            step = step.next();
        }
        step
    }

    fn back_from(&self, step: Step) -> Step {
        if self.mode == Mode::Fix {
            return step;
        }
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
        self.ready_at = None;
        if self.mode == Mode::Fix {
            return Effect::Close;
        }
        self.step = self.forward_from(self.step.next());
        Effect::None
    }

    fn primary(&mut self) -> Effect {
        match self.step {
            Step::TryIt | Step::Done => Effect::Close,
            Step::Permissions if !self.ready() => Effect::None,
            Step::Permissions if self.screen_relaunch() => self.relaunch(),
            _ => self.advance(),
        }
    }

    fn grant(&mut self, permission: Permission) -> Effect {
        if self.step != Step::Permissions
            || !self.asks.contains(&permission)
            || self.granted(permission)
        {
            return Effect::None;
        }
        if self.requested.insert(permission) && self.status(permission) != Status::Denied {
            Effect::Request(permission)
        } else {
            Effect::OpenSettings(permission)
        }
    }

    /// The one relaunch: only from the permissions screen, once everything needed is granted
    /// and Screen Recording still waits for it.
    fn relaunch(&self) -> Effect {
        if self.step != Step::Permissions || !self.ready() || !self.screen_relaunch() {
            return Effect::None;
        }
        let resume = match self.mode {
            Mode::Tour => Some(Step::Permissions.next()),
            Mode::Fix => None,
        };
        Effect::Relaunch { resume }
    }

    /// A fix closes by itself once everything it asked for works; the tour waits for Continue.
    fn tick(&mut self, now_ms: u64) -> Effect {
        if self.mode != Mode::Fix || !self.done() {
            self.ready_at = None;
            return Effect::None;
        }
        let since = *self.ready_at.get_or_insert(now_ms);
        if now_ms.saturating_sub(since) >= AUTO_ADVANCE_MS {
            self.advance()
        } else {
            Effect::None
        }
    }
}

/// Required permissions that keep ⌥⇧S from recording now. Screen Recording is read in this
/// process (a grant it can't use yet still blocks); the others come from the last fresh read.
pub fn missing_to_record(grants: Option<&Grants>, screen_usable: bool) -> Vec<Permission> {
    Permission::REQUIRED
        .into_iter()
        .filter(|p| match p {
            Permission::ScreenRecording => !screen_usable,
            p => grants.is_some_and(|g| !g.granted(*p)),
        })
        .collect()
}

/// Holds a request to show the permissions window until the shortcut's modifiers are up, so
/// nothing appears while ⌥⇧ is still down and every button is clickable when it does.
#[derive(Debug, Default)]
pub struct Gate {
    pending: Option<Vec<Permission>>,
}

impl Gate {
    /// ⌥⇧S went down with nothing recording. Returns whether it may go on; with `missing`
    /// permissions it starts nothing and the window waits for the keys to come up.
    pub fn press(&mut self, missing: Vec<Permission>) -> bool {
        if self.pending.is_some() {
            return false;
        }
        if missing.is_empty() {
            return true;
        }
        self.pending = Some(missing);
        false
    }

    /// Show the window for `asks` once the modifiers are up.
    pub fn defer(&mut self, asks: Vec<Permission>) {
        self.pending = Some(asks);
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// Polled while pending: the permissions to show, once no modifier is held.
    pub fn tick(&mut self, modifiers_down: bool) -> Option<Vec<Permission>> {
        if modifiers_down {
            None
        } else {
            self.pending.take()
        }
    }
}

/// Onboarding progress, under `"onboarding"` in `~/.config/scrawl/config.json`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Progress {
    /// The tour was finished or closed; it doesn't open by itself again.
    #[serde(default)]
    pub seen: bool,
    /// Where the tour picks up at the next launch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume: Option<Step>,
    /// Every permission seen granted, so a later loss reopens the permissions screen.
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

    /// Writes `"onboarding"`, keeping the rest of the config (and its `0600` mode) as it is.
    pub fn save_to(&self, path: &Path) -> anyhow::Result<()> {
        crate::handoff::set_config_value_in(path, "onboarding", serde_json::to_value(self)?)
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

/// What opens at launch: the tour until it's been seen (resuming where it was left), else the
/// permissions screen when a required permission that was granted once is gone, else nothing.
pub fn launch_plan(progress: &Progress, grants: &Grants) -> Option<Flow> {
    if !progress.seen {
        let start = progress.resume.unwrap_or(Step::Welcome);
        return Some(Flow::tour(start, Some(grants.clone())));
    }
    Permission::REQUIRED
        .into_iter()
        .any(|p| progress.granted.contains(&p) && !grants.granted(p))
        .then(|| Flow::fix(Permission::REQUIRED.to_vec(), Some(grants.clone())))
}

/// From the welcome window's helper process and the permission poller.
#[derive(Clone, Debug)]
pub enum Event {
    /// Grants read at launch, to decide whether onboarding opens (`launch_plan`).
    Launch(Grants),
    Grants(Grants),
    /// A button: `continue`, `back`, `later`, `relaunch`, `close` or `grant:<permission>`.
    Action(String),
    Exited,
}

/// Reads what's granted now from a fresh helper, plus whether this process can use a Screen
/// Recording grant yet.
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

/// What draws the welcome window: the SwiftUI `scrawl-helper onboarding` process on macOS, an
/// in-process GPUI window (`crate::onboarding_window`) on Windows. Both take the same JSON
/// messages and report the same button actions.
enum Transport {
    Helper {
        child: std::process::Child,
        stdin: Option<std::process::ChildStdin>,
    },
    InProcess(Option<flume::Sender<serde_json::Value>>),
}

/// The open welcome window: the `Flow`, the window drawing it, and a poller re-reading the
/// grants every 0.5 s so each row's check appears by itself.
pub struct Session {
    flow: Flow,
    transport: Transport,
    polling: std::sync::Arc<std::sync::atomic::AtomicBool>,
    last_view: String,
    saved_resume: Option<Step>,
    clock: std::time::Instant,
}

impl Session {
    pub fn open(
        flow: Flow,
        shortcut: &str,
        events: impl Fn(Event) + Send + Sync + Clone + 'static,
    ) -> anyhow::Result<Self> {
        use std::sync::atomic::{AtomicBool, Ordering};

        let transport = if cfg!(target_os = "windows") {
            let (sender, receiver) = flume::unbounded();
            let window_events = events.clone();
            crate::onboarding_window::request_open(
                receiver,
                shortcut.to_owned(),
                std::sync::Arc::new(move |event| window_events(event)),
            );
            Transport::InProcess(Some(sender))
        } else {
            Self::spawn_helper(shortcut, events.clone())?
        };

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
            transport,
            polling,
            last_view: String::new(),
            saved_resume: None,
            clock: std::time::Instant::now(),
        };
        session.sync();
        Ok(session)
    }

    fn spawn_helper(
        shortcut: &str,
        events: impl Fn(Event) + Send + Sync + Clone + 'static,
    ) -> anyhow::Result<Transport> {
        use std::io::BufRead;
        use std::process::{Command, Stdio};

        let mut command = Command::new(crate::helper::path()?);
        command
            .args(["onboarding", "--shortcut", shortcut, "--paste"])
            .arg(crate::settings::paste_label())
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

        std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                for line in std::io::BufReader::new(stdout)
                    .lines()
                    .map_while(Result::ok)
                {
                    let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                        continue;
                    };
                    let Some(action) = value.get("action").and_then(|v| v.as_str()) else {
                        continue;
                    };
                    let action = match value.get("permission").and_then(|v| v.as_str()) {
                        Some(permission) => format!("{action}:{permission}"),
                        None => action.to_owned(),
                    };
                    events(Event::Action(action));
                }
            }
            events(Event::Exited);
        });
        Ok(Transport::Helper { child, stdin })
    }

    pub fn mode(&self) -> Mode {
        self.flow.mode()
    }

    /// Feeds the flow and carries out what the window can do itself. Returns what needs the
    /// app: `Close` or `Relaunch`.
    pub fn input(&mut self, input: Input) -> Effect {
        let effect = self.flow.on(input);
        crate::capture::scrawl_log(format!("onboarding: {:?} -> {effect:?}", self.flow.step()));
        let result = match effect {
            // These two are attributed to whichever process asks, so Scrawl itself asks.
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
                self.show();
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
            "settings" => Input::Settings,
            other => match other
                .strip_prefix("grant:")
                .and_then(|p| serde_json::from_value(serde_json::Value::String(p.into())).ok())
            {
                Some(permission) => Input::Grant(permission),
                None => return Effect::None,
            },
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
        // Remember the step, so whatever quits Scrawl, the tour comes back here.
        let resume = self.flow.resume_point();
        if resume.is_some() && resume != self.saved_resume {
            let mut progress = Progress::load();
            progress.resume = resume;
            progress.save();
            self.saved_resume = resume;
        }
    }

    fn send(&mut self, message: &serde_json::Value) {
        use std::io::Write;
        match &mut self.transport {
            Transport::Helper { stdin, .. } => {
                if let Some(pipe) = stdin.as_mut()
                    && writeln!(pipe, "{message}")
                        .and_then(|()| pipe.flush())
                        .is_err()
                {
                    *stdin = None;
                }
            }
            Transport::InProcess(sender) => {
                if sender
                    .as_ref()
                    .is_some_and(|s| s.send(message.clone()).is_err())
                {
                    *sender = None;
                }
            }
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.polling
            .store(false, std::sync::atomic::Ordering::Release);
        self.send(&serde_json::json!({"type": "close"}));
        match &mut self.transport {
            Transport::Helper { child, stdin } => {
                *stdin = None;
                let _ = child.wait();
            }
            Transport::InProcess(sender) => *sender = None,
        }
    }
}

/// Starts Scrawl again a moment after this copy quits.
#[cfg(windows)]
pub fn relaunch() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut args: Vec<std::ffi::OsString> = vec![
        "/C".into(),
        "timeout /t 1 /nobreak >nul & start \"\"".into(),
    ];
    args.push(exe.into_os_string());
    args.extend(std::env::args_os().skip(1));
    let _ = std::process::Command::new("cmd.exe").args(args).spawn();
}

/// Starts Scrawl again a moment after this copy quits (after a Screen Recording grant).
#[cfg(not(windows))]
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

    use Permission::{Accessibility, Camera, InputMonitoring, Microphone, ScreenRecording};

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

    fn rows(flow: &Flow) -> Vec<Permission> {
        flow.view().rows.iter().map(|r| r.permission).collect()
    }

    #[test]
    fn the_tour_is_four_screens_with_every_permission_on_one() {
        let mut flow = Flow::tour(Step::Welcome, Some(grants(&[])));
        assert_eq!(flow.view().count, 4);
        assert_eq!(flow.on(Input::Continue), Effect::None);
        assert_eq!(flow.step(), Step::HowItWorks);
        assert_eq!(flow.on(Input::Continue), Effect::None);
        assert_eq!(flow.step(), Step::Permissions);
        assert_eq!(rows(&flow), Permission::REQUIRED);
        assert!(flow.view().rows.iter().all(|r| r.required));
        // Camera and Accessibility are asked for when used, never in the tour.
        assert!(!rows(&flow).contains(&Camera));
        assert!(!rows(&flow).contains(&Accessibility));
        // Continue does nothing until the required ones are on.
        assert!(!flow.view().ready);
        assert_eq!(flow.on(Input::Continue), Effect::None);
        assert_eq!(flow.step(), Step::Permissions);
        assert_eq!(flow.on(Input::Back), Effect::None);
        assert_eq!(flow.step(), Step::HowItWorks);
    }

    #[test]
    fn permissions_are_granted_in_any_order_then_continue_moves_on() {
        let mut flow = Flow::tour(Step::Permissions, Some(grants(&[])));
        assert_eq!(
            flow.on(Input::Grant(InputMonitoring)),
            Effect::Request(InputMonitoring)
        );
        assert_eq!(
            flow.on(Input::Grant(Microphone)),
            Effect::Request(Microphone)
        );
        // A second press opens System Settings.
        assert_eq!(
            flow.on(Input::Grant(Microphone)),
            Effect::OpenSettings(Microphone)
        );
        let some = grants(&[Microphone, InputMonitoring]);
        tick(&mut flow, &some, 0);
        let view = flow.view();
        assert_eq!(view.rows[1].status, Status::Granted);
        assert!(!view.rows[0].requested && !view.ready);
        // No auto-advance in the tour, even long after.
        let all = grants(&[ScreenRecording, Microphone, InputMonitoring]);
        tick(&mut flow, &all, 1);
        tick(&mut flow, &all, 10_000);
        assert_eq!(flow.step(), Step::Permissions);
        assert!(flow.view().ready && !flow.view().relaunch);
        // A granted row's button does nothing.
        assert_eq!(flow.on(Input::Grant(Microphone)), Effect::None);
        assert_eq!(flow.on(Input::Continue), Effect::None);
        assert_eq!(flow.step(), Step::TryIt);
        // Back from Try it passes the finished permissions screen.
        flow.on(Input::Back);
        assert_eq!(flow.step(), Step::HowItWorks);
    }

    #[test]
    fn a_denied_permission_goes_straight_to_settings() {
        let mut denied = grants(&[]);
        denied.statuses.insert(Microphone, Status::Denied);
        let mut flow = Flow::tour(Step::Permissions, Some(denied));
        assert_eq!(
            flow.on(Input::Grant(Microphone)),
            Effect::OpenSettings(Microphone)
        );
    }

    #[test]
    fn granted_permissions_pass_the_screen_and_leave_the_dots() {
        let all = grants(&[ScreenRecording, Microphone, InputMonitoring]);
        let mut flow = Flow::tour(Step::HowItWorks, Some(all.clone()));
        assert_eq!(flow.view().count, 3);
        flow.on(Input::Continue);
        assert_eq!(flow.step(), Step::TryIt);
        assert_eq!(flow.view().index, 2);
        // A tour resumed past the screen goes back to it while something is missing.
        let some = grants(&[Microphone]);
        assert_eq!(
            Flow::tour(Step::TryIt, Some(some)).step(),
            Step::Permissions
        );
        assert_eq!(Flow::tour(Step::Permissions, Some(all)).step(), Step::TryIt);
    }

    #[test]
    fn screen_recording_relaunches_once_at_the_end_and_resumes_after_the_screen() {
        let mut stale = grants(&[ScreenRecording]);
        stale.screen_needs_relaunch = true;
        let mut flow = Flow::tour(Step::Permissions, Some(stale.clone()));
        assert_eq!(flow.step(), Step::Permissions);
        let view = flow.view();
        assert!(view.rows[0].relaunch);
        // Nothing relaunches while another required permission is still off.
        assert!(!view.relaunch);
        assert_eq!(flow.on(Input::Relaunch), Effect::None);
        assert_eq!(flow.on(Input::Continue), Effect::None);
        // All granted: the one relaunch, resuming at Try it.
        let mut all = grants(&[ScreenRecording, Microphone, InputMonitoring]);
        all.screen_needs_relaunch = true;
        tick(&mut flow, &all, 0);
        assert!(flow.view().relaunch);
        let relaunch = Effect::Relaunch {
            resume: Some(Step::TryIt),
        };
        assert_eq!(flow.on(Input::Continue), relaunch);
        assert_eq!(flow.on(Input::Relaunch), relaunch);
        // After it, the tour starts at Try it with nothing more to grant.
        let progress = Progress {
            seen: false,
            resume: Some(Step::TryIt),
            granted: BTreeSet::new(),
        };
        let fresh = grants(&[ScreenRecording, Microphone, InputMonitoring]);
        let flow = launch_plan(&progress, &fresh).expect("the tour");
        assert_eq!((flow.mode(), flow.step()), (Mode::Tour, Step::TryIt));
        // Without a stale grant there's nothing to relaunch for.
        let mut flow = Flow::tour(Step::Permissions, Some(grants(&[])));
        tick(&mut flow, &fresh, 0);
        assert_eq!(flow.on(Input::Relaunch), Effect::None);
    }

    #[test]
    fn a_tour_quit_midway_resumes_where_it_was() {
        let mut flow = Flow::tour(Step::Welcome, Some(grants(&[])));
        assert_eq!(flow.resume_point(), Some(Step::Welcome));
        flow.on(Input::Continue);
        flow.on(Input::Continue);
        assert_eq!(flow.resume_point(), Some(Step::Permissions));
        // macOS's "Quit & Reopen": the next launch opens on the permissions screen.
        let progress = Progress {
            seen: false,
            resume: flow.resume_point(),
            granted: BTreeSet::new(),
        };
        let flow = launch_plan(&progress, &grants(&[ScreenRecording])).expect("the tour");
        assert_eq!(flow.step(), Step::Permissions);
        assert_eq!(
            Flow::fix(vec![Camera], None).resume_point(),
            None,
            "a fix has nothing to resume"
        );
    }

    #[test]
    fn set_up_later_and_closing_end_the_tour() {
        let mut flow = Flow::tour(Step::Permissions, Some(grants(&[])));
        assert_eq!(flow.on(Input::Later), Effect::Close);
        assert_eq!(flow.on(Input::Closed), Effect::Close);
    }

    #[test]
    fn the_shortcut_with_a_permission_off_starts_nothing_and_opens_the_window_on_release() {
        // Mic and Input Monitoring granted, Screen Recording not usable in this process.
        let some = grants(&[Microphone, InputMonitoring]);
        let missing = missing_to_record(Some(&some), false);
        assert_eq!(missing, [ScreenRecording]);
        let mut gate = Gate::default();
        assert!(!gate.press(missing.clone()), "nothing starts");
        // Pressing again while it waits starts nothing either.
        assert!(!gate.press(Vec::new()));
        // Nothing shows while ⌥⇧ is held.
        assert_eq!(gate.tick(true), None);
        let asks = gate.tick(false).expect("the window, once the keys are up");
        assert_eq!(gate.tick(false), None);
        assert!(!gate.is_pending());
        // The window lists every required permission so the user sees what's left.
        let mut flow = Flow::fix(Permission::REQUIRED.to_vec(), Some(some.clone()));
        assert_eq!(asks, missing);
        assert_eq!(flow.step(), Step::Permissions);
        assert_eq!(
            flow.on(Input::Grant(ScreenRecording)),
            Effect::Request(ScreenRecording)
        );
        // With everything on, the shortcut goes through.
        let all = grants(&[ScreenRecording, Microphone, InputMonitoring]);
        assert!(missing_to_record(Some(&all), true).is_empty());
        assert!(gate.press(Vec::new()));
        // A tour that's still explaining jumps to the permissions screen.
        let mut tour = Flow::tour(Step::Welcome, Some(some));
        assert_eq!(tour.on(Input::Blocked), Effect::Show);
        assert_eq!(tour.step(), Step::Permissions);
        // Unknown grants (the launch read failed): only Screen Recording, read here, blocks.
        assert!(missing_to_record(None, true).is_empty());
    }

    #[test]
    fn camera_and_accessibility_are_asked_for_on_their_own_when_first_used() {
        let mut flow = Flow::fix(vec![Camera], Some(grants(&[ScreenRecording])));
        assert_eq!(rows(&flow), [Camera]);
        let view = flow.view();
        assert!(view.fix && !view.ready && !view.rows[0].required);
        assert_eq!(flow.on(Input::Grant(Camera)), Effect::Request(Camera));
        // A permission that isn't on this screen can't be asked for from it.
        assert_eq!(flow.on(Input::Grant(Microphone)), Effect::None);
        let camera = grants(&[Camera]);
        assert_eq!(tick(&mut flow, &camera, 100), Effect::None);
        assert!(flow.view().ready);
        assert_eq!(
            tick(&mut flow, &camera, 100 + AUTO_ADVANCE_MS),
            Effect::Close
        );
        let mut flow = Flow::fix(vec![Accessibility], Some(grants(&[])));
        assert_eq!(flow.on(Input::Later), Effect::Close);
        // A fix never steps back into the tour.
        assert_eq!(flow.on(Input::Back), Effect::None);
        assert_eq!(flow.step(), Step::Permissions);
    }

    #[test]
    fn a_capture_hides_the_window_and_finishing_from_try_it_shows_done() {
        let mut flow = Flow::tour(Step::TryIt, None);
        assert_eq!(flow.on(Input::CaptureStarted), Effect::Hide);
        assert_eq!(flow.on(Input::CaptureStarted), Effect::None);
        assert_eq!(
            flow.on(Input::CaptureEnded { completed: true }),
            Effect::Show
        );
        assert_eq!(flow.step(), Step::Done);
        assert_eq!(flow.on(Input::Settings), Effect::Settings);
        assert_eq!(flow.on(Input::Continue), Effect::Close);
        // A cancelled capture comes back to the same step.
        let mut flow = Flow::tour(Step::TryIt, None);
        flow.on(Input::CaptureStarted);
        assert_eq!(
            flow.on(Input::CaptureEnded { completed: false }),
            Effect::Show
        );
        assert_eq!(flow.step(), Step::TryIt);
    }

    #[test]
    fn launch_shows_the_tour_once_then_only_a_lost_permission() {
        let all = grants(&[ScreenRecording, Microphone, InputMonitoring]);
        let mut progress = Progress::default();
        assert_eq!(
            launch_plan(&progress, &all).map(|f| (f.mode(), f.step())),
            Some((Mode::Tour, Step::Welcome))
        );
        progress.seen = true;
        assert!(progress.note(&all));
        assert!(!progress.note(&all));
        assert!(launch_plan(&progress, &all).is_none());
        // Input Monitoring went away: the permissions screen, as a fix.
        let lost = grants(&[ScreenRecording, Microphone]);
        let flow = launch_plan(&progress, &lost).expect("a fix");
        assert_eq!((flow.mode(), flow.step()), (Mode::Fix, Step::Permissions));
        assert_eq!(rows(&flow), Permission::REQUIRED);
        // Never granted (put off in the tour): no nagging at launch.
        progress.granted.remove(&InputMonitoring);
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
            resume: Some(Step::Permissions),
            granted: [Microphone].into_iter().collect(),
        };
        progress.save_to(&path).unwrap();
        assert_eq!(Progress::load_from(&path), progress);
        let config: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(config["agentHost"], "studio");
        assert_eq!(config["autocrop"], false);
        assert_eq!(config["onboarding"]["resume"], "permissions");
        // A resume point saved by 0.3.2's one-permission-per-step tour still loads.
        std::fs::write(
            &path,
            r#"{"onboarding":{"seen":false,"resume":"microphone","granted":[]}}"#,
        )
        .unwrap();
        assert_eq!(Progress::load_from(&path).resume, Some(Step::Permissions));
        std::fs::write(&path, r#"{"onboarding":{"resume":"draw"}}"#).unwrap();
        assert_eq!(Progress::load_from(&path).resume, Some(Step::HowItWorks));
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
        assert!(g.granted(ScreenRecording));
        assert_eq!(g.status(Microphone), Status::Denied);
        assert_eq!(g.status(Accessibility), Status::NotDetermined);
    }
}
