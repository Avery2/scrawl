export type Mode = "mac" | "iphone"
export type Point = [number, number]
export type Rect = { x: number; y: number; w: number; h: number }

export const STAGE = { w: 1200, h: 720 }
export const INK = "#ff3b30"

export const MAC_WINDOW: Rect = { x: 56, y: 64, w: 680, h: 540 }
export const PHONE: Rect = { x: 267, y: 64, w: 258, h: 560 }
export const AGENT: Rect = { x: 768, y: 64, w: 376, h: 612 }
export const AREA: Rect = { x: 44, y: 52, w: 704, h: 564 }

export const T = {
  keys: 2400,
  keysUp: 3300,
  select: 3100,
  dragStart: 3900,
  dragEnd: 4900,
  phoneClick: 4300,
  record: 5300,
  circle: [6000, 7300] as const,
  pin1: 7600,
  arrow: [8900, 9800] as const,
  head1: [9850, 10050] as const,
  head2: [10100, 10300] as const,
  pin2: 10600,
  finish: 12600,
  copied: 13300,
  promptClick: 13700,
  paste: 13900,
  submit: 14400,
  read: 15000,
  edit: 15500,
  diff: 15700,
  fix: [17000, 18000] as const,
  done: 18400,
  end: 20000,
}

export const BEATS = [
  { label: "Broken", t: 0 },
  { label: "Capture", t: 2200 },
  { label: "Annotate", t: 5300 },
  { label: "Handoff", t: 12400 },
  { label: "Fix", t: 15300 },
] as const

export const WORDS: [string, number][] = [
  ["Look", 5900],
  ["at", 6100],
  ["this", 6250],
  ["header", 6450],
  ["badge—", 6750],
  ["it's", 7200],
  ["colliding", 7400],
  ["with", 7800],
  ["the", 7950],
  ["title,", 8100],
  ["and", 8700],
  ["this", 8850],
  ["button", 9000],
  ["row", 9250],
  ["is", 9450],
  ["shifted", 9600],
  ["down", 9900],
  ["14", 10150],
  ["pixels.", 10450],
  ["Line", 11100],
  ["them", 11300],
  ["up.", 11500],
]
export const SPEECH_END = 11900

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

// Fixed chip columns keep the island's geometry predictable for the scripted cursor.
export const PILL = {
  cx: STAGE.w / 2,
  cy: 24,
  pad: 5,
  gap: 4,
  cols: {
    select: [56, 86, 64, 30],
    recording: [66, 54, 62, 62],
    copied: [78, 56, 66, 30],
  },
}

export const pillWidth = (cols: number[]) =>
  cols.reduce((a, b) => a + b, 0) + PILL.gap * (cols.length - 1) + PILL.pad * 2

const chipCenter = (cols: number[], index: number): Point => {
  let x = PILL.cx - pillWidth(cols) / 2 + PILL.pad
  for (let i = 0; i < index; i++) x += cols[i] + PILL.gap
  return [x + cols[index] / 2, PILL.cy]
}

export type Anchors = { badge: Rect; actions: Rect; title: Rect }

export type Stroke = { id: string; points: Point[]; t0: number; t1: number }
export type Pin = { n: number; t: number; at: Point }
export type Click = { t: number; at: Point }
export type Waypoint = { t: number; at: Point }

export type Script = {
  strokes: Stroke[]
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
  const n = 72
  for (let i = 0; i <= n; i++) {
    const k = i / n
    const a = start + sweep * k
    const wobble = 1 + 0.035 * Math.sin(a * 3 + 0.7) + k * 0.07
    pts.push([cx + Math.cos(a) * rx * wobble, cy + Math.sin(a) * ry * wobble - k * 5])
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

  const tip: Point =
    mode === "mac"
      ? [a.actions.x + a.actions.w + 6, a.actions.y + a.actions.h * 0.55]
      : [a.actions.x + a.actions.w * 0.62, a.actions.y + a.actions.h + 6]
  const start: Point = mode === "mac" ? [tip[0] + 74, tip[1] + 70] : [tip[0] + 34, tip[1] + 92]
  const ctrl: Point = mode === "mac" ? [start[0] + 4, tip[1] + 10] : [tip[0] + 44, tip[1] + 34]
  const shaft = curvePoints(start, ctrl, tip)
  const before = shaft[shaft.length - 5]
  const angle = Math.atan2(tip[1] - before[1], tip[0] - before[0])
  const head = (side: number): Point => [
    tip[0] - Math.cos(angle + side * 0.5) * 15,
    tip[1] - Math.sin(angle + side * 0.5) * 15,
  ]

  const strokes: Stroke[] = [
    { id: "circle", points: circle, t0: T.circle[0], t1: T.circle[1] },
    { id: "shaft", points: shaft, t0: T.arrow[0], t1: T.arrow[1] },
    { id: "head1", points: line(head(1), tip), t0: T.head1[0], t1: T.head1[1] },
    { id: "head2", points: line(tip, head(-1)), t0: T.head2[0], t1: T.head2[1] },
  ]

  const pin1: Point = [a.badge.x + a.badge.w + 2, a.badge.y - 2]
  const pin2: Point =
    mode === "mac" ? [a.actions.x + a.actions.w - 92, a.actions.y + 10] : [a.actions.x + a.actions.w * 0.52, a.actions.y + 8]
  const pins: Pin[] = [
    { n: 1, t: T.pin1, at: pin1 },
    { n: 2, t: T.pin2, at: pin2 },
  ]

  const finish = chipCenter(PILL.cols.recording, 3)
  const prompt: Point = [AGENT.x + 140, AGENT.y + AGENT.h - 30]
  const clicks: Click[] = [...pins.map((p) => ({ t: p.t, at: p.at })), { t: T.finish, at: finish }, { t: T.promptClick, at: prompt }]

  const path: Waypoint[] = [
    { t: 0, at: [880, 560] },
    { t: 1400, at: [560, 330] },
    { t: 2400, at: [520, 300] },
  ]

  let drag: Script["drag"]
  if (mode === "mac") {
    drag = { from: [AREA.x, AREA.y], to: [AREA.x + AREA.w, AREA.y + AREA.h] }
    path.push({ t: T.dragStart - 50, at: drag.from }, { t: T.dragEnd, at: drag.to })
  } else {
    const phoneChip = chipCenter(PILL.cols.select, 2)
    clicks.push({ t: T.phoneClick, at: phoneChip })
    path.push({ t: T.phoneClick, at: phoneChip })
  }

  path.push(
    { t: T.circle[0], at: circle[0] },
    { t: T.circle[1], at: circle[circle.length - 1] },
    { t: T.pin1, at: pin1 },
    { t: T.pin1 + 500, at: [pin1[0] + 30, pin1[1] + 40] },
    { t: T.arrow[0], at: shaft[0] },
    { t: T.arrow[1], at: tip },
    { t: T.head1[0], at: head(1) },
    { t: T.head1[1], at: tip },
    { t: T.head2[1], at: head(-1) },
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
    pins,
    clicks,
    path,
    drag,
    frames: [
      { n: 1, t: T.pin1 - T.record, strokes: ["circle"], pins: [1] },
      { n: 2, t: T.pin2 - T.record, strokes: ["shaft", "head1", "head2"], pins: [2] },
    ],
  }
}

export function strokeAt(s: Stroke, t: number) {
  if (t <= s.t0) return null
  const p = easeInOut(progress(t, s.t0, s.t1))
  const count = Math.max(2, Math.ceil(p * s.points.length))
  return { points: s.points.slice(0, count), done: p >= 1 }
}

export type Cursor = { at: Point; drawing: boolean; pressed: boolean; shift: boolean }

export function cursorAt(script: Script, t: number): Cursor {
  const active = script.strokes.find((s) => t > s.t0 && t < s.t1)
  const pressed = script.clicks.some((c) => t >= c.t - 60 && t < c.t + 140)
  const shift = !!active || script.pins.some((p) => t >= p.t - 250 && t < p.t + 250)
  if (active) {
    const pts = strokeAt(active, t)!.points
    return { at: pts[pts.length - 1], drawing: true, pressed: true, shift: true }
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
