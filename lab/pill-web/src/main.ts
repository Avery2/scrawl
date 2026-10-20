import { completedWidth, measureMetrics } from "./atlas"
import { type Action, Coordinator, SCENES, type Snapshot } from "./coordinator"
import { PANEL_H, PANEL_W, PillRenderer } from "./renderer"
import { FrameGraph, type Take, TimelineView } from "./timeline"
import { fromTune, pillOpts, toTune, tune, tuneJson, tuneRust } from "./tune"
import { ElemId, PillModel } from "./wasm"

const $ = <T extends HTMLElement = HTMLElement>(sel: string) => document.querySelector<T>(sel)!

/** `tick_main` clamps a frame's step to 25 ms of animation time (`0.025 * time_scale`). */
const MAX_DT_S = 0.025
/** A take ends once the model has rested this long with nothing scheduled. */
const TAKE_TAIL_S = 0.35

async function main() {
  const status = $("#status")
  const stage = $("#stage")
  const canvas = $<HTMLCanvasElement>("#pill")
  const glass = $("#glass")

  const adapter = await navigator.gpu?.requestAdapter()
  const device = await adapter?.requestDevice()
  if (!device) {
    $("#nogpu").hidden = false
    status.textContent = "no WebGPU"
    return
  }
  device.lost.then((info) => (status.textContent = `GPU device lost: ${info.message}`))
  const ctx = canvas.getContext("webgpu")!
  const format = navigator.gpu.getPreferredCanvasFormat()
  ctx.configure({ device, format, alphaMode: "premultiplied" })
  const renderer = new PillRenderer(device, ctx, format)

  const model = await PillModel.load()
  await document.fonts.ready
  model.setMetrics(measureMetrics())
  model.reset()

  // ── State ──────────────────────────────────────────────────────────────────────────────
  let speed = 1
  let paused = false
  let playhead: number | null = null
  let zoom = 2
  /** Animation clock (`motion::now()`): runs at `speed`, whether or not the model moves. */
  let anim = 0
  let take: Take | null = null
  let restSince: number | null = null
  let dirty = true
  let queue: { at: number; action: Action }[] = []
  let loopAt: number | null = null
  let liveFrame = model.frame()
  let pointer: { x: number; y: number; down: boolean } | null = null
  let pressDown: number | null = null
  let lastClock = -1
  let lastRealDt = 16.7
  const HIDDEN: Snapshot = {
    s: { phase: "hidden", chips: false, mode: 2, hold: false, hasIphone: false, strokes: 0, completedLabel: 0, recordingSince: 0 },
    timers: [],
    now: 0,
  }

  const coord = new Coordinator(
    (a, auto) => {
      if (take && !take.closed) take.events.push({ t: anim - take.t0, action: a, auto })
      dirty = true
      flash(a)
    },
    () => {
      if (!take || take.closed) startTake(coord.snapshot(), "manual")
    },
  )

  function startTake(start: Snapshot, name: string) {
    take = { frames: [], events: [], start, t0: anim, speed, name, closed: false }
    restSince = null
    playhead = null
  }

  function dispatch(a: Action) {
    if (paused) resume()
    coord.act(a)
  }

  /** Back to a clean model, then the coordinator at `snap`, settled, ready for a script. */
  function prepare(snap: Snapshot) {
    queue = []
    loopAt = null
    model.reset()
    coord.restore(snap, anim)
    syncModel()
    model.setPres(coord.pres())
    for (let i = 0; i < 1200 && !model.atRest; i++) model.advance(1 / 120)
    liveFrame = model.frame()
    dirty = true
  }

  function playScene(name: string) {
    if (paused) resume()
    const hasIphone = $<HTMLInputElement>("#iphone").checked
    prepare({ ...HIDDEN, s: { ...HIDDEN.s, hasIphone }, now: anim })
    startTake(coord.snapshot(), name)
    queue = SCENES[name].map(([t, action]) => ({ at: anim + t, action }))
  }

  function replay() {
    const last = take
    if (!last) return
    if (paused) resume()
    if (SCENES[last.name]) return playScene(last.name)
    prepare(last.start)
    startTake(coord.snapshot(), last.name)
    queue = last.events.filter((e) => !e.auto).map((e) => ({ at: anim + e.t, action: e.action }))
  }

  /** What `NativePill::sync` pushes before each tick. */
  function syncModel() {
    const v = toTune(tune.get() as never)
    model.setOpts(pillOpts(v))
    model.setReduced($<HTMLInputElement>("#reduced").checked)
    const secs = coord.clockSecs()
    if (secs !== lastClock) {
      lastClock = secs
      model.setClock(secs)
      dirty = true
    }
    const pres = coord.pres()
    if (pres.kind === "completed") model.setCompletedLabel(pres.label, completedWidth(pres.label))
    model.setPres(pres)
  }

  function clockText() {
    const s = coord.clockSecs()
    return `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`
  }

  function glassOn(): boolean {
    return tune.get()["pill.glass"] === "glass"
  }

  function pointerSync() {
    if (!pointer) {
      model.setPointer(null, false)
      return
    }
    const inside = model.hitIsland(pointer.x, pointer.y)
    const before = model.hovered
    model.setPointer(inside ? [pointer.x, pointer.y] : null, pointer.down)
    if (model.hovered !== before) dirty = true
  }

  /** One display frame: `tick_main`. */
  function step(realDt: number) {
    const dt = Math.min(realDt / 1000, MAX_DT_S) * speed
    anim += dt
    while (queue.length && queue[0].at <= anim + 1e-9) coord.act(queue.shift()!.action)
    coord.tick(anim)
    if (loopAt !== null && anim >= loopAt) {
      loopAt = null
      replay()
    }
    syncModel()
    pointerSync()
    const gating = $<HTMLInputElement>("#gating").checked
    const animating = !model.atRest || coord.s.phase === "processing"
    if (!gating || animating || dirty) {
      model.advance(dt)
      dirty = false
      liveFrame = model.frame()
      if (take && !take.closed) {
        take.frames.push({
          t: anim - take.t0,
          realDt,
          f: liveFrame,
          sp: model.springs(),
          clock: clockText(),
          completedLabel: coord.s.completedLabel,
        })
      }
    }
    if (take && !take.closed) {
      const busy = queue.length > 0 || coord.timers.some((t) => t.action !== "fold" && t.at - anim < 1.2)
      if (model.atRest && !busy) {
        restSince ??= anim
        if (anim - restSince >= TAKE_TAIL_S || take.frames.length > 4000) {
          take.closed = true
          if ($<HTMLInputElement>("#loop").checked) loopAt = anim + 0.5
        }
      } else {
        restSince = null
      }
    }
  }

  function resume() {
    paused = false
    playhead = null
    $("#pause").innerHTML = "<kbd>Space</kbd> Pause"
  }
  function pause() {
    paused = true
    playhead = take && take.frames.length ? take.frames.length - 1 : null
    $("#pause").innerHTML = "<kbd>Space</kbd> Play"
  }

  function stepFrames(n: number) {
    if (!paused) pause()
    if (take && take.frames.length) {
      const i = (playhead ?? take.frames.length - 1) + n
      if (i >= take.frames.length && n > 0) {
        // Past the end: advance the live model one display frame.
        step(1000 / 60)
        playhead = take.frames.length - 1
      } else {
        playhead = Math.max(0, Math.min(take.frames.length - 1, i))
      }
    } else if (n > 0) {
      step(1000 / 60)
    }
  }

  // ── Rendering ──────────────────────────────────────────────────────────────────────────
  function resize() {
    const scale = devicePixelRatio * zoom
    const w = Math.round(PANEL_W * scale)
    const h = Math.round(PANEL_H * scale)
    if (canvas.width !== w || canvas.height !== h) {
      canvas.width = w
      canvas.height = h
    }
    renderer.setScale(scale)
    stage.style.setProperty("--z", String(zoom))
    document.documentElement.style.setProperty("--z", String(zoom))
    stage.classList.toggle("zoomed", PANEL_W * zoom > stage.clientWidth)
  }

  function draw() {
    const fr = paused && take && playhead !== null ? take.frames[playhead] : null
    const f = fr?.f ?? liveFrame
    const isGlass = glassOn()
    renderer.render(f, {
      glass: isGlass,
      hovered: f[26],
      completed: f[8] > 0.5,
      clock: fr?.clock ?? clockText(),
      completedLabel: fr?.completedLabel ?? coord.s.completedLabel,
    })
    // NSGlassEffectView follows the same silhouette (`render_frame`'s CATransaction).
    if (isGlass && f[6] > 0.005) {
      const z = zoom
      glass.style.display = "block"
      glass.style.left = `${f[1] * z}px`
      glass.style.top = `${f[2] * z}px`
      glass.style.width = `${(f[3] - f[1]) * z}px`
      glass.style.height = `${(f[4] - f[2]) * z}px`
      glass.style.borderRadius = `${f[5] * z}px`
      glass.style.opacity = String(f[6])
      glass.style.background = f[8] > 0.5 ? "rgba(10, 36, 23, 0.28)" : "rgba(15, 18, 26, 0.22)"
    } else {
      glass.style.display = "none"
    }
  }

  const timeline = new TimelineView($<HTMLCanvasElement>("#timeline"))
  timeline.onScrub = (i) => {
    if (!paused) pause()
    playhead = i
  }
  const graph = new FrameGraph($<HTMLCanvasElement>("#framegraph"))

  let lastNow = performance.now()
  function frame(now: number) {
    const realDt = now - lastNow
    lastNow = now
    lastRealDt = realDt
    graph.push(realDt)
    if (!paused) step(realDt)
    draw()
    timeline.draw(take, paused ? playhead : null, graph.expected())
    graph.draw()
    $("#playinfo").textContent = `${coord.s.phase}${coord.s.phase === "selecting" ? ` ${["Area", "Window", "Screen", "iPhone"][coord.s.mode]}${coord.s.hold ? " (hold)" : ""}` : coord.s.phase === "recording" && coord.s.chips ? " + chips" : ""} · model t=${model.t.toFixed(2)}s · ${model.atRest ? "at rest" : "moving"}`
    requestAnimationFrame(frame)
  }

  // ── Pointer on the pill (`poll_pointer_and_input` / `dispatch_click`) ──────────────────
  const toPanel = (e: PointerEvent) => ({ x: e.offsetX / zoom, y: e.offsetY / zoom })
  canvas.addEventListener("pointermove", (e) => {
    pointer = { ...toPanel(e), down: pointer?.down ?? false }
  })
  canvas.addEventListener("pointerleave", () => (pointer = null))
  canvas.addEventListener("pointerdown", (e) => {
    canvas.setPointerCapture(e.pointerId)
    pointer = { ...toPanel(e), down: true }
    pressDown = model.hitElement(pointer.x, pointer.y)
  })
  canvas.addEventListener("pointerup", (e) => {
    pointer = { ...toPanel(e), down: false }
    const up = model.hitElement(pointer.x, pointer.y)
    if (pressDown !== null && pressDown >= 0 && up === pressDown) click(up)
    pressDown = null
  })
  function click(id: number) {
    const selecting = coord.s.phase === "selecting"
    const chip = (name: string) => dispatch(`${selecting ? "select" : "chip"}:${name}` as Action)
    switch (id) {
      case ElemId.ChipArea: return chip("Area")
      case ElemId.ChipWindow: return chip("Window")
      case ElemId.ChipScreen: return chip("Screen")
      case ElemId.ChipIphone: return chip("iPhone")
      case ElemId.Record: return dispatch("record")
      case ElemId.Undo: return dispatch("undo")
      case ElemId.Cancel: return dispatch("cancel")
      case ElemId.Finish: return dispatch("finish")
      case ElemId.Close: return dispatch(coord.s.phase === "completed" ? "dismiss" : "cancel")
      case ElemId.Paste:
      case ElemId.Inspect: return dispatch("dismiss")
    }
  }

  // ── Controls ───────────────────────────────────────────────────────────────────────────
  function flash(a: Action) {
    const b = document.querySelector<HTMLElement>(`[data-act="${a}"]`)
    if (!b) return
    b.classList.remove("flash")
    void b.offsetWidth
    b.classList.add("flash")
  }
  for (const b of document.querySelectorAll<HTMLButtonElement>("[data-act]")) {
    b.addEventListener("click", () => dispatch(b.dataset.act as Action))
  }
  const scenes = $("#scenes")
  Object.keys(SCENES).forEach((name, i) => {
    const b = document.createElement("button")
    b.textContent = name
    b.dataset.key = `⇧${i + 1}`
    b.addEventListener("click", () => playScene(name))
    scenes.append(b)
  })

  function seg(id: string, initial: string, on: (v: string) => void) {
    const el = $(id)
    const pick = (v: string) => {
      for (const b of el.querySelectorAll<HTMLButtonElement>("button")) b.classList.toggle("on", b.dataset.v === v)
      on(v)
    }
    el.addEventListener("click", (e) => {
      const v = (e.target as HTMLElement).closest("button")?.dataset.v
      if (v) pick(v)
    })
    pick(initial)
    return pick
  }
  const SPEEDS = ["1", "0.5", "0.25", "0.1"]
  const pickSpeed = seg("#speed", localStorage.getItem("pill-web.speed") ?? "1", (v) => {
    speed = Number(v)
    localStorage.setItem("pill-web.speed", v)
  })
  seg("#zoom", localStorage.getItem("pill-web.zoom") ?? "2", (v) => {
    zoom = Number(v)
    localStorage.setItem("pill-web.zoom", v)
    resize()
  })
  const imageInput = $<HTMLInputElement>("#image-input")
  const pickBackdrop = seg("#backdrop", localStorage.getItem("pill-web.backdrop") ?? "lab", (v) => {
    if (v === "image" && !$("#wallpaper").style.backgroundImage) {
      imageInput.click()
      return
    }
    const compare = stage.classList.contains("compare")
    stage.className = v
    stage.classList.toggle("compare", compare)
    localStorage.setItem("pill-web.backdrop", v === "image" ? "lab" : v)
    resize()
  })
  imageInput.addEventListener("change", () => {
    const file = imageInput.files?.[0]
    if (!file) return
    $("#wallpaper").style.backgroundImage = `url(${URL.createObjectURL(file)})`
    pickBackdrop("image")
  })
  // Compare: the real Metal pill (`scrawl lab pill`, default tune) under the web one, with the
  // web pill settled in the same state. Paste a screenshot to compare against your own.
  const refFig = $("#reference")
  const refImg = $<HTMLImageElement>("#reference img")
  const showRef = (src: string | null) => {
    refFig.hidden = !src
    stage.classList.toggle("compare", !!src)
    if (src) refImg.src = src
    resize()
  }
  const COMPARE: Record<string, Partial<Snapshot["s"]>> = {
    chips: { phase: "recording", chips: true, recordingSince: -1.5 },
    area: { phase: "selecting", mode: 0, hold: false },
    rec: { phase: "recording", chips: false, strokes: 0, recordingSince: -3.5 },
    done: { phase: "completed", completedLabel: 0 },
  }
  seg("#compare", "off", (v) => {
    if (v === "off") return showRef(null)
    if (paused) resume()
    prepare({ ...HIDDEN, s: { ...HIDDEN.s, ...COMPARE[v] }, now: 0 })
    take = null
    showRef(`/reference/${v}.png`)
  })
  const takeImage = (file: File, reference: boolean) => {
    const url = URL.createObjectURL(file)
    if (reference) showRef(url)
    else {
      $("#wallpaper").style.backgroundImage = `url(${url})`
      pickBackdrop("image")
    }
  }
  document.addEventListener("paste", (e) => {
    const file = [...(e.clipboardData?.files ?? [])].find((f) => f.type.startsWith("image/"))
    if (file) takeImage(file, true)
  })
  stage.addEventListener("dragover", (e) => e.preventDefault())
  stage.addEventListener("drop", (e) => {
    e.preventDefault()
    const file = [...(e.dataTransfer?.files ?? [])].find((f) => f.type.startsWith("image/"))
    if (file) takeImage(file, e.altKey)
  })

  $<HTMLInputElement>("#pixelated").addEventListener("change", (e) =>
    canvas.classList.toggle("pixelated", (e.target as HTMLInputElement).checked),
  )
  $<HTMLInputElement>("#outline").addEventListener("change", (e) =>
    $("#panel-outline").classList.toggle("on", (e.target as HTMLInputElement).checked),
  )
  $<HTMLInputElement>("#autofold").addEventListener("change", (e) => (coord.autoFold = (e.target as HTMLInputElement).checked))
  $<HTMLInputElement>("#iphone").addEventListener("change", (e) => {
    coord.s.hasIphone = (e.target as HTMLInputElement).checked
    dirty = true
  })
  $("#pause").addEventListener("click", () => (paused ? resume() : pause()))
  $("#prev").addEventListener("click", () => stepFrames(-1))
  $("#next").addEventListener("click", () => stepFrames(1))
  $("#replay").addEventListener("click", replay)

  // ── Keyboard (keeps clear of the knobs panel's s, r, u, [, ], ⇧C, ⇧S) ──────────────────
  let holdKey = false
  window.addEventListener("keydown", (e) => {
    const t = e.target as HTMLElement
    if (t.closest("input, textarea, select, [contenteditable], #knobs") || e.metaKey || e.ctrlKey) return
    if (e.shiftKey && /^Digit[1-7]$/.test(e.code)) {
      playScene(Object.keys(SCENES)[Number(e.code.slice(5)) - 1])
      return e.preventDefault()
    }
    if (e.shiftKey) return
    const map: Record<string, Action> = {
      t: "tap", Enter: "record", "1": "chip:Area", "2": "chip:Window", "3": "chip:Screen", "4": "chip:iPhone",
      j: "holdStep", k: "holdRelease", f: "finish", p: "goto:processing", c: "goto:completed",
      Escape: "cancel", m: "mark", z: "undo",
    }
    if (e.key === "h") {
      if (!e.repeat && !holdKey) {
        holdKey = true
        dispatch("holdOpen")
      }
      return e.preventDefault()
    }
    if (map[e.key] && !e.repeat) {
      dispatch(map[e.key])
      return e.preventDefault()
    }
    switch (e.key) {
      case " ": paused ? resume() : pause(); break
      case "ArrowLeft": stepFrames(-1); break
      case "ArrowRight": stepFrames(1); break
      case "0": replay(); break
      case "l": $<HTMLInputElement>("#loop").click(); break
      case "i": $<HTMLInputElement>("#iphone").click(); break
      case "-": pickSpeed(SPEEDS[Math.min(SPEEDS.length - 1, SPEEDS.indexOf(String(speed)) + 1)]); break
      case "=": pickSpeed(SPEEDS[Math.max(0, SPEEDS.indexOf(String(speed)) - 1)]); break
      default: return
    }
    e.preventDefault()
  })
  window.addEventListener("keyup", (e) => {
    // Releasing H is releasing ⌥⇧S: record what the switcher shows.
    if (e.key === "h" && holdKey) {
      holdKey = false
      dispatch("holdRelease")
    }
  })

  // ── Tuning panel and exports ───────────────────────────────────────────────────────────
  tune.mount($("#knobs"), { theme: "dark", density: "compact" } as never)
  tune.subscribe(() => (dirty = true))
  const out = $("#export-out")
  const copy = async (text: string, what: string) => {
    out.textContent = text
    try {
      await navigator.clipboard.writeText(text)
      out.dataset.note = `Copied ${what}`
    } catch {
      out.dataset.note = "Select and copy"
    }
  }
  const values = () => toTune(tune.get() as never)
  $("#copy-json").addEventListener("click", () => copy(tuneJson(values(), speed), "tune.json"))
  $("#copy-rust").addEventListener("click", () => copy(tuneRust(values()), "Rust"))
  $("#apply").addEventListener("click", async () => {
    out.textContent = "Applying…"
    const res = await fetch("/api/apply", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ values: { ...values(), timeScale: 1 } }),
    })
    const json = await res.json()
    out.textContent = res.ok
      ? `Wrote ${json.path} with ${json.bin} tune set (${json.lines.length} keys). Scrawl reloads it within 250 ms.`
      : `Failed: ${json.error}`
  })
  $("#load").addEventListener("click", async () => {
    const res = await fetch("/api/tune")
    const json = await res.json()
    if (!res.ok) return void (out.textContent = `Failed: ${json.error}`)
    tune.set(fromTune(json.values) as never)
    out.textContent = `Loaded ${json.path}:\n${JSON.stringify(json.values, null, 2)}`
  })

  window.addEventListener("resize", resize)
  matchMedia(`(resolution: ${devicePixelRatio}dppx)`).addEventListener("change", resize)
  resize()
  const info = (adapter as GPUAdapter & { info?: GPUAdapterInfo }).info
  status.textContent = `WebGPU ${info?.vendor ?? ""} ${info?.architecture ?? ""} · ${format} · dpr ${devicePixelRatio}`
  requestAnimationFrame(frame)
  void lastRealDt
  Object.assign(window, { __pill: { model, coord, take: () => take, playScene, replay } })
}

main().catch((err) => {
  console.error(err)
  document.querySelector("#status")!.textContent = `error: ${err}`
})
