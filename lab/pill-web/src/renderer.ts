// Port of `MetalPipelines` and `PillRuntimeState::render_frame` / `emit_elem_prims`
// (src/native/pill/mod.rs) to WebGPU: same three passes, same tile packing, same colours.

import { GlyphAtlas, type IconKind, type Sprite } from "./atlas"
import shaderCode from "./pill.wgsl?raw"
import { COMPLETED_LABELS, ElemId, elems, type FrameElem } from "./wasm"

export const PANEL_W = 640
export const PANEL_H = 84
const OFFSCREEN_W_PT = 960
const OFFSCREEN_H_PT = 256
const TILE_PAD_PT = 20
const TEXT_PAD_PT = 2
const CHIP_H = 32
const HUD_CAP = 9.16
const SEG_PX = 10
const SEG_ICON = 14
const SEG_ICON_GAP = 5
const LEAD_TEXT_X = 12 + 8 + 8
const PROCESSING_TEXT_X = 31.5
const DONE_TEXT_X = 5 + 22 + 8
const MAX_PRIMS = 256
const MAX_ELEMS = 16

const MODE_ICON: Record<number, IconKind> = {
  [ElemId.ChipArea]: "Area",
  [ElemId.ChipWindow]: "Window",
  [ElemId.ChipScreen]: "Screen",
  [ElemId.ChipIphone]: "iPhone",
}
const MODE_LABEL: Record<number, string> = {
  [ElemId.ChipArea]: "Area",
  [ElemId.ChipWindow]: "Window",
  [ElemId.ChipScreen]: "Screen",
  [ElemId.ChipIphone]: "iPhone",
}

export interface RenderState {
  glass: boolean
  /** `model.hovered` and `model.pres` facts the renderer reads (Esc hint, Completed). */
  hovered: number
  completed: boolean
  clock: string
  completedLabel: number
}

type Prims = number[]

export class PillRenderer {
  private atlas!: GlyphAtlas
  private offA!: GPUTexture
  private offB!: GPUTexture
  private tilePipe: GPURenderPipeline
  private blurPipe: GPURenderPipeline
  private compPipe: GPURenderPipeline
  private sampler: GPUSampler
  private tileU: GPUBuffer
  private pillU: GPUBuffer
  private primBuf: GPUBuffer
  private boxBuf: GPUBuffer
  private elemBuf: GPUBuffer
  private tileBG!: GPUBindGroup
  private blurBG!: GPUBindGroup
  private compBG!: GPUBindGroup
  scale = 0

  constructor(
    private device: GPUDevice,
    private ctx: GPUCanvasContext,
    private format: GPUTextureFormat,
  ) {
    const parts = shaderCode.split(/^\/\/ ─── Pass \d.*$/m)
    const preamble = parts[0]
    const mod = (i: number) => device.createShaderModule({ code: preamble + parts[i] })
    const premul: GPUBlendState = {
      color: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
      alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
    }
    const pipe = (m: GPUShaderModule, vs: string, fs: string, fmt: GPUTextureFormat, blend?: GPUBlendState) =>
      device.createRenderPipeline({
        layout: "auto",
        vertex: { module: m, entryPoint: vs },
        fragment: { module: m, entryPoint: fs, targets: [{ format: fmt, blend }] },
        primitive: { topology: "triangle-list" },
      })
    this.tilePipe = pipe(mod(1), "pill_tile_vertex", "pill_tile_fragment", "rgba8unorm", premul)
    this.blurPipe = pipe(mod(2), "pill_blur_h_vertex", "pill_blur_h_fragment", "rgba8unorm")
    this.compPipe = pipe(mod(3), "pill_screen_vertex", "pill_composite_fragment", format)
    this.sampler = device.createSampler({
      magFilter: "linear",
      minFilter: "linear",
      addressModeU: "clamp-to-edge",
      addressModeV: "clamp-to-edge",
    })
    const buf = (size: number, usage: number) => device.createBuffer({ size, usage: usage | GPUBufferUsage.COPY_DST })
    this.tileU = buf(32, GPUBufferUsage.UNIFORM)
    this.pillU = buf(160, GPUBufferUsage.UNIFORM)
    this.primBuf = buf(MAX_PRIMS * 64, GPUBufferUsage.STORAGE)
    this.boxBuf = buf(MAX_ELEMS * 32, GPUBufferUsage.STORAGE)
    this.elemBuf = buf(MAX_ELEMS * 64, GPUBufferUsage.STORAGE)
  }

  /** (Re)creates the scale-dependent textures: `MetalPipelines::new(scale)` and `GlyphAtlas::new`. */
  setScale(scale: number) {
    if (scale === this.scale) return
    this.scale = scale
    this.atlas?.destroy()
    this.offA?.destroy()
    this.offB?.destroy()
    this.atlas = new GlyphAtlas(this.device, scale)
    const off = () =>
      this.device.createTexture({
        size: [Math.round(OFFSCREEN_W_PT * scale), Math.round(OFFSCREEN_H_PT * scale)],
        format: "rgba8unorm",
        usage: GPUTextureUsage.RENDER_ATTACHMENT | GPUTextureUsage.TEXTURE_BINDING,
      })
    this.offA = off()
    this.offB = off()
    const d = this.device
    this.tileBG = d.createBindGroup({
      layout: this.tilePipe.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: { buffer: this.tileU } },
        { binding: 1, resource: { buffer: this.primBuf } },
        { binding: 2, resource: this.atlas.texture.createView() },
        { binding: 3, resource: this.sampler },
      ],
    })
    this.blurBG = d.createBindGroup({
      layout: this.blurPipe.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: { buffer: this.tileU } },
        { binding: 1, resource: { buffer: this.boxBuf } },
        { binding: 2, resource: this.offA.createView() },
        { binding: 3, resource: this.sampler },
      ],
    })
    this.compBG = d.createBindGroup({
      layout: this.compPipe.getBindGroupLayout(0),
      entries: [
        { binding: 0, resource: { buffer: this.pillU } },
        { binding: 1, resource: { buffer: this.elemBuf } },
        { binding: 2, resource: this.offB.createView() },
        { binding: 3, resource: this.atlas.texture.createView() },
        { binding: 4, resource: this.sampler },
      ],
    })
  }

  render(f: Float32Array, st: RenderState) {
    const scale = this.scale
    this.atlas.updateClock(st.clock)
    this.atlas.updateCompleted(COMPLETED_LABELS[st.completedLabel] ?? "Copied")
    const t = f[0]
    const list = elems(f).slice(0, MAX_ELEMS)
    const prims: Prims = []
    const boxes = new Float32Array(MAX_ELEMS * 8)
    const comp = new Float32Array(MAX_ELEMS * 16)

    list.forEach((e, i) => {
      const slotX = (i % 5) * 192 + 4
      const slotY = Math.floor(i / 5) * 112 + 4
      const tileW = Math.min(e.base[0] + 2 * TILE_PAD_PT, 184)
      const tileH = Math.min(e.base[1] + 2 * TILE_PAD_PT, 104)
      this.emitElemPrims(e, slotX + TILE_PAD_PT, slotY + TILE_PAD_PT, t, st.glass, prims)
      boxes.set([slotX, slotY, tileW, tileH, e.blur, 0, 0, 0], i * 8)
      const isMode = MODE_ICON[e.id] !== undefined
      const levels = isMode
        ? st.glass ? [0.9 + 0.1 * e.hover, 1.0] : [0.608 + 0.353 * 0.65 * e.hover, 0.961]
        : [1, 1]
      comp.set(
        [...e.rect, slotX, slotY, tileW, tileH, e.base[0], e.base[1], e.scale, e.alpha, e.blur, TILE_PAD_PT, ...levels],
        i * 16,
      )
    })

    // Esc hint under a hovered Cancel / ✕ (not on Completed).
    let hint = [0, 0, 0, 0]
    let hintUv = [0, 0, 0, 0]
    if (!st.completed && (st.hovered === ElemId.Cancel || st.hovered === ElemId.Close)) {
      const h = list.find((e) => e.id === st.hovered && e.alpha > 0.6)
      if (h) {
        const hw = 30
        const hh = 18
        hint = [h.rect[0] + (h.rect[2] - hw) * 0.5, Math.min(f[4] + 4, PANEL_H - hh - 1), hw, hh]
        hintUv = this.atlas.label("Esc").uv
      }
    }

    const q = this.device.queue
    q.writeBuffer(this.tileU, 0, new Float32Array([this.atlas.width, this.atlas.height, OFFSCREEN_W_PT, OFFSCREEN_H_PT, scale, 0, 0, 0]))
    const primCount = Math.min(prims.length / 16, MAX_PRIMS)
    if (primCount > 0) {
      const pb = new ArrayBuffer(primCount * 64)
      const pf = new Float32Array(pb)
      const pu = new Uint32Array(pb)
      for (let i = 0; i < primCount; i++) {
        for (let k = 0; k < 12; k++) pf[i * 16 + k] = prims[i * 16 + k]
        pu[i * 16 + 12] = prims[i * 16 + 12]
      }
      q.writeBuffer(this.primBuf, 0, pb)
    }
    q.writeBuffer(this.boxBuf, 0, boxes)
    q.writeBuffer(this.elemBuf, 0, comp)

    const ub = new ArrayBuffer(160)
    const uf = new Float32Array(ub)
    const uu = new Uint32Array(ub)
    uf.set([PANEL_W, PANEL_H, OFFSCREEN_W_PT, OFFSCREEN_H_PT, scale], 0)
    uu[5] = list.length
    uu[6] = f[8] > 0.5 ? 1 : 0
    uu[7] = f[18] > 0.5 ? 1 : 0
    uf.set([f[1], f[2], f[3], f[4]], 8) // shape l t r b
    uf.set([f[5], f[6], f[7], st.glass ? 1 : 0], 12)
    uf.set([f[19], f[20], f[21], f[22]], 16)
    uf.set([f[23], f[24], 0, 0], 20)
    uf.set(hint, 24)
    uf.set(hintUv, 28)
    if (f[11] > 0.5) {
      uf.set([f[12], f[13], f[12] + f[14], f[13] + f[15]], 32)
      uf.set([f[15] * 0.5, f[16], f[17], 0], 36)
    }
    q.writeBuffer(this.pillU, 0, ub)

    const enc = this.device.createCommandEncoder()
    const pass = (view: GPUTextureView) =>
      enc.beginRenderPass({
        colorAttachments: [{ view, loadOp: "clear", storeOp: "store", clearValue: [0, 0, 0, 0] }],
      })
    const p1 = pass(this.offA.createView())
    if (primCount > 0) {
      p1.setPipeline(this.tilePipe)
      p1.setBindGroup(0, this.tileBG)
      p1.draw(6, primCount)
    }
    p1.end()
    const p2 = pass(this.offB.createView())
    if (list.length > 0) {
      p2.setPipeline(this.blurPipe)
      p2.setBindGroup(0, this.blurBG)
      p2.draw(6, list.length)
    }
    p2.end()
    const p3 = pass(this.ctx.getCurrentTexture().createView())
    p3.setPipeline(this.compPipe)
    p3.setBindGroup(0, this.compBG)
    p3.draw(3)
    p3.end()
    q.submit([enc.finish()])
  }

  private emitElemPrims(e: FrameElem, ox: number, oy: number, t: number, glass: boolean, out: Prims) {
    const atlas = this.atlas
    const [bw, bh] = e.base
    const shape = (rect: number[], radius: number, color: number[]) =>
      out.push(...rect, radius, 0, 0, 0, ...color, 0, 0, 0, 0)
    const sprite = (rect: number[], uv: number[], color: number[]) => out.push(...rect, ...uv, ...color, 1, 0, 0, 0)
    const px = this.scale
    const snap = (v: number) => Math.round(v * px) / px
    const baseline = snap(oy + CHIP_H * 0.5 + HUD_CAP * 0.5)
    const textAt = (sp: Sprite, glyphX: number) => [snap(glyphX - TEXT_PAD_PT), baseline - sp.baseline, sp.size[0], sp.size[1]]
    const centerText = (sp: Sprite) => textAt(sp, ox + (bw - sp.size[0]) * 0.5 + TEXT_PAD_PT)

    switch (e.id) {
      case ElemId.ChipArea:
      case ElemId.ChipWindow:
      case ElemId.ChipScreen:
      case ElemId.ChipIphone: {
        if (glass) {
          shape([ox, oy, bw, bh], bh * 0.5, [0.06, 0.07, 0.1, 0.2 + 0.1 * e.hover])
        } else {
          const bg = 0.08 * e.hover * (1 - e.active) + 0.06 * e.press
          if (bg > 0.002) shape([ox, oy, bw, bh], bh * 0.5, [1, 1, 1, bg])
        }
        const ic = atlas.icon(MODE_ICON[e.id])
        const tx = atlas.label(MODE_LABEL[e.id])
        sprite([ox + SEG_PX, oy + (CHIP_H - SEG_ICON) * 0.5, SEG_ICON, SEG_ICON], ic.uv, [1, 1, 1, 1])
        sprite(textAt(tx, ox + SEG_PX + SEG_ICON + SEG_ICON_GAP), tx.uv, [1, 1, 1, 1])
        break
      }
      case ElemId.Record:
      case ElemId.Finish:
      case ElemId.Paste: {
        const label = e.id === ElemId.Record ? "Record" : e.id === ElemId.Finish ? "Finish" : "Paste"
        const tx = atlas.label(label)
        const c = (v: number) => Math.min(1, Math.max(0, v))
        shape([ox, oy, bw, bh], bh * 0.5, [
          c(0.231 + 0.063 * e.hover - 0.03 * e.press),
          c(0.361 + 0.059 * e.hover - 0.04 * e.press),
          c(0.965 + 0.035 * e.hover - 0.04 * e.press),
          1,
        ])
        sprite(centerText(tx), tx.uv, [0.95, 0.95, 0.96, 1])
        break
      }
      case ElemId.Lead: {
        if (glass) shape([ox, oy, bw, bh], bh * 0.5, [0.06, 0.07, 0.1, 0.24])
        const tx = atlas.clock
        const cycle = 1.6
        const ripple = (t / cycle) % 1
        const breath = 0.5 + 0.5 * Math.cos((Math.PI * 2 * t) / cycle)
        const haloR = 4 + 6 * ripple
        const haloA = 0.34 * (1 - ripple) ** 2
        shape([ox + 16 - haloR, oy + 16 - haloR, haloR * 2, haloR * 2], haloR, [0.941, 0.267, 0.22, haloA])
        shape([ox + 12, oy + 12, 8, 8], 4, [0.941, 0.267, 0.22, 0.78 + 0.22 * breath])
        sprite(textAt(tx, ox + LEAD_TEXT_X), tx.uv, [0.961, 0.961, 0.969, 1])
        break
      }
      case ElemId.Undo:
      case ElemId.Cancel:
      case ElemId.Inspect: {
        const label = e.id === ElemId.Undo ? "Undo" : e.id === ElemId.Cancel ? "Cancel" : "Inspect"
        if (glass) {
          const a = e.enabled ? 0.2 + 0.12 * e.hover + 0.06 * e.press : 0.12
          shape([ox, oy, bw, bh], bh * 0.5, [0.06, 0.07, 0.1, a])
        } else if (e.enabled && (e.hover > 0.01 || e.press > 0.01)) {
          shape([ox, oy, bw, bh], bh * 0.5, [1, 1, 1, 0.08 * e.hover + 0.06 * e.press])
        }
        const tx = atlas.label(label)
        const fg = !e.enabled ? (glass ? 0.78 : 0.56) : glass ? 0.92 + 0.08 * e.hover : 0.74 + 0.22 * e.hover
        sprite(centerText(tx), tx.uv, [fg, fg, fg, e.enabled ? 1 : 0.9])
        break
      }
      case ElemId.Close: {
        if (glass) {
          shape([ox + 2, oy + 2, bh - 4, bh - 4], (bh - 4) * 0.5, [0.06, 0.07, 0.1, 0.2 + 0.12 * e.hover + 0.06 * e.press])
        } else if (e.hover > 0.01 || e.press > 0.01) {
          shape([ox + 2, oy + 2, bh - 4, bh - 4], (bh - 4) * 0.5, [1, 1, 1, 0.1 * e.hover + 0.06 * e.press])
        }
        const ic = atlas.icon("Close")
        const fg = glass ? 0.9 + 0.1 * e.hover : 0.61 + 0.35 * e.hover
        sprite([ox + (CHIP_H - 12) * 0.5, oy + (CHIP_H - 12) * 0.5, 12, 12], ic.uv, [fg, fg, fg, 1])
        break
      }
      case ElemId.Processing: {
        const head = (t * 8) % 8
        for (let k = 0; k < 8; k++) {
          const ang = k * (Math.PI / 4) - Math.PI / 2
          const dcx = ox + 16 + 6.2 * Math.cos(ang)
          const dcy = oy + 16 + 6.2 * Math.sin(ang)
          const phase = (((head - k) % 8) + 8) % 8
          const da = Math.min(1, Math.max(0.18, 1 - phase / 7.5))
          shape([dcx - 1.35, dcy - 1.35, 2.7, 2.7], 1.35, [0.961, 0.961, 0.969, da])
        }
        const tx = atlas.label("Processing…")
        sprite(textAt(tx, ox + PROCESSING_TEXT_X), tx.uv, [0.961, 0.961, 0.969, 1])
        break
      }
      case ElemId.DoneLead: {
        shape([ox + 5, oy + 5, 22, 22], 11, [0.412, 0.847, 0.624, 0.22])
        const ic = atlas.icon("Check")
        sprite([ox + (CHIP_H - 13) * 0.5, oy + (CHIP_H - 13) * 0.5, 13, 13], ic.uv, [0.412, 0.847, 0.624, 1])
        sprite(textAt(atlas.completed, ox + DONE_TEXT_X), atlas.completed.uv, [0.961, 0.961, 0.969, 1])
        break
      }
    }
  }
}
