import { lazy, Suspense } from "react"
import { BundleExplorer } from "./Bundle"
import { Copy } from "./Copy"
import { Demo } from "./demo/Demo"

const Brush3D = lazy(() => import("./Brush3D").then((m) => ({ default: m.Brush3D })))

const REPO = "https://github.com/kitlangton/scrawl"
const BASE = "https://scrawl.kitlangton.com"
const DMG = `/releases/Scrawl-latest-arm64.dmg`
const ZIP = `${BASE}/releases/Scrawl-latest-arm64.zip`

const INSTALL = `curl -LO ${ZIP}
unzip Scrawl-latest-arm64.zip -d ~/Applications
open ~/Applications/Scrawl.app`

const CONTROLS: [string[], string][] = [
  [["⌥", "⇧", "S"], "Start or finish"],
  [["⇧"], "Hold and drag to draw"],
  [["⇧", "click"], "Drop a numbered pin"],
  [["⌘", "Z"], "Undo"],
  [["esc"], "Cancel"],
  [["⌥", "⇧", "V"], "Paste the latest capture"],
  [["M"], "Flag a moment on iPhone"],
]

function Download() {
  return (
    <div className="download">
      <a className="btn-primary" href={DMG}>
        <svg viewBox="0 0 17 21" aria-hidden>
          <path d="M11.2 4.3c.6-.8 1-1.8.9-2.8-.9 0-2 .6-2.6 1.4-.6.7-1.1 1.7-.9 2.7 1 .1 2-.5 2.6-1.3zM12.1 5.8c-1.4-.1-2.7.8-3.3.8-.7 0-1.7-.8-2.9-.8C4.4 5.9 3 6.8 2.2 8.1c-1.6 2.7-.4 6.8 1.1 9 .8 1.1 1.7 2.3 2.8 2.3 1.1 0 1.6-.7 2.9-.7 1.4 0 1.8.7 2.9.7 1.2 0 2-1.1 2.7-2.2.9-1.3 1.2-2.5 1.2-2.6 0 0-2.4-.9-2.4-3.6 0-2.2 1.8-3.3 1.9-3.4-1-1.5-2.6-1.7-3.2-1.7z" />
        </svg>
        Download for macOS
      </a>
      <span className="download-meta">
        Apple Silicon · macOS 26+ · <a href={ZIP}>.zip</a>
      </span>
    </div>
  )
}

export function App() {
  return (
    <>
      <header className="nav">
        <a className="brand" href="#">
          <img src="/app-icon.png" alt="" />
          Scrawl
        </a>
        <nav>
          <a href="#bundle">Bundle</a>
          <a href="#install">Install</a>
          <a href={REPO}>GitHub</a>
          <a className="nav-dl" href={DMG}>
            Download
          </a>
        </nav>
      </header>

      <main>
        <section className="hero">
          <Suspense
            fallback={
              <div className="hero-brush">
                <img className="hero-icon" src="/app-icon.png" alt="Scrawl" />
              </div>
            }
          >
            <Brush3D />
          </Suspense>
          <h1>Show an agent the problem.</h1>
          <p className="lede">
            Press <kbd>⌥</kbd>
            <kbd>⇧</kbd>
            <kbd>S</kbd>, talk, click, and draw. Paste the result into your agent.
          </p>
          <Download />
        </section>

        <section className="demo-section">
          <Demo />
        </section>

        <section id="bundle" className="bundle-section">
          <h2>
            <code>.scrawl</code> bundle
          </h2>
          <BundleExplorer />
        </section>

        <section className="split">
          <div>
            <h2>Controls</h2>
          </div>
          <ul className="shortcuts">
            {CONTROLS.map(([keys, label]) => (
              <li key={label}>
                <span className="combo">
                  {keys.map((k) => (
                    <kbd key={k}>{k}</kbd>
                  ))}
                </span>
                <span>{label}</span>
              </li>
            ))}
          </ul>
        </section>

        <section id="install" className="split">
          <div>
            <h2>Install</h2>
            <Download />
          </div>
          <div className="code-block">
            <div className="code-head">
              <span>Terminal</span>
              <Copy text={INSTALL} />
            </div>
            <pre>
              {INSTALL.split("\n").map((l) => (
                <div key={l}>
                  <span className="dim">$ </span>
                  {l}
                </div>
              ))}
            </pre>
          </div>
        </section>
      </main>

      <footer className="foot">
        <img src="/app-icon.png" alt="" />
        <span>Scrawl</span>
        <a href={REPO}>GitHub</a>
      </footer>
    </>
  )
}
