// Real-time hitch probe: plays the demo at 1x (sound off) and records every requestAnimationFrame
// interval, plus long tasks where the browser reports them.
//
//   bun scripts/motion/hitches.ts --url http://localhost:4602/ [--browser webkit] [--size iphone] [--runs 2]
import { chromium, webkit } from "playwright-core"

const args = new Map<string, string>()
for (let i = 2; i < process.argv.length; i += 2) args.set(process.argv[i].replace(/^--/, ""), process.argv[i + 1])
const url = args.get("url") ?? "http://localhost:4602/"
const browserName = args.get("browser") ?? "webkit"
const size = args.get("size") ?? "iphone"
const runs = Number(args.get("runs") ?? "2")
const query = args.get("query") ?? ""
const viewport = size === "iphone" ? { width: 393, height: 852 } : { width: 1280, height: 860 }

const pct = (xs: number[], p: number) => {
  const s = [...xs].sort((a, b) => a - b)
  return s[Math.min(s.length - 1, Math.floor((s.length - 1) * p))] ?? 0
}

const all: number[] = []
const longTasks: number[] = []
for (let r = 0; r < runs; r++) {
  const exe = process.env.PW_WEBKIT && browserName === "webkit" ? { executablePath: process.env.PW_WEBKIT } : {}
  const browser = await (browserName === "webkit" ? webkit : chromium).launch(exe)
  const ctx = await browser.newContext({ viewport, deviceScaleFactor: size === "iphone" ? 3 : 2, isMobile: size === "iphone", hasTouch: size === "iphone" })
  const page = await ctx.newPage()
  await page.goto(`${url}${url.includes("?") ? "&" : "?"}${query}`, { waitUntil: "networkidle" })
  await page.waitForFunction(() => window.__scrawlDemo?.ready?.() === true, null, { timeout: 20000 })
  await page.evaluate(() => (document.querySelector(".stage-wrap") as HTMLElement).scrollIntoView({ block: "center" }))
  await page.waitForTimeout(1500)
  const res = await page.evaluate(
    () =>
      new Promise<{ dts: number[]; long: number[] }>((resolve) => {
        const dts: number[] = []
        const long: number[] = []
        try {
          new PerformanceObserver((l) => l.getEntries().forEach((e) => long.push(e.duration))).observe({ type: "longtask", buffered: false })
        } catch {
          // WebKit has no long task entries.
        }
        window.__scrawlDemo!.seek(0)
        window.__scrawlDemo!.play()
        let last = performance.now()
        const tick = (now: number) => {
          dts.push(now - last)
          last = now
          if (window.__scrawlDemo!.t() >= 22990) resolve({ dts: dts.slice(2), long })
          else requestAnimationFrame(tick)
        }
        requestAnimationFrame(tick)
      }),
  )
  all.push(...res.dts)
  longTasks.push(...res.long)
  await browser.close()
}

const over = (ms: number) => all.filter((d) => d > ms).length
console.log(
  JSON.stringify({
    url,
    browser: browserName,
    size,
    runs,
    frames: all.length,
    p50: +pct(all, 0.5).toFixed(2),
    p95: +pct(all, 0.95).toFixed(2),
    p99: +pct(all, 0.99).toFixed(2),
    max: +Math.max(...all).toFixed(1),
    over25ms: over(25),
    over34ms: over(34),
    longTasks: longTasks.length,
    longTaskMs: +longTasks.reduce((a, b) => a + b, 0).toFixed(0),
  }),
)
