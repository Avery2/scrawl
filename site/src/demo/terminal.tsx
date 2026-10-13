import { type ReactNode, useLayoutEffect, useRef, useState } from "react"
import { TrafficLights } from "./apps"
import { AGENT, type Mode, type Script, summaryText, T } from "./script"
import { ContactSheet, terminalActive } from "./stage-parts"

// OpenCode's TUI (packages/tui) inside Ghostty, using the v2 "opencode" dark theme tokens:
// background #0a0a0a / raised #141414 / prompt #1e1e1e, text #eeeeee / muted #808080,
// Build agent blue #5c9cf5, paste extmarks on warning #f5a742.

const LOGO_LEFT = ["                   ", "█▀▀█ █▀▀█ █▀▀█ █▀▀▄", "█__█ █__█ █^^^ █__█", "▀▀▀▀ █▀▀▀ ▀▀▀▀ ▀~~▀"]
const LOGO_RIGHT = ["             ▄     ", "█▀▀▀ █▀▀█ █▀▀█ █▀▀█", "█___ █__█ █__█ █^^^", "▀▀▀▀ ▀▀▀▀ ▀▀▀▀ ▀▀▀▀"]

// Ghostty draws block elements as exact cell sprites, so render them as cells rather than font glyphs.
const CELL: Record<string, string> = { "█": "full", "▀": "top", "▄": "bottom", _: "sh", "^": "top-sh", "~": "sh-top" }

function LogoLine({ line, cls }: { line: string; cls: string }) {
  return (
    <span className={`lg ${cls}`}>
      {Array.from(line).map((c, i) => (
        <i key={i} className={CELL[c] ?? ""} />
      ))}
    </span>
  )
}

function Logo() {
  return (
    <div className="tui-logo" aria-label="opencode">
      {LOGO_LEFT.map((l, i) => (
        <div key={i}>
          <LogoLine line={l} cls="lg-open" />
          <i />
          <LogoLine line={LOGO_RIGHT[i]} cls="lg-code" />
        </div>
      ))}
    </div>
  )
}

function Cursor({ focused }: { focused: boolean }) {
  return <i className={`tcur${focused ? " on" : ""}`} />
}

// createFrames({ style: "blocks" }) from packages/tui/src/ui/spinner.ts: an 8-cell bidirectional
// scanner at 40ms per frame, holding 9 frames at the end and 30 at the start.
const TRAIL = [1, 0.82, 0.66, 0.52, 0.4, 0.32]

function Spinner({ t }: { t: number }) {
  const f = Math.floor(t / 40) % 54
  const forward = f < 8 || (f >= 24 && f < 54)
  const pos = f < 8 ? f : f < 17 ? 7 : f < 24 ? 6 - (f - 17) : 0
  const held = f >= 8 && f < 17 ? f - 7 : f >= 24 ? f - 23 : 0
  return (
    <span className="tspin" aria-hidden>
      {Array.from({ length: 8 }, (_, i) => {
        const d = forward ? pos - i : i - pos
        const trail = d >= 0 && d < TRAIL.length && d < (held ? Math.max(1, TRAIL.length - held) : TRAIL.length)
        return (
          <i key={i} style={{ opacity: trail ? TRAIL[d] : 0.36 }}>
            {trail ? "■" : "⬝"}
          </i>
        )
      })}
    </span>
  )
}

const BRAILLE = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]

function Braille({ t }: { t: number }) {
  return <span className="tbraille">{BRAILLE[Math.floor(t / 80) % BRAILLE.length]}</span>
}

function Prompt({ t, mode, focused, running }: { t: number; mode: Mode; focused: boolean; running: boolean }) {
  const image = t >= T.paste && t < T.submit
  const text = t >= T.paste + 160 && t < T.submit
  return (
    <div className="tp">
      <div className="tp-box">
        <span className="tp-bar" />
        <div className="tp-inner">
          <div className="tp-text">
            {image ? (
              <>
                <span className="tx-paste">[Image 1]</span>{" "}
                {text && summaryText(mode, true).join("\n")}
                <Cursor focused={focused} />
              </>
            ) : (
              <span className="tp-ph">
                <Cursor focused={focused} />
                {t >= T.submit ? "" : "Ask anything… \"Fix a TODO in the codebase\""}
              </span>
            )}
          </div>
          <div className="tp-meta">
            <span className="tx-build">Build</span> <span className="tx-mute">·</span> Claude Opus 4.6 <span className="tx-mute">anthropic</span>
          </div>
        </div>
      </div>
      <div className="tp-foot-edge">
        <span className="tp-cap" />
        <span className="tp-half" />
      </div>
      <div className="tp-footer">
        {running ? (
          <span className="tp-status">
            <Spinner t={t} />
            <span>
              esc <span className="tx-mute">interrupt</span>
            </span>
          </span>
        ) : (
          <span className="tx-mute">~/code/pulse:main</span>
        )}
        <span className="tp-keys">
          {t >= T.done ? (
            <span className="tx-mute">18.4K (9%) · $0.11</span>
          ) : t < T.submit ? (
            <span>
              tab <span className="tx-mute">agents</span>
            </span>
          ) : null}
          <span>
            ctrl+p <span className="tx-mute">commands</span>
          </span>
        </span>
      </div>
    </div>
  )
}

type Tok = [string, string?]
const tsx = (src: string): Tok[] => {
  const out: Tok[] = []
  const re = /(<\/?|\/?>)|([A-Za-z][\w.]*)(?==)|("[^"]*")|(=)|(\b[A-Z]\w*\b)|(\b[a-z]\w*\b(?=[\s>]))|([^<>="A-Za-z]+|[A-Za-z]+)/g
  let m: RegExpExecArray | null
  let afterOpen = false
  while ((m = re.exec(src))) {
    if (m[1]) {
      out.push([m[1], "sx-op"])
      afterOpen = m[1].startsWith("<")
      continue
    }
    if (m[2]) out.push([m[2], "sx-attr"])
    else if (m[3]) out.push([m[3], "sx-str"])
    else if (m[4]) out.push([m[4], "sx-op"])
    else if (afterOpen && (m[5] || m[6])) out.push([m[5] || m[6], "sx-tag"])
    else out.push([m[0]])
    afterOpen = false
  }
  return out
}

const DIFF: [number, " " | "-" | "+", string][] = [
  [13, " ", `<h3 className="title">Deploy Activity</h3>`],
  [14, "-", `<Badge className="absolute -left-14 -top-2">`],
  [14, "+", `<Badge className="ml-2.5">`],
  [31, "-", `<ul className="space-y-0 py-px">`],
  [31, "+", `<ul className="divide-y [&>li]:py-2.5">`],
  [44, "-", `<div className="h-8 overflow-hidden pt-3.5">`],
  [44, "+", `<div className="flex h-8 justify-end gap-2">`],
]

function Diff() {
  return (
    <div className="tdiff">
      {DIFF.map(([n, sign, code], i) => (
        <div key={i} className={`td-line${sign === "+" ? " add" : sign === "-" ? " del" : ""}`}>
          <span className="td-num">{n}</span>
          <span className="td-sign">{sign === " " ? " " : sign}</span>
          <span className="td-code">
            {tsx(code).map(([s, c], j) =>
              c ? (
                <span key={j} className={c}>
                  {s}
                </span>
              ) : (
                s
              ),
            )}
          </span>
        </div>
      ))}
    </div>
  )
}

function Session({ t, mode, script, children }: { t: number; mode: Mode; script: Script; children: ReactNode }) {
  const summary = summaryText(mode, true)
  const viewRef = useRef<HTMLDivElement>(null)
  const contentRef = useRef<HTMLDivElement>(null)
  const [offset, setOffset] = useState(0)
  const step = t >= T.done ? 4 : t >= T.diff ? 3 : t >= T.edit ? 2 : t >= T.read ? 1 : 0

  // Sticky-bottom scrollbox: content starts at the top and scrolls once it overflows.
  useLayoutEffect(() => {
    const view = viewRef.current
    const content = contentRef.current
    if (!view || !content) return
    setOffset(Math.min(0, view.clientHeight - content.scrollHeight))
  }, [step])

  return (
    <div className="tui-session">
      <div ref={viewRef} className="ts-view">
        <div ref={contentRef} className="ts-content" style={{ transform: `translateY(${offset}px)` }}>
          <div className="tu-msg">
            <span className="tu-bar" />
            <div className="tu-inner">
              <div className="tu-images">
                <ContactSheet mode={mode} script={script} width={300} height={8 * 15} />
              </div>
              <div className="tu-text">
                <div className="tu-body">{`[Image 1] ${summary.join("\n")}`}</div>
                <div className="tu-files">
                  <span className="tu-chip"> file </span>
                  <span className="tu-name"> contact-sheet.png </span>
                </div>
              </div>
            </div>
          </div>
          {step >= 1 && (
            <div className="tt-inline">
              <span className="tt-icon">→</span>Read src/components/HeaderCard.tsx
            </div>
          )}
          {step >= 2 && (
            <div className="tt-block">
              <span className="tt-bar" />
              <div className="tt-inner">
                <div className="tt-head">
                  {step === 2 ? (
                    <>
                      <Braille t={t} />
                      Preparing edit…
                    </>
                  ) : (
                    <>← Edit src/components/HeaderCard.tsx</>
                  )}
                </div>
                {step >= 3 && <Diff />}
              </div>
            </div>
          )}
          {step >= 4 && (
            <>
              <div className="ta-text">
                The <span className="tx-code">Live</span> badge now sits in the title row's flex flow, rows are back to{" "}
                <span className="tx-code">py-2.5</span>, and the action row lost its stray 14px top padding.
              </div>
              <div className="ta-foot">
                <span className="tx-build">Build</span>
                <span className="tx-mute"> · Claude Opus 4.6 · 4.1s</span>
              </div>
            </>
          )}
        </div>
      </div>
      <div className="ts-composer">{children}</div>
    </div>
  )
}

function Home({ children }: { children: ReactNode }) {
  return (
    <div className="tui-home">
      <div className="th-grow" />
      <div className="th-gap3" />
      <Logo />
      <div className="th-gap1" />
      <div className="th-prompt">{children}</div>
      <div className="th-grow" />
      <div className="th-footer">
        <span>
          <span className="tx-ok">⊙</span> 2 MCP <span className="tx-mute">/mcps</span>
        </span>
        <span className="tx-mute">2.0.0</span>
      </div>
    </div>
  )
}

export function Terminal({ t, mode, script }: { t: number; mode: Mode; script: Script }) {
  const active = terminalActive(t)
  const submitted = t >= T.submit
  const running = submitted && t < T.done
  const title = t >= T.read ? "OC | Fix header badge and button row" : "OpenCode"
  const prompt = <Prompt t={t} mode={mode} focused={active && !running} running={running} />
  return (
    <div className={`term${active ? "" : " inactive"}`} style={{ left: AGENT.x, top: AGENT.y, width: AGENT.w, height: AGENT.h }}>
      <div className="term-bar">
        <TrafficLights inactive={!active} />
        <span className="term-title">{title}</span>
      </div>
      <div className="tui">
        {submitted ? (
          <Session t={t} mode={mode} script={script}>
            {prompt}
          </Session>
        ) : (
          <Home>{prompt}</Home>
        )}
      </div>
    </div>
  )
}
