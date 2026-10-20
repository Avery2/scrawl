// The last transition ("take"): every simulated frame, plotted as lanes of the model's channels
// with the frame ticks, scrubbable.

import type { Action, Snapshot } from "./coordinator"
import { ELEM_NAMES, elems } from "./wasm"

export interface TakeFrame {
  /** Model seconds since the take began. */
  t: number
  /** Wall-clock ms between this rAF and the previous one. */
  realDt: number
  f: Float32Array
  sp: Float32Array
  clock: string
  completedLabel: number
}

export interface Take {
  frames: TakeFrame[]
  events: { t: number; action: Action; auto: boolean }[]
  /** Coordinator state when it began (the model was at rest there). */
  start: Snapshot
  t0: number
  speed: number
  name: string
  closed: boolean
}

interface Series { name: string; color: string; get: (fr: TakeFrame) => number | null; width?: number }
interface Lane { title: string; unit: string; series: Series[]; fixed?: [number, number] }

const ELEM_COLORS = [
  "#ff6b5e", "#7aa2ff", "#5fd4a8", "#f0c05a", "#c79bff", "#5ab8f0", "#ff9ed1",
  "#a3d86b", "#ffb36b", "#8fe0e0", "#d0d0d0", "#69d89f", "#3b5cf6", "#e6a1ff",
]

function elemSeries(field: "alpha" | "blur"): Series[] {
  return ELEM_NAMES.map((name, id) => ({
    name,
    color: ELEM_COLORS[id],
    width: 1,
    get: (fr: TakeFrame) => {
      const e = elems(fr.f).find((x) => x.id === id)
      return e ? e[field] : null
    },
  }))
}

const LANES: Lane[] = [
  {
    title: "walls", unit: "pt", series: [
      { name: "left", color: "#7aa2ff", get: (fr) => (fr.f[6] > 0.002 ? fr.f[1] : null) },
      { name: "right", color: "#ff6b5e", get: (fr) => (fr.f[6] > 0.002 ? fr.f[3] : null) },
    ],
  },
  {
    title: "width", unit: "pt", series: [
      { name: "width", color: "#f0c05a", get: (fr) => (fr.f[6] > 0.002 ? fr.f[3] - fr.f[1] : null) },
    ],
  },
  {
    title: "height · grow", unit: "pt", series: [
      { name: "height", color: "#5fd4a8", get: (fr) => (fr.f[6] > 0.002 ? fr.f[4] - fr.f[2] : null) },
    ],
  },
  {
    title: "highlight edges", unit: "pt", series: [
      { name: "left", color: "#c79bff", get: (fr) => (fr.f[11] > 0.5 ? fr.f[12] : null) },
      { name: "right", color: "#ff9ed1", get: (fr) => (fr.f[11] > 0.5 ? fr.f[12] + fr.f[14] : null) },
    ],
  },
  {
    title: "opacity", unit: "", fixed: [0, 1], series: [
      { name: "shell", color: "#ffffff", width: 2, get: (fr) => fr.f[6] },
      ...elemSeries("alpha"),
    ],
  },
  {
    title: "blur", unit: "pt", series: [
      { name: "shell", color: "#ffffff", width: 2, get: (fr) => fr.f[7] },
      ...elemSeries("blur"),
    ],
  },
]

const LABEL_W = 108
const TICKS_H = 22

export class TimelineView {
  private ctx: CanvasRenderingContext2D
  private hoverX: number | null = null
  onScrub: (index: number) => void = () => {}

  constructor(private canvas: HTMLCanvasElement) {
    this.ctx = canvas.getContext("2d")!
    let dragging = false
    const scrub = (e: PointerEvent) => {
      const i = this.indexAt(e.offsetX)
      if (i !== null) this.onScrub(i)
    }
    canvas.addEventListener("pointerdown", (e) => {
      dragging = true
      canvas.setPointerCapture(e.pointerId)
      scrub(e)
    })
    canvas.addEventListener("pointermove", (e) => {
      this.hoverX = e.offsetX
      if (dragging) scrub(e)
    })
    canvas.addEventListener("pointerup", () => (dragging = false))
    canvas.addEventListener("pointerleave", () => (this.hoverX = null))
  }

  private take: Take | null = null
  private span = 1

  private x(t: number): number {
    const w = this.canvas.clientWidth - LABEL_W - 12
    return LABEL_W + (t / this.span) * w
  }

  private indexAt(px: number): number | null {
    const take = this.take
    if (!take || take.frames.length === 0) return null
    const w = this.canvas.clientWidth - LABEL_W - 12
    const t = ((px - LABEL_W) / w) * this.span
    let best = 0
    for (let i = 0; i < take.frames.length; i++) {
      if (Math.abs(take.frames[i].t - t) < Math.abs(take.frames[best].t - t)) best = i
    }
    return best
  }

  draw(take: Take | null, playhead: number | null, expectedDt: number) {
    this.take = take
    const c = this.ctx
    const dpr = devicePixelRatio
    const W = this.canvas.clientWidth
    const H = this.canvas.clientHeight
    if (this.canvas.width !== Math.round(W * dpr) || this.canvas.height !== Math.round(H * dpr)) {
      this.canvas.width = Math.round(W * dpr)
      this.canvas.height = Math.round(H * dpr)
    }
    c.setTransform(dpr, 0, 0, dpr, 0, 0)
    c.clearRect(0, 0, W, H)
    c.font = "11px ui-monospace, SFMono-Regular, Menlo, monospace"
    c.textBaseline = "middle"
    if (!take || take.frames.length < 2) {
      c.fillStyle = "#7d8590"
      c.fillText("Click a state or scene: the transition is recorded here frame by frame.", LABEL_W, H / 2)
      return
    }
    const frames = take.frames
    this.span = Math.max(0.5, frames[frames.length - 1].t)
    const laneH = (H - TICKS_H - 6) / LANES.length

    // Event markers.
    for (const ev of take.events) {
      const x = this.x(ev.t)
      c.strokeStyle = ev.auto ? "rgba(255,255,255,0.12)" : "rgba(255,255,255,0.28)"
      c.setLineDash([3, 3])
      c.beginPath()
      c.moveTo(x, 0)
      c.lineTo(x, H - TICKS_H)
      c.stroke()
      c.setLineDash([])
      c.fillStyle = "#9da7b3"
      c.fillText(ev.action, x + 3, 7)
    }

    LANES.forEach((lane, li) => {
      const y0 = 14 + li * laneH
      const h = laneH - 10
      let lo = Infinity
      let hi = -Infinity
      if (lane.fixed) [lo, hi] = lane.fixed
      else {
        for (const s of lane.series) for (const fr of frames) {
          const v = s.get(fr)
          if (v !== null && Number.isFinite(v)) {
            lo = Math.min(lo, v)
            hi = Math.max(hi, v)
          }
        }
        if (!Number.isFinite(lo)) [lo, hi] = [0, 1]
        if (hi - lo < 1e-3) {
          lo -= 0.5
          hi += 0.5
        }
        const pad = (hi - lo) * 0.08
        lo -= pad
        hi += pad
      }
      const y = (v: number) => y0 + h - ((v - lo) / (hi - lo)) * h
      c.fillStyle = "rgba(255,255,255,0.03)"
      c.fillRect(LABEL_W, y0, W - LABEL_W - 12, h)
      c.fillStyle = "#c9d1d9"
      c.fillText(lane.title, 8, y0 + 7)
      c.fillStyle = "#6e7681"
      c.fillText(`${fmt(hi)}${lane.unit}`, 8, y0 + 20)
      c.fillText(`${fmt(lo)}${lane.unit}`, 8, y0 + h - 4)
      for (const s of lane.series) {
        c.strokeStyle = s.color
        c.lineWidth = s.width ?? 1.5
        c.globalAlpha = s.width === 1 ? 0.75 : 1
        c.beginPath()
        let pen = false
        let any = false
        let lastY = 0
        for (const fr of frames) {
          const v = s.get(fr)
          if (v === null || !Number.isFinite(v)) {
            pen = false
            continue
          }
          any = true
          // A step per frame: each value holds until the next drawn frame, so a rest (no
          // frames) or a jump shows as what was on screen, never an interpolated ramp.
          const px = this.x(fr.t)
          const py = y(v)
          if (pen) {
            c.lineTo(px, lastY)
            c.lineTo(px, py)
          } else c.moveTo(px, py)
          lastY = py
          pen = true
        }
        if (any) c.stroke()
        // Frame dots on the bold series, so the sampling is visible.
        if (any && (s.width ?? 1.5) > 1 && frames.length < 400) {
          c.fillStyle = s.color
          for (const fr of frames) {
            const v = s.get(fr)
            if (v === null) continue
            c.fillRect(this.x(fr.t) - 1, y(v) - 1, 2, 2)
          }
        }
        c.globalAlpha = 1
      }
    })

    // Frame ticks: one per simulated frame; red where the wall clock skipped a frame.
    const ty = H - TICKS_H + 4
    c.fillStyle = "#6e7681"
    c.fillText("frames", 8, ty + 7)
    for (const fr of frames) {
      const late = fr.realDt > expectedDt * 1.5
      c.fillStyle = late ? "#ff5f56" : "rgba(201,209,217,0.55)"
      c.fillRect(this.x(fr.t) - 0.5, ty, 1, late ? 14 : 9)
    }
    const late = frames.filter((fr) => fr.realDt > expectedDt * 1.5).length
    c.fillStyle = late ? "#ff5f56" : "#5fd4a8"
    c.textAlign = "right"
    c.fillText(`${frames.length} frames · ${late} late · ${(this.span * 1000).toFixed(0)} ms model time @ ${take.speed}×`, W - 12, ty + 7)
    c.textAlign = "left"

    // Playhead and hover readout.
    const ph = playhead ?? frames.length - 1
    const fr = frames[Math.min(ph, frames.length - 1)]
    const px = this.x(fr.t)
    c.strokeStyle = playhead === null ? "rgba(95,212,168,0.5)" : "#5fd4a8"
    c.lineWidth = 1.5
    c.beginPath()
    c.moveTo(px, 0)
    c.lineTo(px, H)
    c.stroke()
    const hi = this.hoverX !== null ? this.indexAt(this.hoverX) : null
    const show = frames[hi ?? ph]
    if (show) {
      const f = show.f
      const vis = f[6] > 0.002
      const text = [
        `#${(hi ?? ph) + 1}  t=${(show.t * 1000).toFixed(1)}ms`,
        vis ? `l=${f[1].toFixed(2)} r=${f[3].toFixed(2)} w=${(f[3] - f[1]).toFixed(2)} h=${(f[4] - f[2]).toFixed(2)}` : "hidden",
        `shell α=${f[6].toFixed(3)} blur=${f[7].toFixed(2)} content α=${f[10].toFixed(3)} blur=${f[9].toFixed(2)}`,
        f[11] > 0.5 ? `hl ${f[12].toFixed(2)}…${(f[12] + f[14]).toFixed(2)} α=${f[16].toFixed(2)}` : "",
      ].filter(Boolean).join("   ")
      c.fillStyle = "rgba(13,17,23,0.85)"
      const tw = c.measureText(text).width
      c.fillRect(W - tw - 20, H - TICKS_H - 22, tw + 12, 16)
      c.fillStyle = "#e6edf3"
      c.fillText(text, W - tw - 14, H - TICKS_H - 14)
    }
  }
}

function fmt(v: number) {
  return Math.abs(v) >= 10 ? v.toFixed(0) : v.toFixed(2)
}

/** Rolling rAF intervals: shows whether the page ever misses a frame. */
export class FrameGraph {
  private ctx: CanvasRenderingContext2D
  private dts: number[] = []
  constructor(private canvas: HTMLCanvasElement) {
    this.ctx = canvas.getContext("2d")!
  }

  push(dt: number) {
    this.dts.push(dt)
    if (this.dts.length > 360) this.dts.shift()
  }

  /** Median interval: the display's refresh period. */
  expected(): number {
    if (this.dts.length < 10) return 1000 / 60
    const s = [...this.dts].sort((a, b) => a - b)
    return s[Math.floor(s.length / 2)]
  }

  draw() {
    const c = this.ctx
    const dpr = devicePixelRatio
    const W = this.canvas.clientWidth
    const H = this.canvas.clientHeight
    if (this.canvas.width !== Math.round(W * dpr)) {
      this.canvas.width = Math.round(W * dpr)
      this.canvas.height = Math.round(H * dpr)
    }
    c.setTransform(dpr, 0, 0, dpr, 0, 0)
    c.clearRect(0, 0, W, H)
    const exp = this.expected()
    const max = Math.max(exp * 3, 40)
    const y = (ms: number) => H - (Math.min(ms, max) / max) * (H - 4)
    for (const ms of [1000 / 120, 1000 / 60, 1000 / 30]) {
      c.strokeStyle = "rgba(255,255,255,0.1)"
      c.beginPath()
      c.moveTo(0, y(ms))
      c.lineTo(W, y(ms))
      c.stroke()
    }
    const bw = W / 360
    let late = 0
    this.dts.forEach((ms, i) => {
      const bad = ms > exp * 1.5
      if (bad) late++
      c.fillStyle = bad ? "#ff5f56" : "#5fd4a8"
      c.fillRect(i * bw, y(ms), Math.max(1, bw - 0.5), H - y(ms))
    })
    c.font = "10px ui-monospace, Menlo, monospace"
    c.fillStyle = "#c9d1d9"
    c.textBaseline = "top"
    c.fillText(`${(1000 / exp).toFixed(0)} Hz · ${exp.toFixed(2)} ms · ${late} late of ${this.dts.length}`, 4, 3)
  }
}
