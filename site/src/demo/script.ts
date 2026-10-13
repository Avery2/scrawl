export type Mode = "mac" | "iphone"
export type Point = [number, number]
export type Rect = { x: number; y: number; w: number; h: number }

export const STAGE = { w: 1200, h: 720 }
export const INK = "#ff3b30"

export const MENUBAR_H = 28
export const MAC_WINDOW: Rect = { x: 52, y: 78, w: 684, h: 540 }
export const PHONE: Rect = { x: 265, y: 76, w: 258, h: 556 }
export const AGENT: Rect = { x: 760, y: 48, w: 422, h: 640 }
export const AREA: Rect = { x: 40, y: 66, w: 708, h: 564 }

// Kit's narration (public/narration.mp3) begins at SPEECH_START; word times are measured offsets.
export const SPEECH_START = 5800

export const T = {
  keys: 2400,
  keysUp: 3300,
  select: 3100,
  dragStart: 3900,
  dragEnd: 4900,
  phoneClick: 4300,
  phoneRecord: 5150,
  record: 5300,
  circle: [5950, 7500] as const,
  pin1: 7800,
  arrow: [9150, 9850] as const,
  head: [9950, 10300] as const,
  pin2: 10900,
  finish: 13700,
  copied: 14400,
  promptClick: 14800,
  paste: 15000,
  submit: 15500,
  read: 16100,
  edit: 16600,
  diff: 16800,
  fix: [18100, 19100] as const,
  done: 19500,
  end: 21100,
}

export const BEATS = [
  { label: "Broken", t: 0 },
  { label: "Capture", t: 2200 },
  { label: "Annotate", t: 5300 },
  { label: "Handoff", t: 13500 },
  { label: "Fix", t: 16400 },
] as const

const SPOKEN: [string, number, number][] = [
  ["Look", 160, 240],
  ["at", 280, 320],
  ["this", 400, 560],
  ["header", 720, 880],
  ["badge—it's", 960, 1840],
  ["colliding", 1960, 2320],
  ["with", 2360, 2400],
  ["the", 2480, 2560],
  ["title,", 2640, 3240],
  ["and", 3280, 3520],
  ["this", 3640, 3760],
  ["button", 3920, 4080],
  ["row", 4160, 4240],
  ["is", 4320, 4400],
  ["shifted", 4480, 4800],
  ["down", 4960, 5120],
  ["14", 5240, 5600],
  ["pixels.", 5680, 6613],
  ["Line", 6667, 6720],
  ["them", 6800, 6880],
  ["up.", 6960, 7360],
]

// [word, start, end] on the demo clock.
export const WORDS: [string, number, number][] = SPOKEN.map(([w, a, b]) => [w, SPEECH_START + a, SPEECH_START + b])
// The second clause ("and this button row…") starts at this word index.
export const CLAUSE_2 = 9
export const SPEECH_END = WORDS[WORDS.length - 1][2]
// The recording opens with a short breath; audio playback starts just before "Look".
export const AUDIO_START = SPEECH_START + 155

export const clamp = (v: number, lo = 0, hi = 1) => Math.min(hi, Math.max(lo, v))
export const lerp = (a: number, b: number, p: number) => a + (b - a) * p
export const progress = (t: number, a: number, b: number) => clamp((t - a) / (b - a))
export const easeInOut = (p: number) => (p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2)
export const easeOut = (p: number) => 1 - Math.pow(1 - p, 3)
export const easeOutBack = (p: number) => {
  const c1 = 1.4
  const c3 = c1 + 1
  return 1 + c3 * Math.pow(p - 1, 3) + c1 * Math.pow(p - 1, 2)
}

export const clock = (ms: number) => {
  const s = Math.max(0, Math.floor(ms / 1000))
  return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`
}

export type Pill = "notch" | "select" | "recording" | "processing" | "copied"

export const pillState = (t: number): Pill => {
  if (t < T.select) return "notch"
  if (t < T.record) return "select"
  if (t < T.finish + 50) return "recording"
  if (t < T.copied) return "processing"
  return "copied"
}

// Mirrors the HUD in src/ui.rs: a 40px capsule, 4px inset, 32px chips with 2px gaps. Item widths
// are fixed (a divider is 1px plus 6px margins) so the scripted cursor can hit exact chip centers.
export const PILL = {
  cx: STAGE.w / 2,
  top: MENUBAR_H + 10,
  h: 40,
  inset: 4,
  gap: 2,
  items: {
    select: [56, 94, 66, 13, 32],
    selectPhone: [56, 94, 66, 72, 13, 32],
    recording: [70, 13, 58, 68, 68],
    processing: [132],
    copied: [98, 13, 62, 74, 32],
  },
}
export const PILL_CY = PILL.top + PILL.h / 2

export const pillWidth = (items: number[]) => items.reduce((a, b) => a + b, 0) + PILL.gap * (items.length - 1) + PILL.inset * 2

const chipCenter = (items: number[], index: number): Point => {
  let x = PILL.cx - pillWidth(items) / 2 + PILL.inset
  for (let i = 0; i < index; i++) x += items[i] + PILL.gap
  return [x + items[index] / 2, PILL_CY]
}

// The home-screen prompt row inside the OpenCode terminal (see Terminal in stage-parts.tsx).
export const PROMPT_AT: Point = [AGENT.x + 150, AGENT.y + 356]

export type Anchors = { badge: Rect; actions: Rect; title: Rect }

export type Stroke = { id: string; points: Point[]; t0: number; t1: number }
export type Pin = { n: number; t: number; at: Point }
export type Click = { t: number; at: Point }
export type Waypoint = { t: number; at: Point }

export type Script = {
  strokes: Stroke[]
  // Where the spoken caption pill's right edge sits (vertically centered) for each clause.
  speech: [Point, Point]
  pins: Pin[]
  clicks: Click[]
  path: Waypoint[]
  frames: { n: number; t: number; strokes: string[]; pins: number[] }[]
  drag?: { from: Point; to: Point }
}

const circlePoints = (cx: number, cy: number, rx: number, ry: number): Point[] => {
  const pts: Point[] = []
  const start = -2.5
  const sweep = Math.PI * 2 * 1.1
  const n = 96
  for (let i = 0; i <= n; i++) {
    const k = i / n
    const a = start + sweep * k
    // A slow outward drift lets the overlap land beside the start instead of on top of it.
    const grow = 1 + k * 0.06
    pts.push([cx + Math.cos(a) * rx * grow, cy + Math.sin(a) * ry * grow - k * 4])
  }
  return pts
}

const curvePoints = (s: Point, c: Point, e: Point, n = 36): Point[] => {
  const pts: Point[] = []
  for (let i = 0; i <= n; i++) {
    const k = i / n
    const u = 1 - k
    pts.push([u * u * s[0] + 2 * u * k * c[0] + k * k * e[0], u * u * s[1] + 2 * u * k * c[1] + k * k * e[1]])
  }
  return pts
}

const line = (a: Point, b: Point, n = 10): Point[] =>
  Array.from({ length: n + 1 }, (_, i) => [lerp(a[0], b[0], i / n), lerp(a[1], b[1], i / n)] as Point)

const center = (r: Rect): Point => [r.x + r.w / 2, r.y + r.h / 2]

export function buildScript(mode: Mode, a: Anchors): Script {
  const badge = center(a.badge)
  const circle = circlePoints(badge[0] - 10, badge[1], a.badge.w / 2 + 40, a.badge.h / 2 + 15)

  // On the Mac the arrow swings up from below-left into the clipped "Retry failed" button.
  const buttons = a.actions.x + a.actions.w - 200
  const tip: Point =
    mode === "mac"
      ? [buttons - 10, a.actions.y + a.actions.h * 0.6]
      : [a.actions.x + a.actions.w * 0.62, a.actions.y + a.actions.h + 6]
  const start: Point = mode === "mac" ? [tip[0] - 104, tip[1] + 52] : [tip[0] + 34, tip[1] + 92]
  const ctrl: Point = mode === "mac" ? [tip[0] - 44, tip[1] + 46] : [tip[0] + 44, tip[1] + 34]
  const shaft = curvePoints(start, ctrl, tip, 40)
  // A quadratic's end tangent points from the control point to the tip.
  const angle = Math.atan2(tip[1] - ctrl[1], tip[0] - ctrl[0])
  const wing = (side: number): Point => [tip[0] - Math.cos(angle + side * 0.5) * 19, tip[1] - Math.sin(angle + side * 0.5) * 19]
  const wing1 = wing(1)
  const wing2 = wing(-1)
  const head = [...line(wing1, tip, 12), ...line(tip, wing2, 12).slice(1)]

  const strokes: Stroke[] = [
    { id: "circle", points: circle, t0: T.circle[0], t1: T.circle[1] },
    { id: "shaft", points: shaft, t0: T.arrow[0], t1: T.arrow[1] },
    { id: "head", points: head, t0: T.head[0], t1: T.head[1] },
  ]

  // The pill rides to the left of each mark, so it never covers the ink or the thing being
  // described, and its glide between clauses never crosses the circle.
  const circleLeft = Math.min(...circle.map((p) => p[0]))
  const speech: Script["speech"] =
    mode === "mac"
      ? [
          [circleLeft - 12, badge[1]],
          [start[0] - 20, start[1]],
        ]
      : [
          [PHONE.x - 16, badge[1]],
          [PHONE.x - 16, start[1]],
        ]

  const pin1: Point = [a.badge.x + a.badge.w + 2, a.badge.y - 2]
  const pin2: Point =
    mode === "mac" ? [a.actions.x + a.actions.w - 92, a.actions.y + 10] : [a.actions.x + a.actions.w * 0.52, a.actions.y + 8]
  const pins: Pin[] = [
    { n: 1, t: T.pin1, at: pin1 },
    { n: 2, t: T.pin2, at: pin2 },
  ]

  const finish = chipCenter(PILL.items.recording, 4)
  const prompt: Point = PROMPT_AT
  const clicks: Click[] = [...pins.map((p) => ({ t: p.t, at: p.at })), { t: T.finish, at: finish }, { t: T.promptClick, at: prompt }]

  const path: Waypoint[] = [
    { t: 0, at: [880, 560] },
    { t: 1400, at: [560, 330] },
    { t: 2400, at: [520, 300] },
  ]

  let drag: Script["drag"]
  if (mode === "mac") {
    drag = { from: [AREA.x, AREA.y], to: [AREA.x + AREA.w, AREA.y + AREA.h] }
    path.push({ t: T.dragStart, at: drag.from }, { t: T.dragEnd, at: drag.to })
  } else {
    const phoneChip = chipCenter(PILL.items.select, 2)
    const recordChip = chipCenter(PILL.items.selectPhone, 3)
    clicks.push({ t: T.phoneClick, at: phoneChip }, { t: T.phoneRecord, at: recordChip })
    path.push({ t: T.phoneClick, at: phoneChip }, { t: T.phoneRecord - 250, at: recordChip }, { t: T.phoneRecord, at: recordChip })
  }

  path.push(
    { t: T.circle[0], at: circle[0] },
    { t: T.circle[1], at: circle[circle.length - 1] },
    { t: T.pin1, at: pin1 },
    { t: T.pin1 + 500, at: [pin1[0] + 30, pin1[1] + 40] },
    { t: T.arrow[0], at: shaft[0] },
    { t: T.arrow[1], at: tip },
    { t: T.head[0], at: wing1 },
    { t: T.head[1], at: wing2 },
    { t: T.pin2, at: pin2 },
    { t: T.pin2 + 700, at: [pin2[0] + 40, pin2[1] + 50] },
    { t: T.finish - 120, at: finish },
    { t: T.finish + 200, at: finish },
    { t: T.promptClick, at: prompt },
    { t: T.read, at: prompt },
    { t: T.read + 1400, at: [AGENT.x - 18, AGENT.y + AGENT.h - 10] },
  )

  return {
    strokes,
    speech,
    pins,
    clicks,
    path,
    drag,
    frames: [
      { n: 1, t: T.pin1 - T.record, strokes: ["circle"], pins: [1] },
      { n: 2, t: T.pin2 - T.record, strokes: ["shaft", "head"], pins: [2] },
    ],
  }
}

// The tip is interpolated between samples so the stroke grows continuously every frame.
export function strokeAt(s: Stroke, t: number) {
  if (t <= s.t0) return null
  const p = easeInOut(progress(t, s.t0, s.t1))
  const last = s.points.length - 1
  const at = p * last
  const i = Math.floor(at)
  if (i >= last) return { points: s.points, tip: s.points[last], done: true }
  const a = s.points[i]
  const b = s.points[i + 1]
  const tip: Point = [lerp(a[0], b[0], at - i), lerp(a[1], b[1], at - i)]
  return { points: [...s.points.slice(0, i + 1), tip], tip, done: false }
}

export type Cursor = { at: Point; drawing: boolean; pressed: boolean; shift: boolean }

export function cursorAt(script: Script, t: number): Cursor {
  const active = script.strokes.find((s) => t > s.t0 && t < s.t1)
  const pressed = script.clicks.some((c) => t >= c.t - 60 && t < c.t + 140)
  const shift = !!active || script.pins.some((p) => t >= p.t - 250 && t < p.t + 250)
  if (active) {
    return { at: strokeAt(active, t)!.tip, drawing: true, pressed: true, shift: true }
  }
  const path = script.path
  if (t <= path[0].t) return { at: path[0].at, drawing: false, pressed, shift }
  for (let i = 1; i < path.length; i++) {
    const a = path[i - 1]
    const b = path[i]
    if (t <= b.t) {
      const k = easeInOut(progress(t, a.t, b.t))
      return {
        at: [lerp(a.at[0], b.at[0], k), lerp(a.at[1], b.at[1], k)],
        drawing: false,
        pressed: pressed || (!!script.drag && t > T.dragStart && t < T.dragEnd),
        shift,
      }
    }
  }
  return { at: path[path.length - 1].at, drawing: false, pressed, shift }
}

export const summaryText = (mode: Mode, short = false) => {
  const dir = short ? "~/…/captures" : "~/Library/Application Support/scrawl/captures"
  const target = mode === "mac" ? "Pulse “Deploy Activity”" : "iPhone · dev.pulse.app (212)"
  return [
    `[scrawl ${clock(T.finish - T.record)} · ${target} · ${dir}/scrawl-1791924018273-3a1f.scrawl]`,
    `- #1 (${clock(T.pin1 - T.record)}) “Look at this header badge—it's colliding with the title,”`,
    `- #2 (${clock(T.pin2 - T.record)}) “and this button row is shifted down 14 pixels. Line them up.”`,
  ]
}
