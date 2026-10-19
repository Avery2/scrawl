import { lazy, Suspense } from "react"
import { BrushFallback } from "./brush-paths"
import { Copy } from "./Copy"
import { Demo } from "./demo/Demo"
import { Download, useDownloadHref } from "./Download"
import { HowItWorks } from "./HowItWorks"
import { IPhoneSection } from "./IPhone"
import { BREW, MAC_ZIP } from "./release"

const Brush3D = lazy(() => import("./Brush3D").then((m) => ({ default: m.Brush3D })))

const REPO = "https://github.com/kitlangton/scrawl"
const ZIP = `https://scrawl.kitlangton.com${MAC_ZIP}`

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

export function App() {
  const downloadHref = useDownloadHref()
  return (
    <>
      <header className="nav">
        <a className="brand" href="#">
          <img src="/app-icon.png" width={26} height={26} decoding="async" alt="" />
          Scrawl
        </a>
        <nav>
          <a href="#iphone">iPhone</a>
          <a href="#how">How it works</a>
          <a href="#install">Install</a>
          <a href={REPO}>GitHub</a>
          <a className="nav-dl" href={downloadHref}>
            Download
          </a>
        </nav>
      </header>

      <main>
        <section className="hero">
          <Suspense fallback={<BrushFallback />}>
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

        <IPhoneSection />

        <HowItWorks />

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
            <Download brew={false} />
            {/* TODO: add a "For agents" note with `bunx skills add kitlangton/scrawl`
                once README.md and skills/ on main describe the skill again (reverted in 52bdf48). */}
          </div>
          <div className="code-blocks">
            <div className="code-block">
              <div className="code-head">
                <span>Homebrew</span>
                <Copy text={BREW} />
              </div>
              <pre>
                <div>
                  <span className="dim">$ </span>
                  {BREW}
                </div>
              </pre>
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
          </div>
        </section>
      </main>

      <footer className="foot">
        <img src="/app-icon.png" width={20} height={20} decoding="async" alt="" />
        <span>Scrawl</span>
        <a href={REPO}>GitHub</a>
      </footer>
    </>
  )
}
