// Runs the lab scenes through the wasm build (what the browser draws) and the native build of the
// same model.rs (what the Mac app runs), frame by frame, and reports the largest difference.
//   bun parity/parity.ts

import { spawnSync } from "node:child_process"
import { readFileSync } from "node:fs"
import { join } from "node:path"

const dir = import.meta.dirname
const root = join(dir, "..")
const spec = JSON.parse(readFileSync(join(dir, "scenes.json"), "utf8")) as {
  dt: number
  seconds: number
  scenes: Record<string, [number, number[]][]>
  opts: Record<string, number[]>
}

const wasmBytes = readFileSync(join(root, "public/pill.wasm"))
const { instance } = await WebAssembly.instantiate(wasmBytes, {})
const x = instance.exports as Record<string, CallableFunction> & { memory: WebAssembly.Memory }
const FRAME_LEN = 32 + 14 * 14

let script = ""
const wasmRuns: Float32Array[][] = []
const names: string[] = []
for (const [optName, opts] of Object.entries(spec.opts)) {
  for (const [scene, steps] of Object.entries(spec.scenes)) {
    names.push(`${scene} · ${optName}`)
    script += `opts ${opts.join(" ")}\n`
    for (const [t, p] of steps) script += `at ${t} ${p.join(" ")}\n`
    script += `run ${spec.seconds} ${spec.dt}\n`

    x.pill_reset()
    x.pill_set_opts(...opts)
    const frames: Float32Array[] = []
    let t = 0
    let i = 0
    // Same f32 clock as the native loop.
    const f32 = Math.fround
    const dt = f32(spec.dt)
    while (t < f32(spec.seconds)) {
      while (i < steps.length && t >= f32(steps[i][0]) - 1e-6) {
        x.pill_set_pres(...steps[i][1])
        i++
      }
      x.pill_advance(dt)
      t = f32(t + dt)
      const ptr = x.pill_frame() as number
      frames.push(new Float32Array(x.memory.buffer, ptr, FRAME_LEN).slice())
    }
    wasmRuns.push(frames)
  }
}

const native = spawnSync(
  process.env.CARGO ?? join(process.env.HOME!, ".cargo/bin/cargo"),
  ["run", "-q", "--release", "--offline", "--example", "trace"],
  { cwd: join(root, "wasm"), input: script, env: { ...process.env, CARGO_TARGET_DIR: "target" }, maxBuffer: 1 << 30 },
)
if (native.status !== 0) {
  console.error(native.stderr.toString())
  process.exit(1)
}
const nativeRuns: number[][][] = [[]]
for (const line of native.stdout.toString().split("\n")) {
  if (!line) continue
  if (line === "end") nativeRuns.push([])
  else nativeRuns[nativeRuns.length - 1].push(line.split(" ").map(Number))
}

let worst = 0
let frames = 0
for (let r = 0; r < wasmRuns.length; r++) {
  let runWorst = 0
  let at = ""
  const w = wasmRuns[r]
  const n = nativeRuns[r]
  if (w.length !== n.length) throw new Error(`${names[r]}: ${w.length} wasm frames vs ${n.length} native`)
  for (let i = 0; i < w.length; i++) {
    frames++
    for (let k = 0; k < FRAME_LEN; k++) {
      const d = Math.abs(w[i][k] - n[i][k])
      if (d > runWorst) {
        runWorst = d
        at = `frame ${i} field ${k}`
      }
    }
  }
  worst = Math.max(worst, runWorst)
  console.log(`${names[r].padEnd(28)} ${w.length} frames  max |wasm − native| = ${runWorst.toExponential(2)}${runWorst ? ` (${at})` : ""}`)
}
console.log(`\n${frames} frames compared; worst difference ${worst.toExponential(2)} (pt or 0–1 levels)`)
process.exit(worst < 1e-3 ? 0 : 1)
