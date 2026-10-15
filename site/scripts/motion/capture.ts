// Deterministic frame capture and per-frame motion trace for the stage demo.
//
// Seeks the demo's timeline (window.__scrawlDemo) at a fixed rate of demo time, steps every CSS
// animation and transition by the same dt through the Web Animations API, then records a trace of
// the tracked elements and (optionally) a PNG of the stage. Any frame renders the same on every run.
//
//   bun scripts/motion/capture.ts --url http://localhost:4602/ --out /tmp/cap --from 19500 --to 21800 \
//     [--size desktop|iphone] [--mode mac|iphone] [--fps 60] [--slow 4] [--preroll 2500] [--no-frames]
//     [--browser chromium|webkit] [--query look=2]
import { mkdirSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { chromium, webkit } from "playwright-core"

const args = new Map<string, string>()
for (let i = 2; i < process.argv.length; i++) {
  const a = process.argv[i]
  if (!a.startsWith("--")) continue
  const next = process.argv[i + 1]
  if (next === undefined || next.startsWith("--")) args.set(a.slice(2), "1")
  else args.set(a.slice(2), next), i++
}
const arg = (k: string, d: string) => args.get(k) ?? d

const url = arg("url", "http://localhost:4602/")
const out = arg("out", "/tmp/scrawl-cap")
const size = arg("size", "desktop")
const mode = arg("mode", "mac")
const from = Number(arg("from", "0"))
const to = Number(arg("to", "23000"))
const fps = Number(arg("fps", "60"))
const slow = Number(arg("slow", "1"))
const preroll = Number(arg("preroll", "2500"))
const frames = !args.has("no-frames")
const browserName = arg("browser", "chromium")
const query = arg("query", "")

const viewport = size === "iphone" ? { width: 393, height: 852 } : { width: 1280, height: 860 }
const dpr = size === "iphone" ? 2 : 1

mkdirSync(out, { recursive: true })
if (frames) mkdirSync(join(out, "frames"), { recursive: true })

const browserType = browserName === "webkit" ? webkit : chromium
const browser = await browserType.launch({
  args: browserName === "chromium" ? ["--enable-unsafe-webgpu", "--enable-features=Vulkan", "--use-angle=metal", "--hide-scrollbars"] : [],
})
const context = await browser.newContext({
  viewport,
  deviceScaleFactor: dpr,
  isMobile: size === "iphone" && browserName !== "firefox",
  hasTouch: size === "iphone",
  reducedMotion: args.has("reduced") ? "reduce" : "no-preference",
})
const page = await context.newPage()
const full = `${url}${url.includes("?") ? "&" : "?"}${query}`
await page.goto(full, { waitUntil: "networkidle" })
await page.waitForFunction(() => window.__scrawlDemo?.ready?.() === true, null, { timeout: 20000 })
if (mode === "iphone") {
  await page.evaluate(() => window.__scrawlDemo!.mode("iphone"))
  await page.waitForFunction(() => window.__scrawlDemo?.ready?.() === true)
}
// Let the lazy GPU overlay initialise before the first frame.
await page.waitForTimeout(1200)

await page.evaluate(() => {
  const el = document.querySelector(".stage-wrap") as HTMLElement
  el.scrollIntoView({ block: "center" })
  ;(document.activeElement as HTMLElement | null)?.blur?.()
})

// Virtual clock for CSS: every animation is paused and advanced by exactly dt per step.
await page.evaluate(() => {
  const seen = new WeakSet<Animation>()
  const w = window as unknown as Record<string, unknown>
  w.__drive = (dt: number) => {
    for (const a of document.getAnimations()) {
      if (!seen.has(a)) {
        seen.add(a)
        a.pause()
        a.currentTime = 0
        continue
      }
      const ct = Number(a.currentTime ?? 0)
      a.currentTime = ct + dt
    }
  }

  // Tracked elements: [id, selector, container selector for relative positions].
  // The stage's own app window only: the contact-sheet miniatures and the bundle probe draw cards too.
  const APP = ".stage > :is(.win, .phone)"
  const TRACK: [string, string, string | null][] = [
    ["caption", ".caption", null],
    ["clause", ".caption .caption-clause", ".caption"],
    ["word", ".caption .w", ".caption-clause"],
    ["title", `${APP} .card .card-title`, null],
    ["badge", `${APP} .card .badge`, null],
    ["actions", `${APP} .card .actions-inner`, null],
    ["row", `${APP} .card .rows li`, null],
    ["incidents", ".stage > .win .incidents, .stage > .phone .phone-section", null],
    ["island", ".island", null],
    ["island-content", ".island .island-content:not(.island-leaving)", null],
    ["island-leaving", ".island .island-leaving", null],
    ["prompt", ".tui > :not(.tui-leaving) .tp-box", null],
    ["prompt-leaving", ".tui-leaving .tp-box", null],
    ["paste", ".tui > :not(.tui-leaving) .tp-paste-line", ".tp-text"],
    ["paste-leaving", ".tui-leaving .tp-paste-line", ".tp-text"],
    ["tui-leaving", ".tui-leaving", null],
    ["scroll", ".ts-content", null],
    ["tline", ".ts-content > *", ".ts-content"],
    ["diff", ".tdiff .td-line", ".tdiff"],
    ["tok", ".ta-text .tok", ".ta-text"],
    ["sheet-card", ".tu-images .sheet-card", ".tu-images"],
    ["logo", ".tui > :not(.tui-leaving) .tui-logo", null],
    ["logo-leaving", ".tui-leaving .tui-logo", null],
  ]
  const wrap = () => document.querySelector(".stage-wrap") as HTMLElement
  const drawnOpacity = (el: Element) => {
    let o = 1
    let n: Element | null = el
    const stop = wrap()
    while (n && n !== stop) {
      const cs = getComputedStyle(n)
      if (cs.display === "none" || cs.visibility === "hidden") return 0
      o *= Number(cs.opacity)
      n = n.parentElement
    }
    return o
  }
  w.__trace = () => {
    const base = wrap().getBoundingClientRect()
    const els: Record<string, unknown>[] = []
    for (const [id, sel, rel] of TRACK) {
      const list = Array.from(document.querySelectorAll(sel))
      list.forEach((el, i) => {
        const r = el.getBoundingClientRect()
        if (r.width === 0 && r.height === 0) return
        let rx = r.left - base.left
        let ry = r.top - base.top
        if (rel) {
          const c = el.closest(rel) ?? document.querySelector(rel)
          if (c && c !== el) {
            const cr = c.getBoundingClientRect()
            rx = r.left - cr.left
            ry = r.top - cr.top
          }
        }
        const cs = getComputedStyle(el)
        els.push({
          id: list.length > 1 || id === "tline" ? `${id}#${i}` : id,
          x: +rx.toFixed(2),
          y: +ry.toFixed(2),
          w: +r.width.toFixed(2),
          h: +r.height.toFixed(2),
          o: +drawnOpacity(el).toFixed(3),
          tf: cs.transform === "none" ? "" : cs.transform,
          f: cs.filter === "none" ? "" : cs.filter,
          ts: cs.textShadow === "none" ? "" : cs.textShadow.slice(0, 60),
          len: id === "tok" || id === "paste" ? (el.textContent ?? "").length : undefined,
        })
      })
    }
    return { scale: base.width / 1200, els }
  }
})

const clip = await page.evaluate(() => {
  const r = (document.querySelector(".stage-wrap") as HTMLElement).getBoundingClientRect()
  return { x: Math.round(r.left), y: Math.round(r.top), width: Math.round(r.width), height: Math.round(r.height) }
})

const step = 1000 / fps / slow
const raf = () => page.evaluate(() => new Promise<void>((r) => requestAnimationFrame(() => r())))

// Preroll at the capture rate so in-flight CSS animations are in the right state at `from`.
let t = Math.max(0, from - preroll)
await page.evaluate((ms) => window.__scrawlDemo!.seek(ms), t)
await page.evaluate(() => (window as unknown as { __drive: (d: number) => void }).__drive(0))
const prerollStep = 1000 / 60
while (t < from - 1e-6) {
  const dt = Math.min(prerollStep, from - t)
  t += dt
  await page.evaluate(
    ([ms, d]) => {
      window.__scrawlDemo!.seek(ms)
      ;(window as unknown as { __drive: (d: number) => void }).__drive(d)
    },
    [t, dt] as const,
  )
}

const trace: { t: number; frame: number; scale: number; els: unknown[] }[] = []
let frame = 0
for (t = from; t <= to + 1e-6; t += step, frame++) {
  const snap = await page.evaluate(
    ([ms, d, first]) => {
      window.__scrawlDemo!.seek(ms)
      const w = window as unknown as { __drive: (d: number) => void; __trace: () => { scale: number; els: unknown[] } }
      w.__drive(first ? 0 : d)
      return w.__trace()
    },
    [t, step, frame === 0] as const,
  )
  trace.push({ t: +t.toFixed(3), frame, ...snap })
  if (frames) {
    await raf()
    await page.screenshot({ path: join(out, "frames", `f${String(frame).padStart(5, "0")}.png`), clip })
  }
  if (frame % 120 === 0) process.stderr.write(`  ${out}: t=${t.toFixed(0)}\n`)
}

writeFileSync(join(out, "trace.json"), JSON.stringify({ url: full, size, mode, from, to, fps, slow, frames: trace }))
const diag = await page.evaluate(() => (window as unknown as { __scrawlGpu?: () => unknown }).__scrawlGpu?.() ?? null)
writeFileSync(join(out, "meta.json"), JSON.stringify({ viewport, dpr, clip, diag }, null, 2))
await browser.close()
process.stderr.write(`wrote ${trace.length} frames to ${out}\n`)
