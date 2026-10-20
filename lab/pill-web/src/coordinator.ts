// A pretend `ScrawlCoordinator` (src/ui.rs `pill_snapshot`, src/ui/lab.rs): island phases and
// the actions that move between them, on the animation clock so slow motion slows timers too.

import type { Mode, Pres } from "./wasm"

export type Phase = "hidden" | "recording" | "selecting" | "processing" | "completed"

/** `ui::REVEAL_*`: the tap's switch chips fold after this long untouched. */
export const REVEAL_AUTO_FOLD_S = 3.5
/** `lab::PROCESSING_S`. */
export const PROCESSING_S = 0.8

export interface CoordState {
  phase: Phase
  chips: boolean
  mode: Mode
  hold: boolean
  hasIphone: boolean
  strokes: number
  completedLabel: number
  recordingSince: number
}

export type Action =
  | "tap" | "fold" | "chip:Area" | "chip:Window" | "chip:Screen" | "chip:iPhone"
  | "select:Area" | "select:Window" | "select:Screen" | "select:iPhone" | "record"
  | "holdOpen" | "holdStep" | "holdRelease" | "finish" | "processed" | "cancel" | "dismiss"
  | "mark" | "undo" | "goto:processing" | "goto:completed"

const MODE_OF: Record<string, Mode> = { Area: 0, Window: 1, Screen: 2, iPhone: 3 }

export interface Timer { at: number; action: Action }

export interface Snapshot { s: CoordState; timers: Timer[]; now: number }

export class Coordinator {
  s: CoordState = {
    phase: "hidden", chips: false, mode: 2, hold: false, hasIphone: false, strokes: 0,
    completedLabel: 0, recordingSince: 0,
  }
  autoFold = true
  timers: Timer[] = []
  now = 0

  constructor(
    private onAction: (a: Action, auto: boolean) => void,
    private beforeAction: (a: Action, auto: boolean) => void = () => {},
  ) {}

  pres(): Pres {
    const s = this.s
    switch (s.phase) {
      case "hidden": return { kind: "hidden" }
      case "recording": return { kind: "recording", chips: s.chips, hasIphone: s.hasIphone, strokes: s.strokes }
      case "selecting": return { kind: "selecting", mode: s.mode, hold: s.hold, hasIphone: s.hasIphone }
      case "processing": return { kind: "processing" }
      case "completed": return { kind: "completed", label: s.completedLabel }
    }
  }

  clockSecs(): number {
    return this.s.phase === "recording" ? Math.floor(Math.max(0, this.now - this.s.recordingSince)) : 0
  }

  snapshot(): Snapshot {
    return { s: { ...this.s }, timers: this.timers.map((t) => ({ ...t })), now: this.now }
  }

  /** Restores a snapshot onto the clock at `now` (timers keep their distance). */
  restore(snap: Snapshot, now: number) {
    const shift = now - snap.now
    this.s = { ...snap.s, recordingSince: snap.s.recordingSince + shift }
    this.timers = snap.timers.map((t) => ({ ...t, at: t.at + shift }))
    this.now = now
  }

  /** Runs timers due by animation time `now`. */
  tick(now: number) {
    this.now = now
    const due = this.timers.filter((t) => t.at <= now + 1e-9)
    this.timers = this.timers.filter((t) => t.at > now + 1e-9)
    for (const t of due) this.act(t.action, true)
  }

  private after(s: number, action: Action) {
    this.timers = this.timers.filter((t) => t.action !== action)
    this.timers.push({ at: this.now + s, action })
  }
  private cancelTimer(action: Action) {
    this.timers = this.timers.filter((t) => t.action !== action)
  }

  private startRecording(chips: boolean) {
    const s = this.s
    if (s.phase !== "recording") {
      s.recordingSince = this.now
      s.strokes = 0
    }
    s.phase = "recording"
    s.chips = chips
    s.hold = false
    if (chips && this.autoFold) this.after(REVEAL_AUTO_FOLD_S, "fold")
    else this.cancelTimer("fold")
  }

  act(a: Action, auto = false): void {
    this.beforeAction(a, auto)
    const s = this.s
    switch (a) {
      case "tap":
        if (s.phase === "recording") return this.act("finish")
        this.startRecording(true)
        break
      case "fold":
        if (s.phase !== "recording" || !s.chips) return
        s.chips = false
        break
      case "chip:Area":
      case "chip:Window":
      case "chip:Screen":
      case "chip:iPhone": {
        const name = a.slice(5)
        if (s.phase === "selecting") return this.act(`select:${name}` as Action)
        if (s.phase !== "recording") return
        // `switch_recording`: Screen folds the chips, iPhone restarts on the phone, Area and
        // Window discard the recording and morph into the picker.
        this.cancelTimer("fold")
        const mode = MODE_OF[name]
        if (mode === 2) s.chips = false
        else if (mode === 3) {
          s.hasIphone = true
          s.phase = "hidden"
          this.startRecording(false)
        } else {
          s.phase = "selecting"
          s.mode = mode
          s.hold = false
        }
        break
      }
      case "select:Area":
      case "select:Window":
      case "select:Screen":
      case "select:iPhone":
        if (s.phase !== "selecting") {
          s.phase = "selecting"
          s.hold = false
        }
        s.mode = MODE_OF[a.slice(7)]
        if (s.mode === 3) s.hasIphone = true
        break
      case "record":
        this.startRecording(false)
        break
      case "holdOpen":
        this.cancelTimer("fold")
        s.phase = "selecting"
        s.mode = 2
        s.hold = true
        break
      case "holdStep": {
        if (s.phase !== "selecting" || !s.hold) return this.act("holdOpen")
        // Screen → Window → iPhone → Screen, one per press of S.
        const next: Record<number, Mode> = { 2: 1, 1: 3, 3: 2, 0: 2 }
        s.mode = next[s.mode]
        break
      }
      case "holdRelease":
        if (s.phase !== "selecting" || !s.hold) return
        if (s.mode === 3) s.hasIphone = true
        if (s.mode === 0 || s.mode === 1) s.hold = false
        else this.startRecording(false)
        break
      case "finish":
        if (s.phase !== "recording") return
        this.cancelTimer("fold")
        s.phase = "processing"
        this.after(PROCESSING_S, "processed")
        break
      case "processed":
        if (s.phase !== "processing") return
        s.phase = "completed"
        break
      case "goto:processing":
        this.timers = []
        s.phase = "processing"
        break
      case "goto:completed":
        this.timers = []
        s.phase = "completed"
        break
      case "mark":
        if (s.phase !== "recording") return
        s.strokes += 1
        break
      case "undo":
        if (s.phase !== "recording") return
        s.strokes = Math.max(0, s.strokes - 1)
        break
      case "cancel":
      case "dismiss":
        this.timers = []
        s.phase = "hidden"
        s.chips = false
        s.hold = false
        break
    }
    this.onAction(a, auto)
  }
}

/** `lab::Scene::steps` (src/ui/lab.rs), in animation seconds from the scene's start. */
export const SCENES: Record<string, [number, Action][]> = {
  "tap-then-area": [[0, "tap"], [1.2, "chip:Area"], [2.4, "cancel"]],
  "tap-then-window": [[0, "tap"], [1.2, "chip:Window"], [2.4, "cancel"]],
  "tap-then-screen": [[0, "tap"], [1.2, "chip:Screen"], [2.4, "cancel"]],
  "hold-switcher": [[0, "holdOpen"], [0.6, "holdStep"], [1.2, "holdStep"], [1.8, "holdStep"], [2.4, "holdRelease"], [3.6, "cancel"]],
  finish: [[0, "tap"], [0.9, "chip:Screen"], [2.0, "finish"], [2.0 + PROCESSING_S + 1.4, "dismiss"]],
  cancel: [[0, "tap"], [1.2, "cancel"]],
  dismiss: [[0, "holdOpen"], [1.2, "dismiss"]],
}
