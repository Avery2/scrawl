// Port of src/native/pill/atlas.rs: labels and line icons rasterized white (premultiplied) into
// one texture at the render scale. Canvas 2D stands in for CoreText/CoreGraphics.

import { COMPLETED_LABELS, type Metrics } from "./wasm"

export const HUD_TEXT = 13
const MEDIUM = 500 // NSFontWeightMedium (0.23)
const SEMIBOLD = 600 // NSFontWeightSemibold (0.30)
const FAMILY = `-apple-system, BlinkMacSystemFont, "SF Pro Text", "SF Pro", system-ui, sans-serif`

export interface Sprite {
  size: [number, number] // pt
  uv: [number, number, number, number]
  baseline: number // pt from the sprite's top to its baseline (whole pixels)
}

export type IconKind = "Area" | "Window" | "Screen" | "iPhone" | "Close" | "Check"

const LABELS: [string, number, number][] = [
  ["Area", HUD_TEXT, MEDIUM],
  ["Window", HUD_TEXT, MEDIUM],
  ["Screen", HUD_TEXT, MEDIUM],
  ["iPhone", HUD_TEXT, MEDIUM],
  ["Record", HUD_TEXT, SEMIBOLD],
  ["Undo", HUD_TEXT, MEDIUM],
  ["Cancel", HUD_TEXT, MEDIUM],
  ["Finish", HUD_TEXT, SEMIBOLD],
  ["Processing…", HUD_TEXT, SEMIBOLD],
  ["Paste", HUD_TEXT, SEMIBOLD],
  ["Inspect", HUD_TEXT, MEDIUM],
  ["Esc", 11, MEDIUM],
]

const font = (size: number, weight: number) => `${weight} ${size}px ${FAMILY}`

const measureCtx = document.createElement("canvas").getContext("2d")!

/** CoreText's typographic width, rounded up (`measure_text_pt`). Digits are tabular when `mono`. */
export function measure(text: string, size: number, weight: number, mono = false): number {
  measureCtx.font = font(size, weight)
  if (!mono) return Math.ceil(measureCtx.measureText(text).width)
  const digit = digitAdvance(size, weight)
  let w = 0
  for (const ch of text) w += /\d/.test(ch) ? digit : measureCtx.measureText(ch).width
  return Math.ceil(w)
}

function digitAdvance(size: number, weight: number): number {
  measureCtx.font = font(size, weight)
  let max = 0
  for (let d = 0; d <= 9; d++) max = Math.max(max, measureCtx.measureText(String(d)).width)
  return max
}

export function measureMetrics(): Metrics {
  return {
    area: measure("Area", HUD_TEXT, MEDIUM),
    window: measure("Window", HUD_TEXT, MEDIUM),
    screen: measure("Screen", HUD_TEXT, MEDIUM),
    iphone: measure("iPhone", HUD_TEXT, MEDIUM),
    record: measure("Record", HUD_TEXT, SEMIBOLD),
    undo: measure("Undo", HUD_TEXT, MEDIUM),
    cancel: measure("Cancel", HUD_TEXT, MEDIUM),
    finish: measure("Finish", HUD_TEXT, SEMIBOLD),
    processing: measure("Processing…", HUD_TEXT, SEMIBOLD),
    paste: measure("Paste", HUD_TEXT, SEMIBOLD),
    inspect: measure("Inspect", HUD_TEXT, MEDIUM),
    clock_000: measure("0:00", HUD_TEXT, SEMIBOLD, true),
    clock_0000: measure("00:00", HUD_TEXT, SEMIBOLD, true),
  }
}

export function completedWidth(n: number): number {
  return measure(COMPLETED_LABELS[n] ?? "Copied", HUD_TEXT, SEMIBOLD)
}

/** `rasterize_text`: white text on a pixel canvas with its baseline on a whole pixel row. */
function rasterizeText(text: string, size: number, weight: number, mono: boolean, scale: number) {
  const ctx = measureCtx
  ctx.font = font(size, weight)
  const m = ctx.measureText("Hg")
  const pad = Math.ceil(2 * scale)
  const ascent = Math.ceil(m.fontBoundingBoxAscent * scale)
  const descent = Math.ceil(m.fontBoundingBoxDescent * scale)
  const width = measure(text, size, weight, mono) * scale
  const w = Math.max(4, Math.ceil(width) + 2 * pad)
  const h = Math.max(4, ascent + descent + 2 * pad)
  const canvas = new OffscreenCanvas(w, h)
  const c = canvas.getContext("2d")!
  c.fillStyle = "#fff"
  c.textBaseline = "alphabetic"
  // Draw at the point size under a transform, so the font picks its 13 pt optical size.
  c.setTransform(scale, 0, 0, scale, 0, 0)
  c.font = font(size, weight)
  const x0 = pad / scale
  const y0 = (pad + ascent) / scale
  if (!mono) {
    c.fillText(text, x0, y0)
  } else {
    const digit = digitAdvance(size, weight)
    let x = x0
    for (const ch of text) {
      const cw = c.measureText(ch).width
      if (/\d/.test(ch)) {
        c.fillText(ch, x + (digit - cw) * 0.5, y0)
        x += digit
      } else {
        c.fillText(ch, x, y0)
        x += cw
      }
    }
  }
  return { canvas, size: [w / scale, h / scale] as [number, number], baseline: (pad + ascent) / scale }
}

/** `rasterize_icon`: Scrawl's 24 × 24 line icons, stroked white. */
function rasterizeIcon(kind: IconKind, sizePt: number, scale: number) {
  const px = Math.max(8, Math.round(sizePt * scale))
  const canvas = new OffscreenCanvas(px, px)
  const c = canvas.getContext("2d")!
  const s = px / 24
  c.strokeStyle = "#fff"
  c.lineCap = "round"
  c.lineJoin = "round"
  c.lineWidth = 2.5 * s
  const line = (x0: number, y0: number, x1: number, y1: number) => {
    c.beginPath()
    c.moveTo(x0 * s, y0 * s)
    c.lineTo(x1 * s, y1 * s)
    c.stroke()
  }
  const rrect = (x: number, y: number, w: number, h: number, r: number) => {
    c.beginPath()
    c.roundRect(x * s, y * s, w * s, h * s, Math.min(r * s, (w * s) / 2, (h * s) / 2))
    c.stroke()
  }
  const poly = (...pts: number[]) => {
    c.beginPath()
    c.moveTo(pts[0] * s, pts[1] * s)
    for (let i = 2; i < pts.length; i += 2) c.lineTo(pts[i] * s, pts[i + 1] * s)
    c.stroke()
  }
  switch (kind) {
    case "Area":
      poly(3, 8, 3, 3, 8, 3)
      poly(16, 3, 21, 3, 21, 8)
      poly(21, 16, 21, 21, 16, 21)
      poly(8, 21, 3, 21, 3, 16)
      break
    case "Window":
      rrect(2.5, 4, 19, 16, 2.5)
      line(2.5, 9.5, 21.5, 9.5)
      break
    case "Screen":
      rrect(2, 3, 20, 14, 2)
      line(8, 21, 16, 21)
      line(12, 17, 12, 21)
      break
    case "iPhone":
      rrect(5.5, 2, 13, 20, 3)
      line(10.5, 6, 13.5, 6)
      break
    case "Close":
      c.lineWidth = 2.4 * s
      line(6.5, 6.5, 17.5, 17.5)
      line(17.5, 6.5, 6.5, 17.5)
      break
    case "Check":
      c.lineWidth = 2.6 * s
      poly(5.5, 12.5, 10, 17, 18.5, 7.5)
      break
  }
  return { canvas, size: [sizePt, sizePt] as [number, number] }
}

export class GlyphAtlas {
  readonly width: number
  readonly height: number
  readonly texture: GPUTexture
  private icons = new Map<IconKind, Sprite>()
  private labels = new Map<string, Sprite>()
  private cx = 2
  private cy = 2
  private rowH = 0
  private clockText = ""
  clock!: Sprite
  private completedText = ""
  completed!: Sprite
  private slotY = 0

  constructor(private device: GPUDevice, readonly scale: number) {
    this.width = 2048
    this.height = Math.max(512, Math.ceil(scale * 200))
    this.texture = device.createTexture({
      size: [this.width, this.height],
      format: "rgba8unorm",
      usage: GPUTextureUsage.TEXTURE_BINDING | GPUTextureUsage.COPY_DST | GPUTextureUsage.RENDER_ATTACHMENT,
    })
    const icons: [IconKind, number][] = [
      ["Area", 14], ["Window", 14], ["Screen", 14], ["iPhone", 14], ["Close", 12], ["Check", 13],
    ]
    for (const [kind, size] of icons) {
      const r = rasterizeIcon(kind, size, scale)
      this.icons.set(kind, this.pack(r.canvas, r.size, 0))
    }
    for (const [text, size, weight] of LABELS) {
      const r = rasterizeText(text, size, weight, false, scale)
      this.labels.set(text, this.pack(r.canvas, r.size, r.baseline))
    }
    // Reserved row for the clock and completed label, below what's packed.
    this.slotY = this.cy + this.rowH + 4
    this.updateClock("0:00")
    this.updateCompleted("Copied")
  }

  private upload(canvas: OffscreenCanvas, x: number, y: number) {
    const w = Math.min(canvas.width, this.width - x)
    const h = Math.min(canvas.height, this.height - y)
    if (w <= 0 || h <= 0) return
    this.device.queue.copyExternalImageToTexture(
      { source: canvas },
      { texture: this.texture, origin: [x, y], premultipliedAlpha: true },
      [w, h],
    )
  }

  private clear(x: number, y: number, w: number, h: number) {
    const blank = new OffscreenCanvas(Math.max(1, w), Math.max(1, h))
    blank.getContext("2d")!.clearRect(0, 0, blank.width, blank.height)
    this.upload(blank, x, y)
  }

  private sprite(x: number, y: number, w: number, h: number, size: [number, number], baseline: number): Sprite {
    return {
      size,
      uv: [x / this.width, y / this.height, (x + w) / this.width, (y + h) / this.height],
      baseline,
    }
  }

  private pack(canvas: OffscreenCanvas, size: [number, number], baseline: number): Sprite {
    const { width: w, height: h } = canvas
    if (this.cx + w + 2 > this.width) {
      this.cx = 2
      this.cy += this.rowH + 2
      this.rowH = 0
    }
    const x = this.cx
    const y = this.cy
    this.cx += w + 2
    this.rowH = Math.max(this.rowH, h)
    this.upload(canvas, x, y)
    return this.sprite(x, y, w, h, size, baseline)
  }

  icon(kind: IconKind): Sprite { return this.icons.get(kind)! }
  label(text: string): Sprite { return this.labels.get(text)! }

  updateClock(text: string): Sprite {
    if (text === this.clockText) return this.clock
    this.clockText = text
    const r = rasterizeText(text, HUD_TEXT, SEMIBOLD, true, this.scale)
    const x = 4
    this.clear(x, this.slotY, Math.ceil(120 * this.scale), Math.ceil(30 * this.scale))
    this.upload(r.canvas, x, this.slotY)
    this.clock = this.sprite(x, this.slotY, r.canvas.width, r.canvas.height, r.size, r.baseline)
    return this.clock
  }

  updateCompleted(text: string): Sprite {
    if (text === this.completedText) return this.completed
    this.completedText = text
    const r = rasterizeText(text, HUD_TEXT, SEMIBOLD, false, this.scale)
    const x = Math.ceil(130 * this.scale)
    this.clear(x, this.slotY, Math.ceil(300 * this.scale), Math.ceil(30 * this.scale))
    this.upload(r.canvas, x, this.slotY)
    this.completed = this.sprite(x, this.slotY, r.canvas.width, r.canvas.height, r.size, r.baseline)
    return this.completed
  }

  destroy() { this.texture.destroy() }
}
