import { type RefObject, useEffect, useRef, useState } from "react"
import { type Script, STAGE } from "./script"

// Wet-ink sheen: while a red stroke is being drawn, the fresh ink carries a soft specular highlight
// that dries over a few hundred ms behind the nib. Each pixel treats the stroke as a small cylinder
// lit from the upper left, so the highlight slides across the line as it curves, which plain CSS
// can't do. Everything else on the stage is DOM.
//
// WebGPU, then WebGL2, then nothing (the ink reads fine without it). `?fx=webgpu|webgl2|css` forces
// a backend (`css` turns the sheen off). Skipped under reduced motion and when the stage is drawn
// so small that strokes are under ~2.5 px wide.

export type GpuBackend = "webgpu" | "webgl2" | "css" | "initializing"

export type GpuDiagnostics = {
  backend: GpuBackend
  drawCalls: number
  points: number
  avgRenderMs: number
  reducedMotion: boolean
  lastT: number
}

declare global {
  interface Window {
    __scrawlGpu?: () => GpuDiagnostics
    __scrawlForceFx?: "webgpu" | "webgl2" | "css"
  }
}

const MAX_POINTS = 32
const VEC4S = 2 + MAX_POINTS
// How long fresh ink stays glossy behind the nib, and the e-folding time of its drying.
const WET_WINDOW = 1100
const WET_TAU = 360
// A little inside perfect-freehand's 4.5 px outline so the highlight never spills past the ink.
const HALF_WIDTH = 1.85

const WGSL = /* wgsl */ `
struct U { data: array<vec4<f32>, ${VEC4S}> };
@group(0) @binding(0) var<uniform> u: U;

struct VsOut { @builtin(position) pos: vec4<f32>, @location(0) p: vec2<f32> };

@vertex
fn vsMain(@builtin(vertex_index) vid: u32) -> VsOut {
  var c = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  let ndc = c[vid];
  var o: VsOut;
  o.pos = vec4<f32>(ndc, 0.0, 1.0);
  o.p = vec2<f32>((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5) * u.data[0].xy;
  return o;
}

@fragment
fn fsMain(i: VsOut) -> @location(0) vec4<f32> {
  let p = i.p;
  let box = u.data[1];
  if (p.x < box.x || p.y < box.y || p.x > box.z || p.y > box.w) { return vec4<f32>(0.0); }
  let n = i32(u.data[0].w);
  var best = 1e9;
  var c = vec2<f32>(0.0);
  var wet = 0.0;
  for (var k = 0; k < ${MAX_POINTS - 1}; k = k + 1) {
    if (k + 1 >= n) { break; }
    let a = u.data[2 + k];
    let b = u.data[3 + k];
    if (b.w <= 0.0) { continue; }
    let ab = b.xy - a.xy;
    let h = clamp(dot(p - a.xy, ab) / max(dot(ab, ab), 1e-4), 0.0, 1.0);
    let q = a.xy + ab * h;
    let d = length(p - q);
    if (d < best) { best = d; c = q; wet = mix(a.z, b.z, h); }
  }
  let hw = ${HALF_WIDTH.toFixed(2)};
  if (best > hw + 1.0) { return vec4<f32>(0.0); }
  let off = (p - c) / hw;
  let r = min(length(off), 1.0);
  let nrm = vec3<f32>(off * (r / max(length(off), 1e-4)), sqrt(max(0.0, 1.0 - r * r)));
  let l = normalize(vec3<f32>(-0.45, -0.8, 0.9));
  let hv = normalize(l + vec3<f32>(0.0, 0.0, 1.0));
  let spec = pow(max(dot(nrm, hv), 0.0), 24.0);
  let cover = smoothstep(hw + 0.5, hw - 0.5, best);
  let a = spec * wet * cover * u.data[0].z;
  return vec4<f32>(a, a * 0.96, a * 0.92, a);
}
`

const GLSL_VS = /* glsl */ `#version 300 es
precision highp float;
uniform vec4 u_data[${VEC4S}];
out vec2 v_p;
void main() {
  vec2 c[3] = vec2[3](vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
  vec2 ndc = c[gl_VertexID];
  gl_Position = vec4(ndc, 0.0, 1.0);
  v_p = vec2((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5) * u_data[0].xy;
}
`

const GLSL_FS = /* glsl */ `#version 300 es
precision highp float;
uniform vec4 u_data[${VEC4S}];
in vec2 v_p;
out vec4 o;
void main() {
  vec2 p = v_p;
  vec4 box = u_data[1];
  if (p.x < box.x || p.y < box.y || p.x > box.z || p.y > box.w) { o = vec4(0.0); return; }
  int n = int(u_data[0].w);
  float best = 1e9;
  vec2 c = vec2(0.0);
  float wet = 0.0;
  for (int k = 0; k < ${MAX_POINTS - 1}; k++) {
    if (k + 1 >= n) break;
    vec4 a = u_data[2 + k];
    vec4 b = u_data[3 + k];
    if (b.w <= 0.0) continue;
    vec2 ab = b.xy - a.xy;
    float h = clamp(dot(p - a.xy, ab) / max(dot(ab, ab), 1e-4), 0.0, 1.0);
    vec2 q = a.xy + ab * h;
    float d = length(p - q);
    if (d < best) { best = d; c = q; wet = mix(a.z, b.z, h); }
  }
  float hw = ${HALF_WIDTH.toFixed(2)};
  if (best > hw + 1.0) { o = vec4(0.0); return; }
  vec2 off = (p - c) / hw;
  float r = min(length(off), 1.0);
  vec3 nrm = vec3(off * (r / max(length(off), 1e-4)), sqrt(max(0.0, 1.0 - r * r)));
  vec3 l = normalize(vec3(-0.45, -0.8, 0.9));
  vec3 hv = normalize(l + vec3(0.0, 0.0, 1.0));
  float spec = pow(max(dot(nrm, hv), 0.0), 24.0);
  float cover = smoothstep(hw + 0.5, hw - 0.5, best);
  float a = spec * wet * cover * u_data[0].z;
  o = vec4(a, a * 0.96, a * 0.92, a);
}
`

type Renderer = { backend: "webgpu" | "webgl2"; render(u: Float32Array): void; clear(): void; dispose(): void }

async function createWebGpu(canvas: HTMLCanvasElement): Promise<Renderer | null> {
  if (typeof navigator === "undefined" || !("gpu" in navigator)) return null
  try {
    const gpu = (navigator as Navigator & { gpu: GPU }).gpu
    const adapter = await gpu.requestAdapter()
    if (!adapter) return null
    const device = await adapter.requestDevice()
    const ctx = canvas.getContext("webgpu") as GPUCanvasContext | null
    if (!ctx) return null
    const format = gpu.getPreferredCanvasFormat()
    ctx.configure({ device, format, alphaMode: "premultiplied" })
    const module = device.createShaderModule({ code: WGSL })
    // UNIFORM | COPY_DST
    const buffer = device.createBuffer({ size: VEC4S * 16, usage: 0x0040 | 0x0008 })
    const layout = device.createBindGroupLayout({ entries: [{ binding: 0, visibility: 0x1 | 0x2, buffer: { type: "uniform" } }] })
    const bind = device.createBindGroup({ layout, entries: [{ binding: 0, resource: { buffer } }] })
    const blend: GPUBlendState = {
      color: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
      alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
    }
    const pipeline = device.createRenderPipeline({
      layout: device.createPipelineLayout({ bindGroupLayouts: [layout] }),
      vertex: { module, entryPoint: "vsMain" },
      fragment: { module, entryPoint: "fsMain", targets: [{ format, blend }] },
      primitive: { topology: "triangle-list" },
    })
    const pass = (draw: boolean) => {
      const encoder = device.createCommandEncoder()
      const rp = encoder.beginRenderPass({
        colorAttachments: [{ view: ctx.getCurrentTexture().createView(), clearValue: { r: 0, g: 0, b: 0, a: 0 }, loadOp: "clear", storeOp: "store" }],
      })
      if (draw) {
        rp.setPipeline(pipeline)
        rp.setBindGroup(0, bind)
        rp.draw(3)
      }
      rp.end()
      device.queue.submit([encoder.finish()])
    }
    return {
      backend: "webgpu",
      render(u) {
        device.queue.writeBuffer(buffer, 0, u.buffer, u.byteOffset, u.byteLength)
        pass(true)
      },
      clear: () => pass(false),
      dispose() {
        try {
          buffer.destroy()
          device.destroy()
        } catch {
          // already gone
        }
      },
    }
  } catch {
    return null
  }
}

function createWebGl2(canvas: HTMLCanvasElement): Renderer | null {
  try {
    const gl = canvas.getContext("webgl2", { alpha: true, premultipliedAlpha: true, antialias: false, depth: false, stencil: false, powerPreference: "low-power" })
    if (!gl) return null
    const compile = (type: number, src: string) => {
      const sh = gl.createShader(type)!
      gl.shaderSource(sh, src)
      gl.compileShader(sh)
      return gl.getShaderParameter(sh, gl.COMPILE_STATUS) ? sh : null
    }
    const vs = compile(gl.VERTEX_SHADER, GLSL_VS)
    const fs = compile(gl.FRAGMENT_SHADER, GLSL_FS)
    if (!vs || !fs) return null
    const prog = gl.createProgram()!
    gl.attachShader(prog, vs)
    gl.attachShader(prog, fs)
    gl.linkProgram(prog)
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) return null
    const loc = gl.getUniformLocation(prog, "u_data")
    const vao = gl.createVertexArray()
    gl.enable(gl.BLEND)
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA)
    const clear = () => {
      gl.viewport(0, 0, canvas.width, canvas.height)
      gl.clearColor(0, 0, 0, 0)
      gl.clear(gl.COLOR_BUFFER_BIT)
    }
    return {
      backend: "webgl2",
      render(u) {
        clear()
        gl.bindVertexArray(vao)
        gl.useProgram(prog)
        gl.uniform4fv(loc, u)
        gl.drawArrays(gl.TRIANGLES, 0, 3)
      },
      clear,
      dispose() {
        gl.deleteProgram(prog)
        if (vao) gl.deleteVertexArray(vao)
      },
    }
  } catch {
    return null
  }
}

function forcedBackend(): "webgpu" | "webgl2" | "css" | null {
  if (typeof window === "undefined") return null
  if (window.__scrawlForceFx) return window.__scrawlForceFx
  const q = new URLSearchParams(window.location.search).get("fx")
  return q === "webgpu" || q === "webgl2" || q === "css" ? q : null
}

// Inverse of script.ts's easeInOut, so each stroke point knows when the nib passed it.
const invEaseInOut = (y: number) => (y < 0.5 ? Math.cbrt(y / 4) : 1 - Math.cbrt(2 * (1 - y)) / 2)

// The glossy part of every scripted stroke at `t`: points the nib passed in the last WET_WINDOW ms,
// with their wetness, as (x, y, wet, ±halfWidth) where a negative width starts a new polyline.
function wetPoints(script: Script, t: number, out: Float32Array) {
  let n = 0
  let minX = Infinity
  let minY = Infinity
  let maxX = -Infinity
  let maxY = -Infinity
  for (const s of script.strokes) {
    if (t <= s.t0 || t > s.t1 + WET_WINDOW) continue
    const last = s.points.length - 1
    const drawnAt = (i: number) => s.t0 + invEaseInOut(i / last) * (s.t1 - s.t0)
    const pts: [number, number, number][] = []
    for (let i = 0; i <= last; i++) {
      const at = drawnAt(i)
      if (at > t) {
        // The live tip, interpolated like strokeAt so the gloss ends exactly at the nib.
        const a = s.points[i - 1]
        const b = s.points[i]
        const k = (t - drawnAt(i - 1)) / Math.max(1e-3, at - drawnAt(i - 1))
        if (a && b) pts.push([a[0] + (b[0] - a[0]) * k, a[1] + (b[1] - a[1]) * k, 1])
        break
      }
      const age = t - at
      if (age < WET_WINDOW) pts.push([s.points[i][0], s.points[i][1], Math.exp(-age / WET_TAU) * Math.min(1, (WET_WINDOW - age) / 200)])
    }
    if (pts.length < 2) continue
    const room = MAX_POINTS - n
    if (room < 2) break
    const stride = Math.max(1, Math.ceil(pts.length / room))
    const picked = pts.filter((_, i) => i % stride === 0 || i === pts.length - 1)
    picked.forEach(([x, y, wet], i) => {
      const o = (2 + n) * 4
      out[o] = x
      out[o + 1] = y
      out[o + 2] = wet
      out[o + 3] = i === 0 ? -HALF_WIDTH : HALF_WIDTH
      minX = Math.min(minX, x)
      minY = Math.min(minY, y)
      maxX = Math.max(maxX, x)
      maxY = Math.max(maxY, y)
      n++
    })
  }
  out[4] = minX - 4
  out[5] = minY - 4
  out[6] = maxX + 4
  out[7] = maxY + 4
  return n
}

export function StageGpuOverlay({
  t,
  script,
  intensity,
  stageRef,
}: {
  t: number
  script: Script | null
  intensity: number
  stageRef: RefObject<HTMLDivElement | null>
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const rendererRef = useRef<Renderer | null>(null)
  const [backend, setBackend] = useState<GpuBackend>("initializing")
  const uniforms = useRef(new Float32Array(VEC4S * 4))
  const cleared = useRef(false)
  const reduced = useRef(false)
  const tooSmall = useRef(false)
  const samples = useRef<number[]>([])
  const diag = useRef<GpuDiagnostics>({ backend: "initializing", drawCalls: 0, points: 0, avgRenderMs: 0, reducedMotion: false, lastT: 0 })

  useEffect(() => {
    window.__scrawlGpu = () => diag.current
    const mq = window.matchMedia?.("(prefers-reduced-motion: reduce)")
    const onMq = () => {
      reduced.current = !!mq?.matches
      diag.current.reducedMotion = reduced.current
    }
    onMq()
    mq?.addEventListener?.("change", onMq)
    return () => mq?.removeEventListener?.("change", onMq)
  }, [])

  useEffect(() => {
    let cancelled = false
    const canvas = canvasRef.current
    if (!canvas) return
    void (async () => {
      const forced = forcedBackend()
      let r: Renderer | null = null
      if (forced !== "css") {
        if (forced !== "webgl2") r = await createWebGpu(canvas)
        if (!r && forced !== "webgpu") r = createWebGl2(canvas)
      }
      if (cancelled) return r?.dispose()
      rendererRef.current = r
      const b = r?.backend ?? "css"
      setBackend(b)
      diag.current.backend = b
    })()
    return () => {
      cancelled = true
      rendererRef.current?.dispose()
      rendererRef.current = null
    }
  }, [])

  // Backing store matches the stage's drawn size (DPR capped at 2) so the highlight stays crisp.
  useEffect(() => {
    const canvas = canvasRef.current
    const stage = stageRef.current
    if (!canvas || !stage) return
    const sync = () => {
      const scale = stage.getBoundingClientRect().width / STAGE.w
      tooSmall.current = scale < 0.55
      const dpr = Math.min(window.devicePixelRatio || 1, 2)
      const w = Math.max(1, Math.round(STAGE.w * scale * dpr))
      const h = Math.max(1, Math.round(STAGE.h * scale * dpr))
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w
        canvas.height = h
        cleared.current = false
      }
    }
    sync()
    const ro = new ResizeObserver(sync)
    ro.observe(stage)
    return () => ro.disconnect()
  }, [stageRef, backend])

  useEffect(() => {
    diag.current.lastT = t
    const r = rendererRef.current
    if (!r) return
    const u = uniforms.current
    const n = !script || reduced.current || tooSmall.current || intensity <= 0 ? 0 : wetPoints(script, t, u)
    diag.current.points = n
    if (n < 2) {
      if (!cleared.current) r.clear()
      cleared.current = true
      return
    }
    cleared.current = false
    u[0] = STAGE.w
    u[1] = STAGE.h
    u[2] = intensity
    u[3] = n
    const t0 = performance.now()
    r.render(u)
    const s = samples.current
    s.push(performance.now() - t0)
    if (s.length > 120) s.shift()
    diag.current.avgRenderMs = +(s.reduce((a, b) => a + b, 0) / s.length).toFixed(3)
    diag.current.drawCalls++
  }, [t, script, intensity])

  if (backend === "css") return null
  return <canvas ref={canvasRef} className="stage-gpu-canvas" aria-hidden />
}

export default StageGpuOverlay
