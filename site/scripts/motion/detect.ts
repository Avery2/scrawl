// Motion detector over a capture trace (see capture.ts). Thresholds are in CSS px of the viewport
// per 60 fps frame of demo time.
//
//   bun scripts/motion/detect.ts <trace.json> [--json out.json] [--verbose]
//
// Rules
//   pop-in / pop-out  drawn opacity moves > 0.5 in one frame, or an element appears/vanishes while
//                     drawn above 0.5 (deduped against a tracked ancestor doing the same, and
//                     excused when another element takes over its exact place that frame: a handoff)
//   snap              a position step > 1.5 px that stands alone: > 3x the larger neighbouring step
//   reversal          a motion that changes direction more than once (one overshoot is allowed);
//                     a leg shorter than 0.3 px doesn't count (layout rounding, not visible motion)
//   layout-shift      an element whose own transform did not change moved > 0.5 px relative to its
//                     container, outside a planned layout motion
//   size-snap         a width/height step > 2 px that stands alone, like snap
import { readFileSync, writeFileSync } from "node:fs"

type El = { id: string; x: number; y: number; w: number; h: number; o: number; tf: string; f: string; ts: string; len?: number }
type Frame = { t: number; frame: number; scale: number; els: El[] }
type Finding = { rule: string; id: string; t: number; frame: number; detail: string }

const path = process.argv[2]
const jsonOut = process.argv.includes("--json") ? process.argv[process.argv.indexOf("--json") + 1] : null
const verbose = process.argv.includes("--verbose")
const data = JSON.parse(readFileSync(path, "utf8")) as { frames: Frame[]; mode: string }
const frames = data.frames

// Layout motion the demo plans: the deploy rows open up when the fix lands, which moves everything
// below them; the pasted prompt grows. Ids are prefixes; times are demo ms.
const PLANNED_LAYOUT: [string, number, number][] = [
  // The prompt eases taller on paste, so the centred home screen glides up with it.
  ["logo", 16850, 17400],
  ["prompt", 16850, 17400],
  ["row#", 19900, 22600],
  ["actions", 19900, 22600],
  ["incidents", 19900, 22600],
]
const planned = (id: string, t: number) => PLANNED_LAYOUT.some(([p, a, b]) => id.startsWith(p) && t >= a && t <= b)

// Ancestry for dedupe: words live in clauses in the caption; terminal children in the scroll view.
const parentOf = (id: string): string[] => {
  if (id.startsWith("word")) return ["clause", "caption"]
  if (id.startsWith("clause")) return ["caption"]
  if (id.startsWith("diff") || id.startsWith("tok") || id.startsWith("sheet-card")) return ["tline", "scroll"]
  if (id.startsWith("tline")) return ["scroll"]
  if (id.startsWith("paste")) return ["prompt"]
  if (id.startsWith("island-content")) return ["island"]
  return []
}

const byId = (f: Frame) => new Map(f.els.map((e) => [e.id, e]))
const maps = frames.map(byId)
const findings: Finding[] = []
const add = (rule: string, id: string, i: number, detail: string) => findings.push({ rule, id, t: frames[i].t, frame: frames[i].frame, detail })

const ancestorDid = (id: string, i: number, test: (a: El | undefined, b: El | undefined) => boolean) =>
  parentOf(id).some((p) => {
    for (const k of maps[i].keys()) if (k === p || k.startsWith(`${p}#`)) if (test(maps[i - 1].get(k), maps[i].get(k))) return true
    if (!maps[i].has(p) && maps[i - 1].has(p)) return test(maps[i - 1].get(p), undefined)
    if (maps[i].has(p) && !maps[i - 1].has(p)) return test(undefined, maps[i].get(p))
    return false
  })

const visibleO = (e: El | undefined) => (e ? e.o : 0)
const popTest = (a: El | undefined, b: El | undefined) => Math.abs(visibleO(b) - visibleO(a)) > 0.5

const contains = (o: El, e: El) => e.x >= o.x - 1 && e.y >= o.y - 1 && e.x + e.w <= o.x + o.w + 1 && e.y + e.h <= o.y + o.h + 1
const coveredBy = (m: Map<string, El>, e: El) => [...m.values()].some((o) => o.id === "tui-leaving" && o.o > 0.5 && contains(o, e))
const sameBox = (a: El, b: El) => Math.abs(a.x - b.x) < 1 && Math.abs(a.y - b.y) < 1 && Math.abs(a.w - b.w) < 1 && Math.abs(a.h - b.h) < 1
for (let i = 1; i < frames.length; i++) {
  const prev = maps[i - 1]
  const cur = maps[i]
  const ids = new Set([...prev.keys(), ...cur.keys()])
  const pops: { id: string; d: number; box: El; o: number }[] = []
  for (const id of ids) {
    const a = prev.get(id)
    const b = cur.get(id)
    const d = visibleO(b) - visibleO(a)
    if (Math.abs(d) > 0.5 && (a ? a.w * a.h : 0) + (b ? b.w * b.h : 0) > 4) {
      if (ancestorDid(id, i, popTest)) continue
      pops.push({ id, d, box: (d > 0 ? b : a)!, o: Math.abs(d) })
    }
  }
  for (const p of pops) {
    const handoff = pops.some((q) => q !== p && Math.sign(q.d) !== Math.sign(p.d) && sameBox(q.box, p.box) && Math.abs(q.o - p.o) < 0.15)
    // Appearing underneath a cross-fade's opaque leaving layer isn't a visible pop.
    const covered = p.d > 0 && coveredBy(cur, p.box)
    if (!handoff && !covered) add(p.d > 0 ? "pop-in" : "pop-out", p.id, i, `opacity ${(p.d > 0 ? 0 : p.o).toFixed(2)} → ${(p.d > 0 ? p.o : 0).toFixed(2)}`)
  }
}

// Series per element (only frames where it exists and is drawn).
const series = new Map<string, { i: number; e: El }[]>()
frames.forEach((f, i) => {
  for (const e of f.els) {
    if (e.o < 0.02) continue
    let s = series.get(e.id)
    if (!s) series.set(e.id, (s = []))
    s.push({ i, e })
  }
})

for (const [id, s] of series) {
  const steps: { i: number; dx: number; dy: number; dw: number; dh: number; tfSame: boolean }[] = []
  for (let k = 1; k < s.length; k++) {
    if (s[k].i !== s[k - 1].i + 1) {
      steps.push({ i: s[k].i, dx: NaN, dy: NaN, dw: NaN, dh: NaN, tfSame: false })
      continue
    }
    const a = s[k - 1].e
    const b = s[k].e
    steps.push({ i: s[k].i, dx: b.x - a.x, dy: b.y - a.y, dw: b.w - a.w, dh: b.h - a.h, tfSame: a.tf === b.tf })
  }
  const mag = (st: (typeof steps)[number] | undefined) => (st && Number.isFinite(st.dx) ? Math.hypot(st.dx, st.dy) : 0)
  const smag = (st: (typeof steps)[number] | undefined) => (st && Number.isFinite(st.dw) ? Math.max(Math.abs(st.dw), Math.abs(st.dh)) : 0)

  for (let k = 0; k < steps.length; k++) {
    const st = steps[k]
    if (!Number.isFinite(st.dx)) continue
    const here = s.find((x) => x.i === st.i)
    if (here && coveredBy(maps[st.i], here.e)) continue
    const m = mag(st)
    const around = Math.max(mag(steps[k - 1]), mag(steps[k + 1]))
    if (m > 1.5 && m > 3 * around) add("snap", id, st.i, `moved ${m.toFixed(1)}px (neighbours ${around.toFixed(1)}px)`)
    else if (m > 0.5 && st.tfSame && !planned(id, frames[st.i].t) && !id.startsWith("island"))
      add("layout-shift", id, st.i, `moved ${m.toFixed(1)}px with no transform change`)
    const sm = smag(st)
    const saround = Math.max(smag(steps[k - 1]), smag(steps[k + 1]))
    if (sm > 2 && sm > 3 * saround && id !== "scroll") add("size-snap", id, st.i, `size ${st.dw.toFixed(1)}×${st.dh.toFixed(1)}px (neighbours ${saround.toFixed(1)}px)`)
  }

  // Reversals per axis within a motion episode (gaps of ≤ 3 still frames continue it).
  for (const axis of ["dx", "dy"] as const) {
    let sign = 0
    let flips = 0
    let still = 0
    let start = -1
    let travel = 0
    let leg = 0
    const flush = (endI: number) => {
      if (flips > 1 && travel > 3) add("reversal", id, endI, `${axis[1]} changed direction ${flips}× over ${travel.toFixed(1)}px (from frame ${start})`)
      sign = 0
      flips = 0
      travel = 0
      leg = 0
      start = -1
    }
    for (const st of steps) {
      const v = st[axis]
      if (!Number.isFinite(v)) {
        flush(st.i)
        continue
      }
      if (Math.abs(v) < 0.05) {
        if (++still > 3 && start >= 0) flush(st.i)
        continue
      }
      still = 0
      if (start < 0) start = st.i
      travel += Math.abs(v)
      const sg = Math.sign(v)
      if (sign !== 0 && sg !== sign) {
        if (leg >= 0.3) flips++
        leg = 0
      }
      leg += Math.abs(v)
      sign = sg
    }
    flush(steps.at(-1)?.i ?? 0)
  }
}

// Collapse identical findings on consecutive frames into one (a word's rise is one event).
findings.sort((a, b) => a.rule.localeCompare(b.rule) || a.id.localeCompare(b.id) || a.frame - b.frame)
const merged: (Finding & { frames: number })[] = []
for (const f of findings) {
  const last = merged.at(-1)
  if (last && last.rule === f.rule && last.id === f.id && f.frame - (last.frame + last.frames - 1) <= 2) last.frames = f.frame - last.frame + 1
  else merged.push({ ...f, frames: 1 })
}

const counts: Record<string, number> = { "pop-in": 0, "pop-out": 0, snap: 0, reversal: 0, "layout-shift": 0, "size-snap": 0 }
for (const f of merged) counts[f.rule] = (counts[f.rule] ?? 0) + 1

// Static polish checks: glowing text and blur radii seen anywhere.
let glowFrames = 0
let maxBlur = 0
for (const f of frames)
  for (const e of f.els) {
    if (e.ts && /rgba?\([^)]*\)\s+0px\s+0px\s+[1-9]/.test(e.ts) && e.o > 0.05) glowFrames++
    const m = e.f.match(/blur\(([\d.]+)px\)/)
    if (m) maxBlur = Math.max(maxBlur, Number(m[1]))
  }

console.log(`${path}`)
console.log(`  frames ${frames.length}  ${Object.entries(counts).map(([k, v]) => `${k} ${v}`).join("  ")}  | glowing-text samples ${glowFrames}, max blur ${maxBlur.toFixed(1)}px`)
if (verbose) for (const f of merged) console.log(`  ${f.rule.padEnd(12)} ${f.id.padEnd(16)} t=${f.t.toFixed(0).padStart(6)} (${f.frames}f) ${f.detail}`)
if (jsonOut) writeFileSync(jsonOut, JSON.stringify({ counts, glowFrames, maxBlur, findings: merged }, null, 2))
