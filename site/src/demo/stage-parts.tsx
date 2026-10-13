import {
  AGENT,
  AREA,
  clock,
  easeOut,
  easeOutBack,
  type Mode,
  PHONE,
  PILL,
  pillState,
  pillWidth,
  progress,
  type Rect,
  type Script,
  SPEECH_END,
  STAGE,
  strokeAt,
  summaryText,
  T,
  WORDS,
  type Cursor as CursorState,
  MAC_WINDOW,
} from "./script"
import { InkPath, inkPath, PinMark } from "./ink"
import { MacWindow, PhoneMirror } from "./apps"

const pressedAt = (t: number, at: number) => t >= at - 60 && t < at + 160

export function MenuBar({ t }: { t: number }) {
  const recording = t >= T.record && t < T.finish
  return (
    <div className="menubar">
      <div className="menubar-left">
        <svg className="apple" viewBox="0 0 17 21" aria-hidden>
          <path d="M11.2 4.3c.6-.8 1-1.8.9-2.8-.9 0-2 .6-2.6 1.4-.6.7-1.1 1.7-.9 2.7 1 .1 2-.5 2.6-1.3zM12.1 5.8c-1.4-.1-2.7.8-3.3.8-.7 0-1.7-.8-2.9-.8C4.4 5.9 3 6.8 2.2 8.1c-1.6 2.7-.4 6.8 1.1 9 .8 1.1 1.7 2.3 2.8 2.3 1.1 0 1.6-.7 2.9-.7 1.4 0 1.8.7 2.9.7 1.2 0 2-1.1 2.7-2.2.9-1.3 1.2-2.5 1.2-2.6 0 0-2.4-.9-2.4-3.6 0-2.2 1.8-3.3 1.9-3.4-1-1.5-2.6-1.7-3.2-1.7z" />
        </svg>
        <b>Pulse</b>
        <span>File</span>
        <span>Edit</span>
        <span>View</span>
        <span>Window</span>
        <span>Help</span>
      </div>
      <div className="menubar-right">
        <span className={`menubar-scrawl${recording ? " rec" : ""}`}>
          <img src="/menubar-icon@2x.png" alt="" />
        </span>
        <svg viewBox="0 0 20 14" className="wifi" aria-hidden>
          <path d="M10 13.2l2.3-2.6a3.4 3.4 0 00-4.6 0zM4.9 7.5l1.5 1.7a5.4 5.4 0 017.2 0l1.5-1.7a7.6 7.6 0 00-10.2 0zM1.6 4.1l1.5 1.7a10 10 0 0113.8 0l1.5-1.7a12.2 12.2 0 00-16.8 0z" />
        </svg>
        <span className="battery">
          <i />
        </span>
        <span>Tue 9:41 AM</span>
      </div>
    </div>
  )
}

function Chip({ w, children, on, primary, dim, pressed }: { w: number; children: React.ReactNode; on?: boolean; primary?: boolean; dim?: boolean; pressed?: boolean }) {
  return (
    <span className={`chip${on ? " on" : ""}${primary ? " primary" : ""}${dim ? " dim" : ""}${pressed ? " pressed" : ""}`} style={{ width: w }}>
      {children}
    </span>
  )
}

export function Island({ t, mode }: { t: number; mode: Mode }) {
  const state = pillState(t)
  const cols = state === "select" || state === "recording" || state === "copied" ? PILL.cols[state] : null
  const notch = state === "notch"
  const width = cols ? pillWidth(cols) : notch ? 184 : 92
  const phoneOn = mode === "iphone" && t >= T.phoneClick
  return (
    <div
      className={`island ${state}`}
      style={{
        left: PILL.cx,
        width,
        top: notch ? 0 : 6,
        height: notch ? 30 : 36,
        borderRadius: notch ? "0 0 13px 13px" : 18,
      }}
    >
      <div className="island-content" key={state}>
        {state === "select" && cols && (
          <>
            <Chip w={cols[0]} on={!phoneOn}>
              Area
            </Chip>
            <Chip w={cols[1]}>Full Screen</Chip>
            <Chip w={cols[2]} on={phoneOn} pressed={mode === "iphone" && pressedAt(t, T.phoneClick)}>
              iPhone
            </Chip>
            <Chip w={cols[3]}>✕</Chip>
          </>
        )}
        {state === "recording" && cols && (
          <>
            <Chip w={cols[0]}>
              <i className="rec-dot" />
              {clock(t - T.record)}
            </Chip>
            <Chip w={cols[1]} dim={t < T.circle[1]}>
              Undo
            </Chip>
            <Chip w={cols[2]}>Cancel</Chip>
            <Chip w={cols[3]} primary pressed={pressedAt(t, T.finish)}>
              Finish
            </Chip>
          </>
        )}
        {state === "processing" && <span className="spinner" />}
        {state === "copied" && cols && (
          <>
            <Chip w={cols[0]}>
              <span className="check">✓</span> Copied
            </Chip>
            <Chip w={cols[1]}>Paste</Chip>
            <Chip w={cols[2]}>Inspect</Chip>
            <Chip w={cols[3]}>✕</Chip>
          </>
        )}
      </div>
    </div>
  )
}

export function Selection({ t, script }: { t: number; script: Script }) {
  if (!script.drag || t < T.select || t > T.finish + 300) return null
  let rect: Rect | null = null
  if (t >= T.dragStart) {
    const k = easeOut(progress(t, T.dragStart, T.dragEnd))
    const [fx, fy] = script.drag.from
    rect = { x: fx, y: fy, w: AREA.w * k, h: AREA.h * k }
  }
  const recording = t >= T.record
  const dim = recording ? 0.16 : 0.5
  const fade = 1 - progress(t, T.finish, T.finish + 300)
  return (
    <div className="selection" style={{ opacity: fade }}>
      {rect ? (
        <div
          className={`selection-rect${recording ? " rec" : ""}`}
          style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h, boxShadow: `0 0 0 4000px rgba(0,0,0,${dim})` }}
        >
        </div>
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
    <svg className="ink" viewBox={`0 0 ${STAGE.w} ${STAGE.h}`} style={{ opacity: fade }}>
      {script.strokes.map((s) => {
        const live = strokeAt(s, t)
        return live ? <InkPath key={s.id} d={inkPath(live.points, live.done)} /> : null
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

export function Cursor({ c }: { c: CursorState }) {
  const [x, y] = c.at
  return (
    <div className="cursor" style={{ transform: `translate(${x}px, ${y}px)` }}>
      {c.shift ? (
        <svg width="26" height="26" viewBox="0 0 26 26" style={{ transform: `translate(-2px, -24px) scale(${c.pressed ? 0.94 : 1})` }}>
          <path d="M3 23l2-7L18 3l5 5L10 21z" fill="#f2f2f2" stroke="#111" strokeWidth="1.3" strokeLinejoin="round" />
          <path d="M3 23l2-7 5 5z" fill="#ff3b30" stroke="#111" strokeWidth="1.3" strokeLinejoin="round" />
          <path d="M15.5 5.5l5 5" stroke="#111" strokeWidth="1.3" />
        </svg>
      ) : (
        <svg width="22" height="28" viewBox="0 0 22 28" style={{ transform: `scale(${c.pressed ? 0.88 : 1})`, transformOrigin: "2px 2px" }}>
          <path d="M2 2v20.5l5.2-5 3.4 8 3.6-1.5-3.4-7.8h7.3z" fill="#000" stroke="#fff" strokeWidth="1.6" strokeLinejoin="round" />
        </svg>
      )}
      {c.shift && <span className="cursor-key">⇧</span>}
    </div>
  )
}

const KEYS = ["⌃", "⌥", "⇧", "S"]

export function Keys({ t, mode }: { t: number; mode: Mode }) {
  const cx = mode === "mac" ? MAC_WINDOW.x + MAC_WINDOW.w / 2 : PHONE.x + PHONE.w / 2
  const shortcut = t >= T.keys - 150 && t < T.keysUp + 350
  const paste = t >= T.paste - 250 && t < T.paste + 450
  if (shortcut) {
    const out = progress(t, T.keysUp, T.keysUp + 350)
    return (
      <div className="keys" style={{ left: cx, top: 640, opacity: (1 - out) * Math.min(1, progress(t, T.keys - 150, T.keys)) }}>
        {KEYS.map((k, i) => (
          <kbd key={k} className={t >= T.keys + i * 150 && t < T.keysUp ? "down" : ""}>
            {k}
          </kbd>
        ))}
      </div>
    )
  }
  if (paste) {
    const out = progress(t, T.paste + 200, T.paste + 450)
    return (
      <div className="keys small" style={{ left: AGENT.x + AGENT.w / 2, top: AGENT.y + AGENT.h - 110, opacity: 1 - out }}>
        <kbd className={t >= T.paste - 150 ? "down" : ""}>⌘</kbd>
        <kbd className={t >= T.paste && t < T.paste + 250 ? "down" : ""}>V</kbd>
      </div>
    )
  }
  return null
}

export function Caption({ t, mode }: { t: number; mode: Mode }) {
  if (t < WORDS[0][1] - 300 || t > T.finish + 400) return null
  const opacity = Math.min(progress(t, WORDS[0][1] - 300, WORDS[0][1]), 1 - progress(t, T.finish, T.finish + 400))
  const spoken = WORDS.filter(([, at]) => at <= t)
  const last = spoken.length - 1
  const talking = t < SPEECH_END && last >= 0 && t - spoken[last][1] < 500
  const cx = mode === "mac" ? MAC_WINDOW.x + MAC_WINDOW.w / 2 : PHONE.x + PHONE.w / 2
  return (
    <div className="caption" style={{ left: cx, top: mode === "mac" ? 628 : 640, opacity }}>
      <span className="wave">
        {[0, 1, 2, 3, 4].map((i) => {
          const amp = talking ? 0.35 + 0.65 * Math.abs(Math.sin(t / (90 + i * 23) + i * 1.7)) : 0.12
          return <i key={i} style={{ transform: `scaleY(${amp})` }} />
        })}
      </span>
      <p>
        {WORDS.map(([w, at], i) => (
          <span key={i} className={at > t ? "w pending" : i === last && talking ? "w now" : "w said"}>
            {w}{" "}
          </span>
        ))}
      </p>
    </div>
  )
}

export function ContactSheet({ mode, script, width }: { mode: Mode; script: Script; width: number }) {
  const area = mode === "mac" ? AREA : { x: PHONE.x - 6, y: PHONE.y - 6, w: PHONE.w + 12, h: PHONE.h + 12 }
  const gap = 3
  const maxW = (width - gap - 6) / 2
  const maxH = mode === "mac" ? 150 : 176
  const s = Math.min(maxW / area.w, maxH / area.h)
  const fw = area.w * s
  const fh = area.h * s
  return (
    <div className="sheet" style={{ gridTemplateColumns: `repeat(2, ${fw}px)`, gap }}>
      {script.frames.map((f) => (
        <div key={f.n} className="sheet-frame" style={{ width: fw, height: fh }}>
          <div
            className="sheet-scene wallpaper"
            style={{ width: STAGE.w, height: STAGE.h, transform: `scale(${s}) translate(${-area.x}px, ${-area.y}px)` }}
          >
            {mode === "mac" ? <MacWindow fix={0} settle={0} /> : <PhoneMirror fix={0} settle={0} />}
            <svg className="ink" viewBox={`0 0 ${STAGE.w} ${STAGE.h}`}>
              {script.strokes
                .filter((st) => f.strokes.includes(st.id))
                .map((st) => (
                  <InkPath key={st.id} d={inkPath(st.points, true, 10)} />
                ))}
              {script.pins
                .filter((p) => f.pins.includes(p.n))
                .map((p) => (
                  <PinMark key={p.n} n={p.n} at={p.at} scale={1.7} />
                ))}
            </svg>
          </div>
          <span className="sheet-badge">
            #{f.n} · {clock(f.t)}
          </span>
        </div>
      ))}
    </div>
  )
}

const DIFF: [string, string][] = [
  ["-", `<Badge className="absolute -left-14 -top-2">`],
  ["+", `<Badge className="ml-2.5">`],
  ["-", `<ul className="space-y-0 py-px">`],
  ["+", `<ul className="divide-y py-2.5">`],
  ["-", `<div className="h-8 overflow-hidden pt-3.5">`],
  ["+", `<div className="flex h-8 justify-end">`],
]

export function Agent({ t, mode, script }: { t: number; mode: Mode; script: Script }) {
  const pasted = t >= T.paste && t < T.submit
  const submitted = t >= T.submit
  const working = submitted && t < T.done
  const summary = summaryText(mode, true)
  return (
    <div className="agent" style={{ left: AGENT.x, top: AGENT.y, width: AGENT.w, height: AGENT.h }}>
      <div className="agent-head">
        <span className="agent-brand">opencode</span>
        <span className="agent-title">{submitted ? "Fix header card alignment" : "New session"}</span>
      </div>
      <div className="agent-body">
        {!submitted && <div className="agent-empty">opencode</div>}
        {submitted && (
          <div className="msg user enter">
            <div className="attach">
              <span>[Image 1]</span> contact-sheet.png
            </div>
            <ContactSheet mode={mode} script={script} width={AGENT.w - 46} />
            <pre className="summary">{summary.join("\n")}</pre>
          </div>
        )}
        {t >= T.read && (
          <div className="tool enter">
            <span className="tool-icon">✱</span> Read <b>src/HeaderCard.tsx</b>
          </div>
        )}
        {t >= T.edit && (
          <div className="tool enter">
            <div>
              <span className="tool-icon">←</span> Edit <b>src/HeaderCard.tsx</b>
            </div>
            <div className="diff">
              {DIFF.map(([sign, code], i) =>
                t >= T.diff + i * 190 ? (
                  <div key={i} className={`diff-line ${sign === "+" ? "add" : "del"} enter`}>
                    <span>{sign}</span>
                    {code}
                  </div>
                ) : null,
              )}
            </div>
          </div>
        )}
        {t >= T.done && <div className="msg assistant enter">Badge sits after the title, the button row is back on its baseline, and rows have 10px padding.</div>}
        {working && (
          <div className="working">
            <i />
            <i />
            <i />
          </div>
        )}
      </div>
      <div className={`agent-prompt${t >= T.promptClick && t < T.submit ? " focus" : ""}`}>
        <span className="caret-mark">›</span>
        {pasted ? (
          <span className="prompt-text">
            <span className="paste-chip">[Image 1]</span> <span className="paste-chip">[Pasted ~3 lines]</span>
            <i className="caret" />
          </span>
        ) : (
          <span className="prompt-placeholder">
            <i className="caret" />
            Ask anything
          </span>
        )}
      </div>
      <div className="agent-foot">
        <span className="agent-mode">Build</span>
        <span>{working ? "esc interrupt" : "enter send"}</span>
      </div>
    </div>
  )
}

export function fixAmount(t: number) {
  const p = progress(t, T.fix[0], T.fix[1])
  return { fix: easeOutBack(p), settle: progress(t, T.fix[1] - 150, T.fix[1] + 1100), p }
}

export const phoneLive = (t: number) => Math.min(progress(t, T.phoneClick + 80, T.phoneClick + 380), 1 - progress(t, T.finish, T.finish + 300))

