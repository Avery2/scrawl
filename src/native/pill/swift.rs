//! The recording capsule drawn by SwiftUI in `scrawl-helper pill` (`helper/Pill.swift`), behind
//! `"pillRenderer": "swiftui"`. Like Settings and onboarding, Rust decides and the helper draws:
//! each change of what the capsule shows (the resting layout from `model::layout_of`, the clock,
//! the springs from `Tune`) goes over stdin as one JSON line, and SwiftUI animates to it in its
//! own process, so no work on this process's main thread can hold a frame back. The helper
//! reports clicks, hover and the end of an exit; clicks become the same `PillAction`s as the
//! Metal capsule's (`click_action`).

use std::cell::{Cell, RefCell};
use std::io::{BufRead, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::model::{ElemId, Metrics, PillOpts, Pres, SpringCfg, layout_of};
use super::{PillAction, PillSnapshot, PillStyle, click_action};
use crate::capture::scrawl_log;
use crate::ui::motion;

/// How long `open` waits for the helper's first line before giving up on it.
const READY_TIMEOUT: Duration = Duration::from_millis(1500);
/// A hidden capsule whose exit the helper never confirmed counts as hidden after this.
const HIDDEN_FALLBACK: Duration = Duration::from_millis(1200);

/// What the helper reports.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Ready { window: u32 },
    Hover(bool),
    Click(ElemId),
    /// Paste dragged off its chip and dropped at this point (global top-left points).
    Paste([f32; 2]),
    Hidden(u64),
    Exited,
}

pub fn parse_event(line: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(line).ok()?;
    Some(match v.get("event")?.as_str()? {
        "ready" => Event::Ready {
            window: v.get("window").and_then(Value::as_u64).unwrap_or(0) as u32,
        },
        "hover" => Event::Hover(v.get("on").and_then(Value::as_bool).unwrap_or(false)),
        "click" => Event::Click(ElemId::from_trace_key(v.get("id")?.as_str()?)?),
        "paste" => {
            let at = v.get("at")?.as_array()?;
            Event::Paste([at.first()?.as_f64()? as f32, at.get(1)?.as_f64()? as f32])
        }
        "hidden" => Event::Hidden(v.get("seq").and_then(Value::as_u64).unwrap_or(0)),
        _ => return None,
    })
}

/// The `PillAction` an event asks for while `pres` shows.
pub fn action_for(event: &Event, pres: &Pres) -> Option<PillAction> {
    match event {
        Event::Click(elem) => click_action(pres, *elem),
        Event::Paste(at) => Some(PillAction::Paste(Some(*at))),
        _ => None,
    }
}

fn spring(cfg: SpringCfg) -> Value {
    json!([round3(cfg.duration), round3(cfg.bounce)])
}

fn round3(v: f32) -> f64 {
    (f64::from(v) * 1000.0).round() / 1000.0
}

fn phase(pres: &Pres) -> &'static str {
    match pres {
        Pres::Hidden => "hidden",
        Pres::Selecting { .. } => "selecting",
        Pres::Recording { .. } => "recording",
        Pres::Processing => "processing",
        Pres::Completed { .. } => "completed",
    }
}

/// The hold switcher steps modes with a key: the user's own hand, so the highlight doesn't slide.
pub fn keyed(prev: &Pres, next: &Pres) -> bool {
    matches!(
        (prev, next),
        (Pres::Selecting { hold: true, .. }, Pres::Selecting { hold: true, .. })
    )
}

/// One state line for the helper, without its `seq`: the resting capsule and items (panel
/// points, as `model::layout_of` lays them out), the text, and the springs to move with.
pub fn state_message(
    snapshot: &PillSnapshot,
    opts: &PillOpts,
    metrics: &Metrics,
    completed_w: f32,
    keyed: bool,
    time_scale: f32,
) -> Value {
    let (lay, items) = layout_of(
        &snapshot.pres,
        &snapshot.clock,
        completed_w,
        metrics,
        opts.hold_chips,
    );
    let items: Vec<Value> = items
        .iter()
        .map(|item| {
            json!({
                "id": item.id.trace_key(),
                "rect": item.rect.map(round3),
                "active": item.active > 0.5,
                "enabled": item.enabled,
            })
        })
        .collect();
    let label = match &snapshot.pres {
        Pres::Completed { label } => label.clone(),
        _ => String::new(),
    };
    json!({
        "type": "state",
        "phase": phase(&snapshot.pres),
        "shell": [round3(lay.l), round3(lay.t), round3(lay.width()), round3(lay.height())],
        "items": items,
        "clock": snapshot.clock,
        "label": label,
        "style": snapshot.style.as_str(),
        "reduced": snapshot.reduced_motion,
        "keyed": keyed,
        "timeScale": round3(time_scale),
        "opts": {
            "origin": match opts.origin {
                super::model::Origin::Centre => 0,
                super::model::Origin::Top => 1,
                super::model::Origin::Lead => 2,
            },
            "blur": round3(opts.content_blur),
            "shellBlur": round3(opts.shell_blur),
            "enter": spring(opts.enter),
            "exit": spring(opts.exit),
            "morphOpen": spring(opts.morph_open),
            "morphClose": spring(opts.morph_close),
            "hlLead": spring(opts.highlight_lead),
            "hlTrail": spring(opts.highlight_trail),
            "punch": round3(opts.punch),
        },
    })
}

struct Sent {
    /// The last state line without its `seq`, to send only changes.
    body: String,
    pres: Pres,
    seq: u64,
    /// When the latest hidden state went out.
    hidden_at: Option<Instant>,
    origin: Option<[f32; 2]>,
    completed: Option<(String, f32)>,
}

pub struct SwiftPill {
    child: RefCell<Child>,
    stdin: RefCell<Option<ChildStdin>>,
    events: flume::Receiver<Event>,
    actions: RefCell<Vec<PillAction>>,
    window: u32,
    metrics: Metrics,
    sent: RefCell<Sent>,
    hover: Cell<bool>,
    hidden_ack: Cell<u64>,
    alive: Cell<bool>,
}

impl SwiftPill {
    pub fn open() -> Result<Self, String> {
        let helper = crate::helper::path().map_err(|e| e.to_string())?;
        let mut child = Command::new(helper)
            .arg("pill")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("scrawl-helper pill: {e}"))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().ok_or("scrawl-helper pill: no stdout")?;
        let (tx, events) = flume::unbounded();
        std::thread::Builder::new()
            .name("pill-helper".into())
            .spawn(move || {
                for line in std::io::BufReader::new(stdout).lines().map_while(Result::ok) {
                    match parse_event(&line) {
                        Some(event) => {
                            let _ = tx.send(event);
                        }
                        None => scrawl_log(format!("pill helper: {line}")),
                    }
                }
                let _ = tx.send(Event::Exited);
            })
            .map_err(|e| e.to_string())?;
        let window = match events.recv_timeout(READY_TIMEOUT) {
            Ok(Event::Ready { window }) => window,
            Ok(other) => {
                let _ = child.kill();
                return Err(format!("scrawl-helper pill said {other:?} before it was ready"));
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("scrawl-helper pill didn't start".into());
            }
        };
        let pill = Self {
            child: RefCell::new(child),
            stdin: RefCell::new(stdin),
            events,
            actions: RefCell::new(Vec::new()),
            window,
            metrics: super::atlas::measure_metrics(),
            sent: RefCell::new(Sent {
                body: String::new(),
                pres: Pres::Hidden,
                seq: 0,
                hidden_at: None,
                origin: None,
                completed: None,
            }),
            hover: Cell::new(false),
            hidden_ack: Cell::new(0),
            alive: Cell::new(true),
        };
        if let Some(point) = crate::screens::pointer() {
            pill.place_on(point);
        }
        Ok(pill)
    }

    pub fn window_number(&self) -> u32 {
        self.window
    }

    /// The helper is still there to draw.
    pub fn is_alive(&self) -> bool {
        self.pump();
        self.alive.get()
    }

    fn send(&self, message: &Value) {
        let mut stdin = self.stdin.borrow_mut();
        if let Some(pipe) = stdin.as_mut()
            && writeln!(pipe, "{message}").and_then(|()| pipe.flush()).is_err()
        {
            *stdin = None;
            self.alive.set(false);
        }
    }

    /// Takes in what the helper reported.
    fn pump(&self) {
        let pres = self.sent.borrow().pres.clone();
        for event in self.events.try_iter() {
            match event {
                Event::Hover(on) => self.hover.set(on),
                Event::Hidden(seq) => self.hidden_ack.set(seq),
                Event::Exited => {
                    if self.alive.replace(false) {
                        scrawl_log("pill helper exited");
                    }
                }
                Event::Ready { .. } => {}
                ref other => {
                    if let Some(action) = action_for(other, &pres) {
                        self.actions.borrow_mut().push(action);
                    }
                }
            }
        }
    }

    pub fn place_on(&self, point: [f64; 2]) {
        let screens = crate::screens::current();
        let Some(screen) = super::panel_screen(&screens, Some(point)) else {
            return;
        };
        let origin = super::panel_origin(screen);
        let mut sent = self.sent.borrow_mut();
        if sent
            .origin
            .is_some_and(|o| (o[0] - origin[0]).abs() < 0.5 && (o[1] - origin[1]).abs() < 0.5)
        {
            return;
        }
        sent.origin = Some(origin);
        drop(sent);
        self.send(&json!({"type": "place", "origin": origin}));
    }

    pub fn sync(&self, snapshot: PillSnapshot) {
        self.pump();
        let completed_w = match &snapshot.pres {
            Pres::Completed { label } => {
                let mut sent = self.sent.borrow_mut();
                match &sent.completed {
                    Some((text, w)) if text == label => *w,
                    _ => {
                        let w = super::atlas::measure_text_pt(
                            label,
                            super::model::HUD_TEXT,
                            super::atlas::WEIGHT_SEMIBOLD,
                            false,
                        );
                        sent.completed = Some((label.clone(), w));
                        w
                    }
                }
            }
            _ => 0.0,
        };
        let mut sent = self.sent.borrow_mut();
        let keyed = keyed(&sent.pres, &snapshot.pres);
        let mut message = state_message(
            &snapshot,
            &motion::tune().pill_opts(),
            &self.metrics,
            completed_w,
            keyed,
            motion::time_scale(),
        );
        let body = message.to_string();
        if body == sent.body {
            return;
        }
        sent.seq += 1;
        sent.body = body;
        if matches!(snapshot.pres, Pres::Hidden) {
            if !matches!(sent.pres, Pres::Hidden) {
                sent.hidden_at = Some(Instant::now());
            }
        } else {
            sent.hidden_at = None;
        }
        sent.pres = snapshot.pres;
        message["seq"] = sent.seq.into();
        drop(sent);
        self.send(&message);
    }

    pub fn drain_actions(&self) -> Vec<PillAction> {
        self.pump();
        std::mem::take(&mut *self.actions.borrow_mut())
    }

    /// Hidden, and the exit has played (or the helper never said, or is gone).
    pub fn is_fully_hidden(&self) -> bool {
        self.pump();
        let sent = self.sent.borrow();
        if !matches!(sent.pres, Pres::Hidden) {
            return false;
        }
        let Some(at) = sent.hidden_at else {
            return true;
        };
        !self.alive.get()
            || self.hidden_ack.get() >= sent.seq
            || at.elapsed() >= HIDDEN_FALLBACK.mul_f32(1.0 / motion::time_scale().max(0.05))
    }

    pub fn pointer_on_pill(&self) -> bool {
        self.pump();
        self.hover.get() && self.alive.get()
    }

    #[allow(dead_code)]
    pub fn style(&self) -> PillStyle {
        PillStyle::current()
    }
}

impl Drop for SwiftPill {
    fn drop(&mut self) {
        self.send(&json!({"type": "close"}));
        *self.stdin.borrow_mut() = None;
        let mut child = self.child.borrow_mut();
        for _ in 0..20 {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::SelectionMode;

    fn snapshot(pres: Pres) -> PillSnapshot {
        PillSnapshot {
            pres,
            clock: "0:07".into(),
            reduced_motion: false,
            style: PillStyle::Black,
        }
    }

    fn message(pres: Pres) -> Value {
        state_message(
            &snapshot(pres),
            &PillOpts::default(),
            &Metrics::default(),
            46.0,
            false,
            1.0,
        )
    }

    #[test]
    fn a_state_line_carries_the_rust_layout_and_springs() {
        let pres = Pres::Selecting {
            mode: SelectionMode::Window,
            hold: false,
            has_iphone: false,
        };
        let v = message(pres.clone());
        assert_eq!(v["type"], "state");
        assert_eq!(v["phase"], "selecting");
        let (lay, items) = layout_of(&pres, "0:07", 46.0, &Metrics::default(), true);
        assert_eq!(v["shell"][2].as_f64().unwrap() as f32, lay.width());
        let ids: Vec<&str> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap())
            .collect();
        assert_eq!(
            ids,
            items.iter().map(|i| i.id.trace_key()).collect::<Vec<_>>()
        );
        assert_eq!(ids, ["chip:Area", "chip:Window", "chip:Screen", "close"]);
        let active: Vec<&str> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["active"] == true)
            .map(|i| i["id"].as_str().unwrap())
            .collect();
        assert_eq!(active, ["chip:Window"]);
        assert_eq!(v["opts"]["morphOpen"], json!([0.42, 0.16]));
        assert_eq!(v["opts"]["origin"], 1);
        assert_eq!(v["opts"]["punch"], 2.0);
    }

    #[test]
    fn hidden_and_completed_lines() {
        let v = message(Pres::Hidden);
        assert_eq!(v["phase"], "hidden");
        assert_eq!(v["items"], json!([]));
        let v = message(Pres::Completed {
            label: "Copied".into(),
        });
        assert_eq!(v["phase"], "completed");
        assert_eq!(v["label"], "Copied");
        let ids: Vec<&str> = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["done.lead", "paste", "inspect", "close"]);
    }

    #[test]
    fn undo_is_disabled_until_a_stroke() {
        let v = message(Pres::Recording {
            chips: false,
            has_iphone: false,
            stroke_count: 0,
        });
        let undo = v["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == "undo")
            .unwrap()
            .clone();
        assert_eq!(undo["enabled"], false);
    }

    #[test]
    fn events_parse() {
        assert_eq!(
            parse_event(r#"{"event":"ready","window":4711}"#),
            Some(Event::Ready { window: 4711 })
        );
        assert_eq!(
            parse_event(r#"{"event":"hover","on":true}"#),
            Some(Event::Hover(true))
        );
        assert_eq!(
            parse_event(r#"{"event":"click","id":"chip:iPhone"}"#),
            Some(Event::Click(ElemId::ChipIphone))
        );
        assert_eq!(
            parse_event(r#"{"event":"paste","at":[100.5,20]}"#),
            Some(Event::Paste([100.5, 20.0]))
        );
        assert_eq!(
            parse_event(r#"{"event":"hidden","seq":9}"#),
            Some(Event::Hidden(9))
        );
        assert_eq!(parse_event(r#"{"event":"click","id":"nope"}"#), None);
        assert_eq!(parse_event("garbage"), None);
    }

    #[test]
    fn clicks_map_to_the_metal_capsules_actions() {
        let selecting = Pres::Selecting {
            mode: SelectionMode::Area,
            hold: false,
            has_iphone: true,
        };
        let recording = Pres::Recording {
            chips: true,
            has_iphone: true,
            stroke_count: 0,
        };
        let completed = Pres::Completed {
            label: "Copied".into(),
        };
        let act = |e: Event, p: &Pres| format!("{:?}", action_for(&e, p));
        assert_eq!(
            act(Event::Click(ElemId::ChipWindow), &selecting),
            "Some(SelectMode(Window))"
        );
        assert_eq!(
            act(Event::Click(ElemId::ChipWindow), &recording),
            "Some(SwitchRecording(Window))"
        );
        assert_eq!(act(Event::Click(ElemId::Close), &recording), "Some(Cancel)");
        assert_eq!(
            act(Event::Click(ElemId::Close), &completed),
            "Some(DismissCompleted)"
        );
        assert_eq!(
            act(Event::Click(ElemId::Paste), &completed),
            "Some(Paste(None))"
        );
        assert_eq!(
            act(Event::Paste([3.0, 4.0]), &completed),
            "Some(Paste(Some([3.0, 4.0])))"
        );
        assert_eq!(act(Event::Hover(true), &completed), "None");
    }

    #[test]
    fn the_hold_switcher_steps_are_keyed() {
        let hold = |mode| Pres::Selecting {
            mode,
            hold: true,
            has_iphone: false,
        };
        assert!(keyed(&hold(SelectionMode::Screen), &hold(SelectionMode::Window)));
        assert!(!keyed(&Pres::Hidden, &hold(SelectionMode::Screen)));
    }
}
