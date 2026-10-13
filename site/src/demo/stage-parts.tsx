import {
  AGENT,
  AREA,
  CLAUSE_2,
  clock,
  easeInOut,
  easeOut,
  easeOutBack,
  type Mode,
  PHONE,
  PILL,
  pillState,
  pillWidth,
  lerp,
  progress,
  type Rect,
  type Script,
  STAGE,
  strokeAt,
  T,
  WORDS,
  type Cursor as CursorState,
  MAC_WINDOW,
} from "./script"
import { INK_SHADOW, InkPath, inkPath, PinMark } from "./ink"
import { MacWindow, PhoneMirror } from "./apps"

const pressedAt = (t: number, at: number) => t >= at - 60 && t < at + 160

// Ghostty becomes the key window once the cursor clicks into the OpenCode prompt.
export const terminalActive = (t: number) => t >= T.promptClick

const APPLE =
  "M11.2 4.3c.6-.8 1-1.8.9-2.8-.9 0-2 .6-2.6 1.4-.6.7-1.1 1.7-.9 2.7 1 .1 2-.5 2.6-1.3zM12.1 5.8c-1.4-.1-2.7.8-3.3.8-.7 0-1.7-.8-2.9-.8C4.4 5.9 3 6.8 2.2 8.1c-1.6 2.7-.4 6.8 1.1 9 .8 1.1 1.7 2.3 2.8 2.3 1.1 0 1.6-.7 2.9-.7 1.4 0 1.8.7 2.9.7 1.2 0 2-1.1 2.7-2.2.9-1.3 1.2-2.5 1.2-2.6 0 0-2.4-.9-2.4-3.6 0-2.2 1.8-3.3 1.9-3.4-1-1.5-2.6-1.7-3.2-1.7z"

export function MenuBar({ t }: { t: number }) {
  const app = terminalActive(t) ? "Ghostty" : "Pulse"
  return (
    <div className="menubar">
      <div className="menubar-left">
        <svg className="apple" viewBox="0 0 17 21" aria-hidden>
          <path d={APPLE} />
        </svg>
        <b>{app}</b>
        <span>File</span>
        <span>Edit</span>
        <span>View</span>
        <span>Window</span>
        <span>Help</span>
      </div>
      <div className="menubar-right">
        <span className="menubar-extra menubar-scrawl">
          <img src="/menubar-icon@2x.png" alt="" />
        </span>
        <span className="menubar-extra">
          <span className="battery">
            <i />
          </span>
        </span>
        <span className="menubar-extra">
          <svg viewBox="0 0 20 14" className="wifi" aria-hidden>
            <path d="M10 13.2l2.3-2.6a3.4 3.4 0 00-4.6 0zM4.9 7.5l1.5 1.7a5.4 5.4 0 017.2 0l1.5-1.7a7.6 7.6 0 00-10.2 0zM1.6 4.1l1.5 1.7a10 10 0 0113.8 0l1.5-1.7a12.2 12.2 0 00-16.8 0z" />
          </svg>
        </span>
        <span className="menubar-extra">
          <svg viewBox="0 0 16 16" className="spot" aria-hidden>
            <circle cx="6.6" cy="6.6" r="4.6" fill="none" stroke="currentColor" strokeWidth="1.7" />
            <path d="M10 10l4 4" stroke="currentColor" strokeWidth="1.9" strokeLinecap="round" />
          </svg>
        </span>
        <span className="menubar-extra">
          <svg viewBox="0 0 18 16" className="cc" aria-hidden>
            <rect x="1" y="1.5" width="16" height="5.6" rx="2.8" fill="none" stroke="currentColor" strokeWidth="1.3" />
            <circle cx="4.4" cy="4.3" r="1.7" />
            <rect x="1" y="8.9" width="16" height="5.6" rx="2.8" />
            <circle cx="13.6" cy="11.7" r="1.7" fill="#2b2b33" />
          </svg>
        </span>
        <span className="menubar-clock">Tue Oct 13&nbsp;&nbsp;9:41 AM</span>
      </div>
    </div>
  )
}

function Chip({ w, children, on, primary, dim, pressed, round }: { w: number; children: React.ReactNode; on?: boolean; primary?: boolean; dim?: boolean; pressed?: boolean; round?: boolean }) {
  const cls = ["chip", on && "on", primary && "primary", dim && "dim", pressed && "pressed", round && "round"].filter(Boolean).join(" ")
  return (
    <span className={cls} style={{ width: w }}>
      {children}
    </span>
  )
}

const Divider = () => <span className="island-divider" />

export function Island({ t, mode }: { t: number; mode: Mode }) {
  const state = pillState(t)
  const phoneOn = mode === "iphone" && t >= T.phoneClick
  const items = state === "notch" ? PILL.items.select : state === "select" && phoneOn ? PILL.items.selectPhone : PILL.items[state]
  const gone = state === "notch" || t >= T.paste
  const width = pillWidth(items)
  const [lead, , c2, c3, c4, c5] = items
  return (
    <div
      className={`island ${state}${gone ? " gone" : ""}`}
      style={{ left: PILL.cx, top: PILL.top, width, height: PILL.h }}
      aria-hidden={gone}
    >
      <div className="island-content" key={state === "select" && phoneOn ? "select-phone" : state}>
        {state === "select" && (
          <>
            <Chip w={lead} on={!phoneOn}>
              Area
            </Chip>
            <Chip w={items[1]}>Full Screen</Chip>
            <Chip w={c2} on={phoneOn} pressed={mode === "iphone" && pressedAt(t, T.phoneClick)}>
              iPhone
            </Chip>
            {phoneOn && (
              <Chip w={c3} primary pressed={pressedAt(t, T.phoneRecord)}>
                Record
              </Chip>
            )}
            <Divider />
            <Chip w={phoneOn ? c5 : c4} round>
              <svg viewBox="0 0 10 10" aria-hidden>
                <path d="M2 2l6 6M8 2L2 8" />
              </svg>
            </Chip>
          </>
        )}
        {state === "recording" && (
          <>
            <span className="island-lead" style={{ width: lead }}>
              <span className="rec-slot">
                <i className="rec-halo" />
                <i className="rec-dot" />
              </span>
              <b>{clock(t - T.record)}</b>
            </span>
            <Divider />
            <Chip w={c2} dim={t < T.circle[1]}>
              Undo
            </Chip>
            <Chip w={c3}>Cancel</Chip>
            <Chip w={c4} primary pressed={pressedAt(t, T.finish)}>
              Finish
            </Chip>
          </>
        )}
        {state === "processing" && (
          <span className="island-lead" style={{ width: lead }}>
            <span className="orbit">
              {Array.from({ length: 8 }, (_, i) => (
                <i key={i} style={{ transform: `rotate(${i * 45}deg) translateY(-7.2px)`, animationDelay: `${(i / 8) * 0.476 - 0.476}s` }} />
              ))}
            </span>
            <b>Processing…</b>
          </span>
        )}
        {state === "copied" && (
          <>
            <span className="island-lead" style={{ width: lead }}>
              <span className="check">
                <svg viewBox="0 0 12 12" aria-hidden>
                  <path d="M2.6 6.3l2.2 2.2 4.6-4.9" />
                </svg>
              </span>
              <b>Copied</b>
            </span>
            <Divider />
            <Chip w={c2} primary>
              Paste
            </Chip>
            <Chip w={c3}>Inspect</Chip>
            <Chip w={c4} round>
              <svg viewBox="0 0 10 10" aria-hidden>
                <path d="M2 2l6 6M8 2L2 8" />
              </svg>
            </Chip>
          </>
        )}
      </div>
    </div>
  )
}

export function Selection({ t, script, cursor }: { t: number; script: Script; cursor: CursorState | null }) {
  if (!script.drag || t < T.select || t > T.finish + 300) return null
  let rect: Rect | null = null
  const [fx, fy] = script.drag.from
  const dragging = t >= T.dragStart && t < T.dragEnd
  if (dragging && cursor) {
    // The corner is the cursor tip, so the box can never lead or lag the pointer.
    rect = { x: fx, y: fy, w: Math.max(0, cursor.at[0] - fx), h: Math.max(0, cursor.at[1] - fy) }
  } else if (t >= T.dragEnd) {
    rect = { x: fx, y: fy, w: AREA.w, h: AREA.h }
  }
  // Matches laser_overlay.metal: 0.32 picker dim easing to 0.22 while recording, 1px border 0.92 → 0.28.
  const intro = easeOut(progress(t, T.record, T.record + 240))
  const dim = 0.32 + (0.22 - 0.32) * intro
  const border = 0.92 + (0.28 - 0.92) * intro
  const fade = 1 - progress(t, T.finish, T.finish + 300)
  const sizeOpacity = Math.min(progress(t, T.dragStart, T.dragStart + 120), 1 - progress(t, T.dragEnd, T.dragEnd + 220))
  return (
    <div className="selection" style={{ opacity: fade }}>
      {rect ? (
        <>
          <div
            className="selection-rect"
            style={{
              left: rect.x,
              top: rect.y,
              width: rect.w,
              height: rect.h,
              boxShadow: `0 0 0 4000px rgba(0,0,0,${dim}), inset 0 0 0 1px rgba(255,255,255,${border})`,
            }}
          />
          {sizeOpacity > 0 && (
            <span className="selection-size" style={{ left: rect.x + rect.w, top: rect.y + rect.h, opacity: sizeOpacity }}>
              {Math.round(rect.w)} × {Math.round(rect.h)}
            </span>
          )}
        </>
      ) : (
        <div className="selection-dim" />
      )}
    </div>
  )
}

export function Ink({ t, script }: { t: number; script: Script }) {
  if (t < T.circle[0]) return null
  const fade = 1 - progress(t, T.finish, T.finish + 280)
  if (fade <= 0) return null
  return (
    <svg className="ink" viewBox={`0 0 ${STAGE.w} ${STAGE.h}`} style={{ opacity: fade, ...INK_SHADOW }}>
      {script.strokes.map((s) => {
        const live = strokeAt(s, t)
        return live ? <InkPath key={s.id} d={inkPath(live.points)} /> : null
      })}
      {script.pins.map((p) =>
        t >= p.t ? <PinMark key={p.n} n={p.n} at={p.at} scale={Math.max(0, easeOutBack(progress(t, p.t, p.t + 260)))} /> : null,
      )}
    </svg>
  )
}

export function Ripples({ t, script }: { t: number; script: Script }) {
  return (
    <svg className="ink" viewBox={`0 0 ${STAGE.w} ${STAGE.h}`}>
      {script.clicks.map((c, i) => {
        const p = progress(t, c.t, c.t + 650)
        if (p <= 0 || p >= 1) return null
        return <circle key={i} cx={c.at[0]} cy={c.at[1]} r={6 + 22 * easeOut(p)} fill="none" stroke="#fff" strokeWidth={2} opacity={0.7 * (1 - p)} />
      })}
    </svg>
  )
}

export function Cursor({ c, crosshair, ibeam }: { c: CursorState; crosshair?: boolean; ibeam?: boolean }) {
  const [x, y] = c.at
  return (
    <div className="cursor" style={{ transform: `translate(${x}px, ${y}px)` }}>
      {crosshair && !c.shift ? (
        <svg width="25" height="25" viewBox="-12.5 -12.5 25 25" style={{ transform: "translate(-12.5px, -12.5px)" }}>
          <path d="M0 -11V11M-11 0H11" stroke="#fff" strokeWidth="3" strokeLinecap="square" />
          <path d="M0 -10V10M-10 0H10" stroke="#000" strokeWidth="1" shapeRendering="crispEdges" />
        </svg>
      ) : c.shift ? (
        <svg width="26" height="26" viewBox="0 0 26 26" style={{ transform: `translate(-2px, -24px) scale(${c.pressed ? 0.94 : 1})` }}>
          <path d="M3 23l2-7L18 3l5 5L10 21z" fill="#f2f2f2" stroke="#111" strokeWidth="1.3" strokeLinejoin="round" />
          <path d="M3 23l2-7 5 5z" fill="#ff3b30" stroke="#111" strokeWidth="1.3" strokeLinejoin="round" />
          <path d="M15.5 5.5l5 5" stroke="#111" strokeWidth="1.3" />
        </svg>
      ) : ibeam ? (
        <svg width="10" height="20" viewBox="0 0 10 20" style={{ transform: "translate(-5px, -10px)" }}>
          <path d="M1.5 1.5c2 0 3.5.6 3.5 2v13c0 1.4-1.5 2-3.5 2M8.5 1.5c-2 0-3.5.6-3.5 2v13c0 1.4 1.5 2 3.5 2M3 10h4" fill="none" stroke="#fff" strokeWidth="3" strokeLinecap="round" />
          <path d="M1.5 1.5c2 0 3.5.6 3.5 2v13c0 1.4-1.5 2-3.5 2M8.5 1.5c-2 0-3.5.6-3.5 2v13c0 1.4 1.5 2 3.5 2M3 10h4" fill="none" stroke="#000" strokeWidth="1.1" strokeLinecap="round" />
        </svg>
      ) : (
        <svg width="22" height="28" viewBox="0 0 22 28" style={{ transform: `translate(-2px, -2px) scale(${c.pressed ? 0.88 : 1})`, transformOrigin: "2px 2px" }}>
          <path d="M2 2v20.5l5.2-5 3.4 8 3.6-1.5-3.4-7.8h7.3z" fill="#000" stroke="#fff" strokeWidth="1.6" strokeLinejoin="round" />
        </svg>
      )}
      {c.shift && <span className="cursor-key">⇧</span>}
    </div>
  )
}

const KEYS = ["⌥", "⇧", "S"]

export function Keys({ t, mode }: { t: number; mode: Mode }) {
  const cx = mode === "mac" ? MAC_WINDOW.x + MAC_WINDOW.w / 2 : PHONE.x + PHONE.w / 2
  const shortcut = t >= T.keys - 150 && t < T.keysUp + 350
  const paste = t >= T.paste - 400 && t < T.paste + 500
  if (shortcut) {
    const out = progress(t, T.keysUp, T.keysUp + 350)
    return (
      <div className="keys" style={{ left: cx, top: 646, opacity: (1 - out) * Math.min(1, progress(t, T.keys - 150, T.keys)) }}>
        {KEYS.map((k, i) => (
          <kbd key={k} className={t >= T.keys + i * 150 && t < T.keysUp ? "down" : ""}>
            {k}
          </kbd>
        ))}
      </div>
    )
  }
  if (paste) {
    const out = progress(t, T.paste + 250, T.paste + 500)
    return (
      <div className="keys small" style={{ left: AGENT.x + AGENT.w / 2, top: AGENT.y + AGENT.h - 150, opacity: (1 - out) * Math.min(1, progress(t, T.paste - 400, T.paste - 250)) }}>
        <kbd className={t >= T.paste - 300 && t < T.paste + 250 ? "down" : ""}>⌥</kbd>
        <kbd className={t >= T.paste - 150 && t < T.paste + 250 ? "down" : ""}>⇧</kbd>
        <kbd className={t >= T.paste && t < T.paste + 200 ? "down" : ""}>V</kbd>
      </div>
    )
  }
  return null
}

// The spoken words follow the pen: each clause's pill sits beside the mark it describes.
export function Caption({ t, script }: { t: number; script: Script }) {
  const first = WORDS[0][1]
  if (t < first - 300 || t > T.finish + 400) return null
  const swap = WORDS[CLAUSE_2][1] - 20
  const second = t >= swap
  const words = second ? WORDS.slice(CLAUSE_2) : WORDS.slice(0, CLAUSE_2)
  const opacity = Math.min(
    progress(t, first - 300, first),
    1 - progress(t, T.finish, T.finish + 400),
    second ? 0.25 + 0.75 * progress(t, swap, swap + 140) : 1 - 0.75 * progress(t, swap - 140, swap),
  )
  const glide = easeInOut(progress(t, swap - 100, swap + 520))
  const [a, b] = script.speech
  const pos = (i: 0 | 1) => lerp(a[i], b[i], glide)
  const now = words.findIndex(([, at, end]) => t >= at && t < end + 120)
  const talking = now >= 0
  return (
    <div className="caption" style={{ left: pos(0), top: pos(1), opacity }}>
      <span className="wave">
        {[0, 1, 2, 3, 4].map((i) => {
          const amp = talking ? 0.35 + 0.65 * Math.abs(Math.sin(t / (90 + i * 23) + i * 1.7)) : 0.16
          return <i key={i} style={{ transform: `scaleY(${amp})` }} />
        })}
      </span>
      <p>
        {words.map(([w, at], i) => (
          <span key={i} className={at > t ? "w pending" : i === now ? "w now" : "w said"}>
            {w.replace("—", "\u2060—\u2060")}{" "}
          </span>
        ))}
      </p>
    </div>
  )
}

export const captureArea = (mode: Mode): Rect => (mode === "mac" ? AREA : { x: PHONE.x - 6, y: PHONE.y - 6, w: PHONE.w + 12, h: PHONE.h + 12 })

export function KeyframeScene({ mode, script, frame, scale }: { mode: Mode; script: Script; frame: Script["frames"][number]; scale: number }) {
  const area = captureArea(mode)
  return (
    <div className="sheet-scene wallpaper" style={{ width: STAGE.w, height: STAGE.h, transform: `scale(${scale}) translate(${-area.x}px, ${-area.y}px)` }}>
      {mode === "mac" ? <MacWindow fix={0} settle={0} /> : <PhoneMirror fix={0} settle={0} />}
      <svg className="ink" viewBox={`0 0 ${STAGE.w} ${STAGE.h}`} style={INK_SHADOW}>
        {script.strokes
          .filter((st) => frame.strokes.includes(st.id))
          .map((st) => (
            <InkPath key={st.id} d={inkPath(st.points, 10)} />
          ))}
        {script.pins
          .filter((p) => frame.pins.includes(p.n))
          .map((p) => (
            <PinMark key={p.n} n={p.n} at={p.at} scale={1.7} />
          ))}
      </svg>
    </div>
  )
}

// Corner index badge drawn by render.rs: a 22px dark pill with a faint ring, top-right.
export function FrameBadge({ n, k }: { n: number; k: number }) {
  const s = Math.max(k, 0.42)
  return (
    <span className="frame-badge" style={{ height: 22 * s, minWidth: 22 * s, padding: `0 ${6.5 * s}px`, fontSize: 12 * s, top: 6 * s, right: 6 * s }}>
      {n}
    </span>
  )
}

// Same geometry as render_contact_sheet: 640px cells (440 for portrait), 16px padding, 14px gaps,
// 10px card radius with a 1px #2e2e30 rim on a #141414 sheet.
export function ContactSheet({ mode, script, width, height }: { mode: Mode; script: Script; width: number; height?: number }) {
  const area = captureArea(mode)
  const portrait = area.h > area.w
  const cell = portrait ? 440 : 640
  const fw0 = Math.min(cell, (960 * area.w) / area.h)
  const fh0 = (fw0 * area.h) / area.w
  const n = script.frames.length
  const w0 = 32 + cell * n + 14 * (n - 1)
  const h0 = 32 + fh0
  const k = Math.min(width / w0, (height ?? Infinity) / h0)
  return (
    <div className="sheet" style={{ width: w0 * k, height: h0 * k, padding: 16 * k, gap: 14 * k, borderRadius: Math.max(2, 6 * k) }}>
      {script.frames.map((f) => (
        <div key={f.n} className="sheet-frame" style={{ width: cell * k, height: fh0 * k }}>
          <div className="sheet-card" style={{ width: fw0 * k, height: fh0 * k, borderRadius: 10 * k }}>
            <KeyframeScene mode={mode} script={script} frame={f} scale={(fw0 * k) / area.w} />
            <FrameBadge n={f.n} k={k} />
          </div>
        </div>
      ))}
    </div>
  )
}

export function fixAmount(t: number) {
  const p = progress(t, T.fix[0], T.fix[1])
  return { fix: easeOutBack(p), settle: progress(t, T.fix[1] - 150, T.fix[1] + 1100), p }
}

export const phoneLive = (t: number) => Math.min(progress(t, T.phoneRecord, T.phoneRecord + 300), 1 - progress(t, T.finish, T.finish + 300))
