import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react"
import { MacWindow, PhoneMirror } from "./apps"
import { INK_SHADOW, InkPath, inkPath } from "./ink"
import { measureAnchors, sameAnchors } from "./measure"
import { AGENT, type Anchors, BEATS, buildScript, cursorAt, type Mode, NARRATION, STAGE, T } from "./script"
import * as sfx from "./sfx"
import { Caption, Cursor, fixAmount, Island, Ink, Keys, MenuBar, phoneLive, Ripples, Selection, terminalActive } from "./stage-parts"
import { Terminal } from "./terminal"

type UserStroke = { points: [number, number, number][]; done: boolean }

// `clock` can report an authoritative time (the narration while it plays); otherwise frames advance t.
function usePlayback(duration: number, clock: React.RefObject<() => number | null>) {
  const [t, setT] = useState(0)
  const [playing, setPlaying] = useState(false)
  const tRef = useRef(0)

  useEffect(() => {
    if (!playing) return
    let raf = 0
    let last = performance.now()
    const tick = (now: number) => {
      const master = clock.current()
      const next = Math.min(duration, master !== null ? Math.max(tRef.current, master) : tRef.current + Math.min(64, now - last))
      last = now
      tRef.current = next
      setT(next)
      if (next >= duration) setPlaying(false)
      else raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [playing, duration, clock])

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

// Plays Kit's narration from the moment recording starts. While it plays it drives the demo clock,
// so dropped frames can't pull the words, ink, and voice apart.
function useNarration(t: number, playing: boolean, sound: boolean, mode: Mode, clock: React.RefObject<() => number | null>) {
  const audioRef = useRef<HTMLAudioElement>(null)
  const wanted = useRef(false)
  const live = useRef(false)
  const end = NARRATION[mode].end

  useEffect(() => {
    const a = audioRef.current
    if (!a) return
    const on = () => (live.current = true)
    const off = () => (live.current = false)
    for (const e of ["playing", "seeked"]) a.addEventListener(e, on)
    for (const e of ["pause", "waiting", "seeking", "ended", "emptied"]) a.addEventListener(e, off)
    clock.current = () => (live.current && wanted.current && !a.paused ? T.record + a.currentTime * 1000 : null)
    return () => {
      for (const e of ["playing", "seeked"]) a.removeEventListener(e, on)
      for (const e of ["pause", "waiting", "seeking", "ended", "emptied"]) a.removeEventListener(e, off)
      clock.current = () => null
    }
  }, [clock])

  useEffect(() => {
    const a = audioRef.current
    if (!a) return
    wanted.current = playing && sound && t >= T.record && t < end - 20
    if (!wanted.current) {
      if (!a.paused) a.pause()
      return
    }
    const want = (t - T.record) / 1000
    if (a.paused) {
      a.currentTime = want
      a.play().catch(() => {})
    } else if (Math.abs(a.currentTime - want) > 0.2) {
      a.currentTime = want
    }
    sfx.gateNarration(a, mode)
  }, [t, playing, sound, mode, end])

  // Safari only lets audio start later if it was started once inside a user gesture.
  const unlock = useCallback(() => {
    sfx.unlock()
    const a = audioRef.current
    if (!a || a.dataset.unlocked) return
    a.dataset.unlocked = "1"
    sfx.routeNarration(a)
    a.muted = true
    a.play()
      .then(() => {
        if (!wanted.current) a.pause()
        a.muted = false
      })
      .catch(() => {
        a.muted = false
      })
  }, [])

  return { audioRef, unlock }
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
  const clock = useRef<() => number | null>(() => null)
  const { t, playing, setPlaying, seek, play } = usePlayback(T.end, clock)
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
  const [started, setStarted] = useState(false)
  const [sound, setSound] = useState(true)
  const { audioRef, unlock } = useNarration(t, playing, sound, mode, clock)

  useEffect(() => sfx.setEnabled(sound), [sound])

  const start = (withSound = sound) => {
    unlock()
    setStarted(true)
    setSound(withSound)
    play()
  }

  // A click on the page's own controls.
  const click = (s: sfx.Sound = "tick") => {
    sfx.unlock()
    sfx.play(s, 0.8)
  }

  useLayoutEffect(() => {
    const el = wrapRef.current
    if (!el) return
    const ro = new ResizeObserver(([entry]) => setScale(entry.contentRect.width / STAGE.w))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])

  const { fix, settle } = fixAmount(t)
  const broken = fix === 0
  const fixRef = useRef(fix)
  fixRef.current = fix

  // Strokes target the broken layout, so only measure while nothing has been fixed yet.
  useLayoutEffect(() => {
    if (!broken) return
    const measure = () => {
      if (fixRef.current !== 0 || !stageRef.current || !badgeRef.current || !actionsRef.current || !titleRef.current) return
      const next = measureAnchors(stageRef.current, badgeRef.current, actionsRef.current, titleRef.current)
      setAnchors((prev) => (prev && sameAnchors(prev, next) ? prev : next))
    }
    measure()
    document.fonts?.ready.then(measure)
  }, [mode, broken])

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
    click("toggle")
    setAnchors(null)
    setUser([])
    seek(0)
    setMode(next)
    start()
  }

  const replay = () => {
    click("pulse")
    setUser([])
    seek(0)
    start()
  }

  const toStage = (e: React.PointerEvent): [number, number, number] => {
    const r = stageRef.current!.getBoundingClientRect()
    return [(e.clientX - r.left) / scale, (e.clientY - r.top) / scale, e.pointerType === "pen" ? e.pressure : 0.5]
  }

  const pen = useRef<{ x: number; y: number; at: number; speed: number } | null>(null)

  const onPointerDown = (e: React.PointerEvent) => {
    if (!(e.shiftKey || drawMode || e.pointerType === "pen")) return
    e.preventDefault()
    ;(e.target as Element).setPointerCapture?.(e.pointerId)
    sfx.unlock()
    pen.current = { x: e.clientX, y: e.clientY, at: e.timeStamp, speed: 0 }
    setUser((s) => [...s, { points: [toStage(e)], done: false }])
  }
  const onPointerMove = (e: React.PointerEvent) => {
    const p = pen.current
    if (p) {
      const dt = Math.max(1, e.timeStamp - p.at)
      const v = Math.hypot(e.clientX - p.x, e.clientY - p.y) / scale / dt
      p.speed += (v - p.speed) * 0.35
      pen.current = { x: e.clientX, y: e.clientY, at: e.timeStamp, speed: p.speed }
      sfx.scribble("user", "marker", p.speed / 0.9)
    }
    setUser((s) => {
      const last = s[s.length - 1]
      if (!last || last.done) return s
      return [...s.slice(0, -1), { ...last, points: [...last.points, toStage(e)] }]
    })
  }
  const onPointerUp = () => {
    if (pen.current) sfx.scribbleStop("user")
    pen.current = null
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
      click("press")
      if (playing) setPlaying(false)
      else start()
    } else if (e.key === "z" && (e.metaKey || e.ctrlKey)) {
      e.preventDefault()
      setUser((s) => s.slice(0, -1))
    }
  }

  const cursor = script ? cursorAt(script, t) : null
  const live = mode === "iphone" ? phoneLive(t) : 0
  const refs = { badge: badgeRef, actions: actionsRef, title: titleRef }
  const drawing = shiftDown || drawMode
  const cues = useMemo(() => sfx.demoCues(mode), [mode])
  const last = useRef<{ t: number; at: [number, number] | null; speed: number }>({ t: 0, at: null, speed: 0 })

  // Scripted cues fire as the clock crosses them; seeks and scrubs (large or backward jumps) stay quiet.
  useEffect(() => {
    const prev = last.current
    const at = cursor?.at ?? null
    const dt = t - prev.t
    const stepping = playing && sound && dt > 0 && dt < 250
    if (stepping) for (const c of cues) if (c.t > prev.t && c.t <= t) sfx.play(c.sound, c.volume)
    const texture: sfx.Texture | null = !stepping || !cursor ? null : cursor.drawing ? "marker" : mode === "mac" && t > T.dragStart && t < T.dragEnd ? "glass" : null
    const v = texture && at && prev.at ? Math.hypot(at[0] - prev.at[0], at[1] - prev.at[1]) / dt : 0
    const speed = texture ? prev.speed + (v - prev.speed) * 0.4 : 0
    if (texture) sfx.scribble("demo", texture, speed / (texture === "marker" ? 0.45 : 1.4))
    else sfx.scribbleStop("demo")
    last.current = { t, at, speed }
  }, [t, playing, sound])

  const ended = started && !playing && t >= T.end
  const other: Mode = mode === "mac" ? "iphone" : "mac"

  const overTerminal =
    !!cursor && cursor.at[0] > AGENT.x && cursor.at[0] < AGENT.x + AGENT.w && cursor.at[1] > AGENT.y + 28 && cursor.at[1] < AGENT.y + AGENT.h

  return (
    <div className="demo">
      <ModeSwitch mode={mode} onMode={switchMode} />
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
          {mode === "mac" ? (
            <MacWindow fix={fix} settle={settle} refs={refs} inactive={terminalActive(t)} />
          ) : (
            <PhoneMirror fix={fix} settle={settle} refs={refs} live={live} />
          )}
          {script && <Terminal t={t} mode={mode} script={script} />}
          {script && <Selection t={t} script={script} cursor={cursor} />}
          {script && <Ink t={t} script={script} />}
          {script && <Caption t={t} script={script} />}
          <Keys t={t} mode={mode} />
          <Island t={t} mode={mode} />
          {script && <Ripples t={t} script={script} />}
          <svg className="ink" viewBox={`0 0 ${STAGE.w} ${STAGE.h}`} style={INK_SHADOW}>
            {user.map((s, i) => (
              <InkPath key={i} d={inkPath(s.points)} />
            ))}
          </svg>
          {cursor && <Cursor c={cursor} crosshair={mode === "mac" && t >= T.select && t < T.record} ibeam={overTerminal && t >= T.promptClick - 400} />}
        </div>
        {!started && (
          <button className="stage-play" onPointerDown={(e) => e.stopPropagation()} onClick={() => start(true)}>
            <svg viewBox="0 0 16 16" aria-hidden>
              <path d="M4.5 2.8v10.4a.6.6 0 00.9.5l8.3-5.2a.6.6 0 000-1L5.4 2.3a.6.6 0 00-.9.5z" />
            </svg>
            Play with sound
          </button>
        )}
        {ended && (
          <div className="stage-end" onPointerDown={(e) => e.stopPropagation()}>
            <button className="stage-btn" onClick={replay}>
              <ReplayIcon />
              Replay
            </button>
            <button className="stage-btn primary" onClick={() => switchMode(other)}>
              {other === "iphone" ? <PhoneIcon /> : <MacIcon />}
              {other === "iphone" ? "Watch iPhone Mirror" : "Watch Mac Window"}
              <span aria-hidden>→</span>
            </button>
          </div>
        )}
        <audio ref={audioRef} src={NARRATION[mode].src} preload="auto" />
      </div>
      <Controls
        t={t}
        playing={playing}
        drawMode={drawMode}
        sound={sound}
        onToggle={() => {
          click("press")
          if (playing) setPlaying(false)
          else start()
        }}
        onSound={() => {
          unlock()
          if (!sound) {
            sfx.setEnabled(true)
            sfx.play("toggle", 0.8)
          }
          setSound((v) => !v)
        }}
        onReplay={replay}
        onSeek={seek}
        onBeat={(bt) => {
          click("tick")
          seek(bt)
          start()
        }}
        onDraw={() => {
          click("toggle")
          setDrawMode((d) => !d)
        }}
        onPause={() => setPlaying(false)}
      />
    </div>
  )
}

function Controls(props: {
  t: number
  playing: boolean
  drawMode: boolean
  sound: boolean
  onToggle: () => void
  onSound: () => void
  onReplay: () => void
  onSeek: (t: number) => void
  onBeat: (t: number) => void
  onDraw: () => void
  onPause: () => void
}) {
  const { t, playing, drawMode, sound } = props
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
          <ReplayIcon />
        </button>
        <button className={`sound-btn${sound ? " on" : ""}`} onClick={props.onSound} aria-pressed={sound}>
          <svg viewBox="0 0 16 16" aria-hidden>
            <path d="M2.5 6h2.2L8 3.2v9.6L4.7 10H2.5z" fill="currentColor" />
            {sound ? (
              <path d="M10.5 5.5a3.5 3.5 0 010 5M12.3 3.7a6 6 0 010 8.6" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
            ) : (
              <path d="M10.5 6l4 4M14.5 6l-4 4" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
            )}
          </svg>
          {sound ? "Sound" : "Muted"}
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
        <button className={`draw-btn${drawMode ? " on" : ""}`} onClick={props.onDraw} title="Draw (or hold ⇧ and drag)">
          <kbd>⇧</kbd> Draw
        </button>
      </div>
    </div>
  )
}

const ReplayIcon = () => (
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" aria-hidden>
    <path d="M3 8a5 5 0 105-5H5.5" />
    <path d="M7 1L5 3l2 2" />
  </svg>
)

const MacIcon = () => (
  <svg viewBox="0 0 18 16" fill="none" stroke="currentColor" strokeWidth="1.4" strokeLinejoin="round" aria-hidden>
    <rect x="1.5" y="1.5" width="15" height="10" rx="2" />
    <path d="M6.5 14.5h5M9 11.5v3" strokeLinecap="round" />
  </svg>
)

const PhoneIcon = () => (
  <svg viewBox="0 0 12 18" fill="none" stroke="currentColor" strokeWidth="1.4" aria-hidden>
    <rect x="1.5" y="1" width="9" height="16" rx="2.4" />
    <path d="M4.6 3.2h2.8" strokeLinecap="round" strokeWidth="1.6" />
  </svg>
)

function ModeSwitch({ mode, onMode }: { mode: Mode; onMode: (m: Mode) => void }) {
  return (
    <div className="mode-switch" role="tablist" aria-label="Capture source">
      <span className={`mode-thumb ${mode}`} aria-hidden />
      <button role="tab" aria-selected={mode === "mac"} className={mode === "mac" ? "on" : ""} onClick={() => onMode("mac")}>
        <MacIcon />
        Mac Window
      </button>
      <button role="tab" aria-selected={mode === "iphone"} className={mode === "iphone" ? "on" : ""} onClick={() => onMode("iphone")}>
        <PhoneIcon />
        iPhone Mirror
      </button>
    </div>
  )
}
