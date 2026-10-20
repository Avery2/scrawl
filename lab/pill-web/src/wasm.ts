// The Metal pill's model (src/native/pill/model.rs) compiled to wasm: see wasm/src/lib.rs.

export const FRAME_HEADER = 32
export const FRAME_ELEM = 14
export const ELEM_COUNT = 14
export const FRAME_LEN = FRAME_HEADER + FRAME_ELEM * ELEM_COUNT

/** `ElemId` in model.rs order. */
export const ElemId = {
  Lead: 0,
  ChipArea: 1,
  ChipWindow: 2,
  ChipScreen: 3,
  ChipIphone: 4,
  Record: 5,
  Close: 6,
  Undo: 7,
  Cancel: 8,
  Finish: 9,
  Processing: 10,
  DoneLead: 11,
  Paste: 12,
  Inspect: 13,
} as const
export type ElemId = (typeof ElemId)[keyof typeof ElemId]
export const ELEM_NAMES = [
  "lead", "chip:Area", "chip:Window", "chip:Screen", "chip:iPhone", "record", "close",
  "undo", "cancel", "finish", "processing", "done.lead", "paste", "inspect",
]

export type Mode = 0 | 1 | 2 | 3 // Area, Window, Screen, iPhone
export const MODE_NAMES = ["Area", "Window", "Screen", "iPhone"] as const

export type Pres =
  | { kind: "hidden" }
  | { kind: "recording"; chips: boolean; hasIphone: boolean; strokes: number }
  | { kind: "selecting"; mode: Mode; hold: boolean; hasIphone: boolean }
  | { kind: "processing" }
  | { kind: "completed"; label: number }

export const COMPLETED_LABELS = ["Copied", "Sent", "Saved", "Queued"]

export interface Metrics {
  area: number; window: number; screen: number; iphone: number; record: number; undo: number
  cancel: number; finish: number; processing: number; paste: number; inspect: number
  clock_000: number; clock_0000: number
}

export interface Opts {
  origin: 0 | 1 | 2
  holdChips: boolean
  contentBlur: number
  shellBlur: number
  enter: [number, number]
  exit: [number, number]
  morphOpen: [number, number]
  morphClose: [number, number]
  highlightLead: [number, number]
  highlightTrail: [number, number]
  punch: number
}

interface Exports {
  memory: WebAssembly.Memory
  pill_reset(): void
  pill_set_metrics(...v: number[]): void
  pill_set_opts(...v: number[]): void
  pill_set_reduced(on: number): void
  pill_set_pres(kind: number, mode: number, flag: number, iphone: number, n: number): void
  pill_set_completed_label(n: number, w: number): void
  pill_set_clock(secs: number): void
  pill_set_pointer(x: number, y: number, has: number, down: number): void
  pill_hit_element(x: number, y: number): number
  pill_hovered(): number
  pill_hit_island(x: number, y: number): number
  pill_advance(dt: number): void
  pill_is_at_rest(): number
  pill_is_fully_hidden(): number
  pill_time(): number
  pill_frame(): number
  pill_springs(): number
}

export class PillModel {
  private constructor(private x: Exports) {}

  static async load(url = "/pill.wasm"): Promise<PillModel> {
    const { instance } = await WebAssembly.instantiateStreaming(fetch(url), {})
    return new PillModel(instance.exports as unknown as Exports)
  }

  reset() { this.x.pill_reset() }
  setMetrics(m: Metrics) {
    this.x.pill_set_metrics(m.area, m.window, m.screen, m.iphone, m.record, m.undo, m.cancel,
      m.finish, m.processing, m.paste, m.inspect, m.clock_000, m.clock_0000)
  }
  setOpts(o: Opts) {
    this.x.pill_set_opts(o.origin, o.holdChips ? 1 : 0, o.contentBlur, o.shellBlur, ...o.enter,
      ...o.exit, ...o.morphOpen, ...o.morphClose, ...o.highlightLead, ...o.highlightTrail, o.punch)
  }
  setReduced(on: boolean) { this.x.pill_set_reduced(on ? 1 : 0) }
  setPres(p: Pres) {
    switch (p.kind) {
      case "hidden": return this.x.pill_set_pres(0, 0, 0, 0, 0)
      case "recording": return this.x.pill_set_pres(1, 0, p.chips ? 1 : 0, p.hasIphone ? 1 : 0, p.strokes)
      case "selecting": return this.x.pill_set_pres(2, p.mode, p.hold ? 1 : 0, p.hasIphone ? 1 : 0, 0)
      case "processing": return this.x.pill_set_pres(3, 0, 0, 0, 0)
      case "completed": return this.x.pill_set_pres(4, 0, 0, 0, p.label)
    }
  }
  setCompletedLabel(n: number, width: number) { this.x.pill_set_completed_label(n, width) }
  setClock(secs: number) { this.x.pill_set_clock(secs) }
  setPointer(p: [number, number] | null, down: boolean) {
    this.x.pill_set_pointer(p?.[0] ?? 0, p?.[1] ?? 0, p ? 1 : 0, down ? 1 : 0)
  }
  hitElement(x: number, y: number): number { return this.x.pill_hit_element(x, y) }
  get hovered(): number { return this.x.pill_hovered() }
  hitIsland(x: number, y: number): boolean { return this.x.pill_hit_island(x, y) !== 0 }
  advance(dt: number) { this.x.pill_advance(dt) }
  get atRest() { return this.x.pill_is_at_rest() !== 0 }
  get fullyHidden() { return this.x.pill_is_fully_hidden() !== 0 }
  get t() { return this.x.pill_time() }

  /** A copy of `model.frame()` (layout in wasm/src/lib.rs `pill_frame`). */
  frame(): Float32Array {
    const ptr = this.x.pill_frame()
    return new Float32Array(this.x.memory.buffer, ptr, FRAME_LEN).slice()
  }

  /** `l r b grow shell_alpha shell_blur press squish`, each `x, v`. */
  springs(): Float32Array {
    const ptr = this.x.pill_springs()
    return new Float32Array(this.x.memory.buffer, ptr, 16).slice()
  }
}

export interface FrameElem {
  id: number
  rect: [number, number, number, number]
  base: [number, number]
  alpha: number
  blur: number
  scale: number
  active: number
  hover: number
  press: number
  enabled: boolean
}

export function elems(f: Float32Array): FrameElem[] {
  const out: FrameElem[] = []
  const n = f[25]
  for (let i = 0; i < n; i++) {
    const o = FRAME_HEADER + i * FRAME_ELEM
    out.push({
      id: f[o],
      rect: [f[o + 1], f[o + 2], f[o + 3], f[o + 4]],
      base: [f[o + 5], f[o + 6]],
      alpha: f[o + 7],
      blur: f[o + 8],
      scale: f[o + 9],
      active: f[o + 10],
      hover: f[o + 11],
      press: f[o + 12],
      enabled: f[o + 13] > 0.5,
    })
  }
  return out
}
