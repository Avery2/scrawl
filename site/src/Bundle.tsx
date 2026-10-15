import { type KeyboardEvent, type ReactNode, useLayoutEffect, useMemo, useRef, useState } from "react"
import { Copy } from "./Copy"
import { MacWindow } from "./demo/apps"
import { measureAnchors } from "./demo/measure"
import { AREA, type Anchors, buildScript, clock, type Point, type Script, STAGE, summaryText, T, WORDS } from "./demo/script"
import { ContactSheet, FrameBadge, KeyframeScene } from "./demo/stage-parts"

const BUNDLE = "scrawl-2026-10-13.scrawl"
const ID = "scrawl-1791924018273-3a1f"
const DURATION = T.finish - T.record
const SUMMARY = summaryText("mac").join("\n")

type Kind = "png" | "md" | "json" | "mp4" | "wav"
type FileId = "sheet" | "summary" | "json" | "frame1" | "frame2" | "video" | "audio"

const FILES: { id: FileId; name: string; role: string; size: string; kind: Kind }[] = [
  { id: "sheet", name: "contact-sheet.png", role: "Annotated keyframes", size: "212 KB", kind: "png" },
  { id: "summary", name: "summary.md", role: "Agent handoff", size: `${new TextEncoder().encode(SUMMARY).length} B`, kind: "md" },
  { id: "json", name: "scrawl.json", role: "Timings, strokes, target", size: "4.8 KB", kind: "json" },
  { id: "frame1", name: "frames/frame-01.png", role: "Keyframe #1", size: "309 KB", kind: "png" },
  { id: "frame2", name: "frames/frame-02.png", role: "Keyframe #2", size: "298 KB", kind: "png" },
  { id: "video", name: "recording.mp4", role: "Screen recording", size: "745 KB", kind: "mp4" },
  { id: "audio", name: "audio.wav", role: "Voice", size: "234 KB", kind: "wav" },
]

const KIND_COLOR: Record<Kind, string> = {
  png: "#ff453a",
  md: "#8e8e93",
  json: "#ffd60a",
  mp4: "#bf5af2",
  wav: "#30d158",
}

function FileIcon({ kind }: { kind: Kind }) {
  return (
    <svg className="file-icon" viewBox="0 0 26 32" aria-hidden>
      <path d="M3 .75h13.4L24.25 8.6V29a2.25 2.25 0 01-2.25 2.25H3A2.25 2.25 0 01.75 29V3A2.25 2.25 0 013 .75z" fill="#2c2c2e" stroke="rgba(255,255,255,.16)" strokeWidth="1.5" />
      <path d="M16.4.75V6.4a2.2 2.2 0 002.2 2.2h5.65" fill="#3a3a3c" stroke="rgba(255,255,255,.16)" strokeWidth="1.5" strokeLinejoin="round" />
      <rect x="2.5" y="19" width="21" height="9" rx="2" fill={KIND_COLOR[kind]} />
      <text x="13" y="26" textAnchor="middle" fontSize="6.6" fontWeight="700" fill={kind === "json" ? "#1c1c1e" : "#fff"} fontFamily="-apple-system, system-ui, sans-serif">
        {kind.toUpperCase()}
      </text>
    </svg>
  )
}

function useMacScript() {
  const stageRef = useRef<HTMLDivElement>(null)
  const badgeRef = useRef<HTMLSpanElement>(null)
  const actionsRef = useRef<HTMLDivElement>(null)
  const titleRef = useRef<HTMLHeadingElement>(null)
  const [anchors, setAnchors] = useState<Anchors | null>(null)

  useLayoutEffect(() => {
    const measure = () => {
      if (!stageRef.current || !badgeRef.current || !actionsRef.current || !titleRef.current) return
      setAnchors(measureAnchors(stageRef.current, badgeRef.current, actionsRef.current, titleRef.current))
    }
    measure()
    document.fonts?.ready.then(measure)
  }, [])

  const script = useMemo(() => (anchors ? buildScript("mac", anchors) : null), [anchors])
  const probe = (
    <div className="bundle-probe" aria-hidden>
      <div ref={stageRef} style={{ width: STAGE.w, height: STAGE.h }}>
        <MacWindow fix={0} settle={0} refs={{ badge: badgeRef, actions: actionsRef, title: titleRef }} />
      </div>
    </div>
  )
  return { script, probe }
}

function useWidth<T extends HTMLElement>() {
  const ref = useRef<T>(null)
  const [width, setWidth] = useState(0)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const ro = new ResizeObserver(([entry]) => setWidth(entry.contentRect.width))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return [ref, width] as const
}

/* ---------- summary.md ---------- */

function SummaryView() {
  return (
    <div className="md">
      {summaryText("mac").map((line, i) => {
        const delayStyle = { animationDelay: `${i * 65}ms` }
        const head = line.match(/^\[scrawl (\S+) · (.+) · (.+)\]$/)
        if (head)
          return (
            <div key={i} className="md-head md-line-reveal" style={delayStyle}>
              <span className="md-dim">[</span>
              <span className="md-key">scrawl</span> <span className="md-num">{head[1]}</span>
              <span className="md-dim"> · </span>
              <span className="md-target">{head[2]}</span>
              <span className="md-dim"> · </span>
              <span className="md-path">{head[3]}</span>
              <span className="md-dim">]</span>
            </div>
          )
        const item = line.match(/^- #(\d+) \((\S+)\) (.*)$/)
        if (item)
          return (
            <div key={i} className="md-item md-line-reveal" style={delayStyle}>
              <span className="md-dim">-</span>
              <span>
                <span className="md-pin">#{item[1]}</span> <span className="md-num">({item[2]})</span> <span className="md-quote">{item[3]}</span>
              </span>
            </div>
          )
        return (
          <div key={i} className="md-line-reveal" style={delayStyle}>
            {line}
          </div>
        )
      })}
    </div>
  )
}

/* ---------- scrawl.json ---------- */

const MORE = Symbol("more")
type Json = string | number | boolean | typeof MORE | Json[] | { [key: string]: Json }

const round = (v: number) => Math.round(v * 10000) / 10000
const norm = (p: Point): [number, number] => [round((p[0] - AREA.x) / AREA.w), round((p[1] - AREA.y) / AREA.h)]

function boxNorm(points: Point[]) {
  const [x0, y0] = norm([Math.min(...points.map((p) => p[0])), Math.min(...points.map((p) => p[1]))])
  const [x1, y1] = norm([Math.max(...points.map((p) => p[0])), Math.max(...points.map((p) => p[1]))])
  return [x0, y0, x1, y1]
}

function bundleJson(script: Script): Json {
  const rel = (t: number) => t - T.record
  const circle = script.strokes.find((s) => s.id === "circle")!
  const shaft = script.strokes.find((s) => s.id === "shaft")!
  const step = (circle.t1 - circle.t0) / (circle.points.length - 1)
  const pts = circle.points.slice(0, 2).flatMap((p, i) => [...norm(p), Math.round(i * step)])
  return {
    version: 1,
    id: ID,
    durationMs: DURATION,
    target: {
      mode: "region",
      app: "Pulse",
      windowTitle: "Deploy Activity",
      frame: { x: AREA.x, y: AREA.y, width: AREA.w, height: AREA.h, scale: 2 },
    },
    transcript: {
      source: "speech-analyzer",
      words: [
        ...WORDS.slice(0, 3).map(([text, at], i) => ({ t0: rel(at), t1: rel(WORDS[i + 1][1]), text })),
        MORE,
      ],
    },
    strokes: [{ t0: rel(circle.t0), t1: rel(circle.t1), pts: [...pts, MORE] }, MORE],
    marks: [
      { id: "m1", shape: "circle", boxNorm: boxNorm(circle.points), on: "Live" },
      { id: "m2", shape: "arrow", to: norm(shaft.points[shaft.points.length - 1]), on: "Promote" },
    ],
    keyframes: script.frames.map((f) => ({ index: f.n, t: f.t, reason: "mark", file: `frames/frame-0${f.n}.png` })),
  }
}

const isLeaf = (v: Json) => typeof v !== "object"

const leafText = (v: Json): string =>
  v === MORE ? "…" : Array.isArray(v) ? `[${v.map(leafText).join(", ")}]` : typeof v === "object" ? "" : typeof v === "string" ? JSON.stringify(v) : String(v)

type Emit = (text: string, cls?: string) => void

function jsonTokens(v: Json, depth: number, emit: Emit) {
  const pad = "  ".repeat(depth + 1)
  if (v === MORE) emit("…", "j-more")
  else if (typeof v === "string") emit(JSON.stringify(v), "j-str")
  else if (typeof v === "number" || typeof v === "boolean") emit(String(v), "j-num")
  else if (Array.isArray(v)) {
    if (v.every(isLeaf)) {
      emit("[")
      v.forEach((x, i) => {
        if (i) emit(", ")
        jsonTokens(x, depth, emit)
      })
      emit("]")
    } else {
      emit("[\n")
      v.forEach((x, i) => {
        emit(pad)
        jsonTokens(x, depth + 1, emit)
        emit(i < v.length - 1 ? ",\n" : "\n")
      })
      emit("  ".repeat(depth) + "]")
    }
  } else {
    const entries = Object.entries(v)
    const flat = entries.every(([, x]) => isLeaf(x) || (Array.isArray(x) && x.every(isLeaf)))
    const width = entries.reduce((n, [k, x]) => n + k.length + leafText(x).length + 6, 4)
    const inline = flat && depth * 2 + width <= 80
    emit(inline ? "{ " : "{\n")
    entries.forEach(([k, x], i) => {
      if (!inline) emit(pad)
      emit(JSON.stringify(k), "j-key")
      emit(": ")
      jsonTokens(x, depth + 1, emit)
      if (i < entries.length - 1) emit(inline ? ", " : ",\n")
    })
    emit(inline ? " }" : "\n" + "  ".repeat(depth) + "}")
  }
}

function jsonText(value: Json) {
  let text = ""
  jsonTokens(value, 0, (t) => (text += t))
  return text
}

function JsonView({ value }: { value: Json }) {
  const out: ReactNode[] = []
  jsonTokens(value, 0, (text, cls) => out.push(cls ? <span key={out.length} className={cls}>{text}</span> : text))
  return <pre className="json">{out}</pre>
}

/* ---------- media ---------- */

function Frame({ script, n, width, height, children }: { script: Script; n: number; width: number; height: number; children?: ReactNode }) {
  const frame = script.frames.find((f) => f.n === n)!
  const s = Math.min(width / AREA.w, height / AREA.h)
  return (
    <div className="bundle-frame" style={{ width: AREA.w * s, height: AREA.h * s }}>
      <KeyframeScene mode="mac" script={script} frame={frame} scale={s} />
      {children ?? <FrameBadge n={frame.n} k={s * (AREA.w / 640)} />}
    </div>
  )
}

function VideoView({ script, width, height }: { script: Script; width: number; height: number }) {
  return (
    <div className="video">
      <Frame script={script} n={2} width={width} height={height - 44}>
        <span className="video-play liquid-glass" aria-hidden>
          <svg viewBox="0 0 16 16">
            <path d="M4.5 2.8v10.4a.6.6 0 00.9.5l8.3-5.2a.6.6 0 000-1L5.4 2.3a.6.6 0 00-.9.5z" />
          </svg>
        </span>
      </Frame>
      <div className="video-bar">
        <span className="video-time">0:00</span>
        <div className="video-track">
          {script.frames.map((f) => (
            <span key={f.n} className="video-mark" style={{ left: `${(f.t / DURATION) * 100}%` }}>
              #{f.n}
            </span>
          ))}
        </div>
        <span className="video-time">{clock(DURATION)}</span>
      </div>
    </div>
  )
}

const BARS = 120

function AudioView({ script }: { script: Script }) {
  const words = WORDS.map(([text, at], i) => ({ text, t0: at - T.record, t1: (WORDS[i + 1]?.[1] ?? at + 400) - T.record }))
  const bars = Array.from({ length: BARS }, (_, i) => {
    const t = ((i + 0.5) / BARS) * DURATION
    const w = words.find((x) => t >= x.t0 && t < Math.min(x.t1, x.t0 + 420))
    if (!w) return 0.06 + 0.04 * Math.abs(Math.sin(i * 2.3))
    const k = (t - w.t0) / Math.min(w.t1 - w.t0, 420)
    return 0.25 + 0.7 * Math.sin(Math.PI * k) * (0.55 + 0.45 * Math.abs(Math.sin(i * 1.7) * Math.cos(i * 0.37)))
  })
  const ticks = Array.from({ length: Math.floor(DURATION / 1000) + 1 }, (_, s) => s)
  return (
    <div className="audio">
      <div className="audio-wave">
        {bars.map((a, i) => (
          <i key={i} style={{ transform: `scaleY(${a})` }} />
        ))}
        {script.frames.map((f) => (
          <span key={f.n} className="audio-mark" style={{ left: `${(f.t / DURATION) * 100}%` }}>
            #{f.n}
          </span>
        ))}
      </div>
      <div className="audio-axis">
        {ticks.map((s) => (
          <span key={s} style={{ left: `${((s * 1000) / DURATION) * 100}%` }}>
            {clock(s * 1000)}
          </span>
        ))}
      </div>
      <p className="audio-words">
        {words.map((w, i) => (
          <span key={i} className="audio-word-chip" style={{ animationDelay: `${i * 18}ms` }}>
            <em>{(w.t0 / 1000).toFixed(1)}</em>
            {w.text}
          </span>
        ))}
      </p>
    </div>
  )
}

/* ---------- explorer ---------- */

export function BundleExplorer() {
  const { script, probe } = useMacScript()
  const [selected, setSelected] = useState<FileId>("sheet")
  const [bodyRef, width] = useWidth<HTMLDivElement>()
  const listRef = useRef<HTMLUListElement>(null)
  const file = FILES.find((f) => f.id === selected)!
  const json = useMemo(() => (script ? bundleJson(script) : null), [script])
  const height = 380

  const onKey = (e: KeyboardEvent) => {
    const step = e.key === "ArrowDown" || e.key === "ArrowRight" ? 1 : e.key === "ArrowUp" || e.key === "ArrowLeft" ? -1 : 0
    if (!step) return
    e.preventDefault()
    const i = (FILES.findIndex((f) => f.id === selected) + step + FILES.length) % FILES.length
    setSelected(FILES[i].id)
    listRef.current?.querySelectorAll("button")[i]?.focus()
  }

  let preview: ReactNode = null
  if (script && width > 0) {
    if (selected === "sheet") preview = <ContactSheet mode="mac" script={script} width={width} height={height - 20} develop />
    else if (selected === "summary") preview = <SummaryView />
    else if (selected === "json" && json) preview = <JsonView value={json} />
    else if (selected === "frame1") preview = <Frame script={script} n={1} width={width} height={height} />
    else if (selected === "frame2") preview = <Frame script={script} n={2} width={width} height={height} />
    else if (selected === "video") preview = <VideoView script={script} width={width} height={height} />
    else if (selected === "audio") preview = <AudioView script={script} />
  }

  const text = selected === "summary" ? SUMMARY : selected === "json" && json ? jsonText(json) : null

  return (
    <div className="bundle">
      {probe}
      <div className="bundle-bar">
        <span className="lights">
          <i />
          <i />
          <i />
        </span>
        <span className="bundle-title">
          <img src="/app-icon.png" width={17} height={17} decoding="async" alt="" />
          {BUNDLE}
        </span>
      </div>
      <div className="bundle-body">
        <ul ref={listRef} className="bundle-files" role="tablist" aria-orientation="vertical" onKeyDown={onKey}>
          {FILES.map((f) => (
            <li key={f.id}>
              <button role="tab" aria-selected={f.id === selected} tabIndex={f.id === selected ? 0 : -1} className={f.id === selected ? "on" : ""} onClick={() => setSelected(f.id)}>
                <FileIcon kind={f.kind} />
                <span className="bf-text">
                  <span className="bf-name">{f.name}</span>
                  <span className="bf-meta">
                    {f.role} · {f.size}
                  </span>
                </span>
              </button>
            </li>
          ))}
        </ul>
        <div className="bundle-preview" role="tabpanel">
          <div className="bundle-preview-head">
            <span>{file.name}</span>
            {text && <Copy text={text} />}
          </div>
          <div ref={bodyRef} className={`bundle-preview-body kind-${file.kind}`}>
            <div className="bundle-preview-inner" key={selected}>
              {preview}
            </div>
          </div>
        </div>
      </div>
    </div>
  )
}
