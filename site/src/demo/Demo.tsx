import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { MacWindow, PhoneMirror } from "./apps"
import { InkPath, inkPath } from "./ink"
import { type Anchors, BEATS, buildScript, cursorAt, type Mode, type Rect, STAGE, T } from "./script"
import { Agent, Caption, Cursor, fixAmount, Island, Ink, Keys, MenuBar, phoneLive, Ripples, Selection } from "./stage-parts"

type UserStroke = { points: [number, number, number][]; done: boolean }

function usePlayback(duration: number) {
  const [t, setT] = useState(0)
  const [playing, setPlaying] = useState(false)
  const tRef = useRef(0)

  useEffect(() => {
    if (!playing) return
    let raf = 0
    let last = performance.now()
    const tick = (now: number) => {
      const next = Math.min(duration, tRef.current + Math.min(64, now - last))
      last = now
      tRef.current = next
      setT(next)
      if (next >= duration) setPlaying(false)
      else raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [playing, duration])

  const seek = useCallback((v: number) => {
    tRef.current = Math.max(0, Math.min(duration, v))
    setT(tRef.current)
  }, [duration])

  const play = useCallback(() => {
    if (tRef.current >= duration) seek(0)
    setPlaying(true)
  }, [duration, seek])

  return { t, playing, setPlaying, seek, play }
}

const beatIndex = (t: number) => {
  let i = 0
  BEATS.forEach((b, j) => {
    if (t >= b.t) i = j
  })
  return i
}

export function Demo() {
  const [mode, setMode] = useState<Mode>("mac")
  const { t, playing, setPlaying, seek, play } = usePlayback(T.end)
  const wrapRef = useRef<HTMLDivElement>(null)
  const stageRef = useRef<HTMLDivElement>(null)
  const badgeRef = useRef<HTMLSpanElement>(null)
  const actionsRef = useRef<HTMLDivElement>(null)
  const titleRef = useRef<HTMLHeadingElement>(null)
  const [scale, setScale] = useState(1)
  const [anchors, setAnchors] = useState<Anchors | null>(null)
  const [user, setUser] = useState<UserStroke[]>([])
  const [shiftDown, setShiftDown] = useState(false)
  const [drawMode, setDrawMode] = useState(false)
  const started = useRef(false)

  useLayoutEffect(() => {
    const el = wrapRef.current
    if (!el) return
    const ro = new ResizeObserver(([entry]) => setScale(entry.contentRect.width / STAGE.w))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  useLayoutEffect(() => {
    const measure = () => {
      const stage = stageRef.current
      const els = [badgeRef.current, actionsRef.current, titleRef.current]
      if (!stage || els.some((e) => !e)) return
      const base = stage.getBoundingClientRect()
      const s = base.width / STAGE.w
      const rect = (el: Element): Rect => {
        const r = el.getBoundingClientRect()
        return { x: (r.left - base.left) / s, y: (r.top - base.top) / s, w: r.width / s, h: r.height / s }
      }
      setAnchors({ badge: rect(els[0]!), actions: rect(els[1]!), title: rect(els[2]!) })
    }
    measure()
    document.fonts?.ready.then(measure)
  }, [mode])

  useEffect(() => {
    const el = wrapRef.current
    if (!el) return
    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches
    const io = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting && !started.current) {
          started.current = true
          if (!reduce) play()
        }
      },
      { threshold: 0.45 },
    )
    io.observe(el)
    return () => io.disconnect()
  }, [play])

  useEffect(() => {
    const key = (e: KeyboardEvent) => setShiftDown(e.shiftKey)
    const blur = () => setShiftDown(false)
    window.addEventListener("keydown", key)
    window.addEventListener("keyup", key)
    window.addEventListener("blur", blur)
    return () => {
      window.removeEventListener("keydown", key)
      window.removeEventListener("keyup", key)
      window.removeEventListener("blur", blur)
    }
  }, [])

  const script = useMemo(() => (anchors ? buildScript(mode, anchors) : null), [anchors, mode])

  const switchMode = (next: Mode) => {
    if (next === mode) return
    setAnchors(null)
    setUser([])
    seek(0)
    setMode(next)
    setPlaying(true)
  }

  const replay = () => {
    setUser([])
    seek(0)
    setPlaying(true)
  }

  const toStage = (e: React.PointerEvent): [number, number, number] => {
    const r = stageRef.current!.getBoundingClientRect()
    return [(e.clientX - r.left) / scale, (e.clientY - r.top) / scale, e.pointerType === "pen" ? e.pressure : 0.5]
  }

  const onPointerDown = (e: React.PointerEvent) => {
    if (!(e.shiftKey || drawMode || e.pointerType === "pen")) return
    e.preventDefault()
    ;(e.target as Element).setPointerCapture?.(e.pointerId)
    setUser((s) => [...s, { points: [toStage(e)], done: false }])
  }
  const onPointerMove = (e: React.PointerEvent) => {
    setUser((s) => {
      const last = s[s.length - 1]
      if (!last || last.done) return s
      return [...s.slice(0, -1), { ...last, points: [...last.points, toStage(e)] }]
    })
  }
  const onPointerUp = () => {
    setUser((s) => {
      const last = s[s.length - 1]
      if (!last || last.done) return s
      return [...s.slice(0, -1), { ...last, done: true }]
    })
  }

  const onKeyDown = (e: React.KeyboardEvent) => {
    const i = beatIndex(t)
    if (e.key === "ArrowRight") {
      e.preventDefault()
      seek(BEATS[Math.min(BEATS.length - 1, i + 1)].t)
    } else if (e.key === "ArrowLeft") {
      e.preventDefault()
      seek(t - BEATS[i].t > 700 ? BEATS[i].t : BEATS[Math.max(0, i - 1)].t)
    } else if (e.key === " ") {
      e.preventDefault()
      if (playing) setPlaying(false)
      else play()
    } else if (e.key === "z" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault()
      setUser((s) => s.slice(0, -1))
    }
  }

  const { fix, settle } = fixAmount(t)
  const cursor = script ? cursorAt(script, t) : null
  const live = mode === "iphone" ? phoneLive(t) : 0
  const refs = { badge: badgeRef, actions: actionsRef, title: titleRef }
  const drawing = shiftDown || drawMode

  return (
    <div className="demo">
      <div
        ref={wrapRef}
        className={`stage-wrap${drawing ? " drawing" : ""}`}
        style={{ height: STAGE.h * scale }}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        aria-label="Scrawl demo"
      >
        <div ref={stageRef} className="stage wallpaper" style={{ width: STAGE.w, height: STAGE.h, transform: `scale(${scale})` }}>
          <MenuBar t={t} />
          {mode === "mac" ? <MacWindow fix={fix} settle={settle} refs={refs} /> : <PhoneMirror fix={fix} settle={settle} refs={refs} live={live} />}
          {script && <Agent t={t} mode={mode} script={script} />}
          {script && <Selection t={t} script={script} />}
          {script && <Ink t={t} script={script} />}
          <Caption t={t} mode={mode} />
          <Keys t={t} mode={mode} />
          <Island t={t} mode={mode} />
          {script && <Ripples t={t} script={script} />}
          <svg className="ink" viewBox={`0 0 ${STAGE.w} ${STAGE.h}`}>
            {user.map((s, i) => (
              <InkPath key={i} d={inkPath(s.points.length > 1 ? s.points : [...s.points, s.points[0]], s.done)} />
            ))}
          </svg>
          {cursor && <Cursor c={cursor} />}
        </div>
      </div>
      <Controls
        t={t}
        playing={playing}
        mode={mode}
        drawMode={drawMode}
        onToggle={() => (playing ? setPlaying(false) : play())}
        onReplay={replay}
        onSeek={seek}
        onBeat={(bt) => {
          seek(bt)
          setPlaying(true)
        }}
        onMode={switchMode}
        onDraw={() => setDrawMode((d) => !d)}
        onPause={() => setPlaying(false)}
      />
    </div>
  )
}

function Controls(props: {
  t: number
  playing: boolean
  mode: Mode
  drawMode: boolean
  onToggle: () => void
  onReplay: () => void
  onSeek: (t: number) => void
  onBeat: (t: number) => void
  onMode: (m: Mode) => void
  onDraw: () => void
  onPause: () => void
}) {
  const { t, playing, mode, drawMode } = props
  const trackRef = useRef<HTMLDivElement>(null)
  const scrubbing = useRef(false)

  const seekFromEvent = (e: React.PointerEvent) => {
    const r = trackRef.current!.getBoundingClientRect()
    props.onSeek(((e.clientX - r.left) / r.width) * T.end)
  }

  const current = beatIndex(t)

  return (
    <div className="controls">
      <div className="controls-left">
        <button className="icon-btn" onClick={props.onToggle} aria-label={playing ? "Pause" : "Play"}>
          {playing ? (
            <svg viewBox="0 0 16 16">
              <rect x="3.5" y="2.5" width="3" height="11" rx="1" />
              <rect x="9.5" y="2.5" width="3" height="11" rx="1" />
            </svg>
          ) : (
            <svg viewBox="0 0 16 16">
              <path d="M4.5 2.8v10.4a.6.6 0 00.9.5l8.3-5.2a.6.6 0 000-1L5.4 2.3a.6.6 0 00-.9.5z" />
            </svg>
          )}
        </button>
        <button className="icon-btn" onClick={props.onReplay} aria-label="Replay">
          <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round">
            <path d="M3 8a5 5 0 105-5H5.5" />
            <path d="M7 1L5 3l2 2" />
          </svg>
        </button>
      </div>
      <div className="scrubber">
        <div
          ref={trackRef}
          className="track"
          onPointerDown={(e) => {
            scrubbing.current = true
            ;(e.target as Element).setPointerCapture(e.pointerId)
            props.onPause()
            seekFromEvent(e)
          }}
          onPointerMove={(e) => scrubbing.current && seekFromEvent(e)}
          onPointerUp={() => (scrubbing.current = false)}
        >
          {BEATS.map((b, i) => {
            const end = BEATS[i + 1]?.t ?? T.end
            const fill = Math.min(1, Math.max(0, (t - b.t) / (end - b.t)))
            return (
              <div key={b.label} className="seg" style={{ flexGrow: end - b.t }}>
                <i style={{ transform: `scaleX(${fill})` }} />
              </div>
            )
          })}
        </div>
        <div className="beats">
          {BEATS.map((b, i) => {
            const end = BEATS[i + 1]?.t ?? T.end
            return (
              <button key={b.label} className={i === current ? "on" : ""} style={{ flexGrow: end - b.t }} onClick={() => props.onBeat(b.t)}>
                <span>{i + 1}</span>
                <em>{b.label}</em>
              </button>
            )
          })}
        </div>
      </div>
      <div className="controls-right">
        <div className="toggle" role="tablist">
          <button className={mode === "mac" ? "on" : ""} onClick={() => props.onMode("mac")}>
            Mac
          </button>
          <button className={mode === "iphone" ? "on" : ""} onClick={() => props.onMode("iphone")}>
            iPhone
          </button>
        </div>
        <button className={`draw-btn${drawMode ? " on" : ""}`} onClick={props.onDraw} title="Draw (or hold ⇧ and drag)">
          <kbd>⇧</kbd> Draw
        </button>
      </div>
    </div>
  )
}
