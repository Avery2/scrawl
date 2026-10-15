import { memo, type ReactNode, type Ref } from "react"
import { lerp, MAC_WINDOW, PHONE } from "./script"

export type CardRefs = {
  badge?: Ref<HTMLSpanElement>
  actions?: Ref<HTMLDivElement>
  title?: Ref<HTMLHeadingElement>
}

const ROWS = [
  ["api-gateway", "a41c9e2", "prod", "ok", "2m"],
  ["billing-worker", "7f03b1d", "prod", "fail", "9m"],
  ["web-console", "c2e88a0", "staging", "ok", "14m"],
  ["search-indexer", "19bd4f7", "prod", "ok", "31m"],
] as const

export function TrafficLights({ inactive }: { inactive?: boolean }) {
  return (
    <span className={`lights${inactive ? " inactive" : ""}`} aria-hidden>
      <i />
      <i />
      <i />
    </span>
  )
}

// How far the fix has carried each changed element (spring values, 0 broken → 1 fixed) and where
// the single lock-in sheen is on each (-1 when it isn't passing).
export type Fix = { badge: number; rows: number; actions: number; badgeSheen: number; actionsSheen: number }
export const BROKEN: Fix = { badge: 0, rows: 0, actions: 0, badgeSheen: -1, actionsSheen: -1 }

// A soft band of light crossing its element once: p runs 0 → 1, the band enters and leaves outside.
function Sheen({ p, alpha }: { p: number; alpha: number }) {
  if (p < 0 || p > 1 || alpha <= 0) return null
  return <i className="sheen" style={{ backgroundPosition: `${(130 - 160 * p).toFixed(2)}% 0`, opacity: alpha }} />
}

export function DeployCard({ fix, sheen, refs, compact }: { fix: Fix; sheen: number; refs?: CardRefs; compact?: boolean }) {
  const badgeX = compact ? -46 : -60
  const rf = Math.max(0, fix.rows)
  return (
    <div className={`card${compact ? " compact" : ""}`}>
      <div className="card-head">
        <h3 ref={refs?.title} className="card-title">
          Deploy Activity
        </h3>
        <span
          ref={refs?.badge}
          className="badge"
          style={{ transform: fix.badge === 1 ? undefined : `translate3d(${lerp(badgeX, 0, fix.badge).toFixed(2)}px, ${lerp(-8, 0, fix.badge).toFixed(2)}px, 0)` }}
        >
          <i />
          Live
          <Sheen p={fix.badgeSheen} alpha={sheen} />
        </span>
      </div>
      <p className="card-sub">Last 24 hours · 128 deploys</p>
      <ul className="rows">
        {ROWS.map(([name, sha, env, status, ago]) => (
          <li key={name} style={{ paddingBlock: `${lerp(1, compact ? 5 : 6.5, rf).toFixed(2)}px` }}>
            <span className="row-name">
              {name}
              {!compact && <em>{sha}</em>}
            </span>
            {!compact && <span className={`row-env ${env}`}>{env}</span>}
            <span className={`row-status ${status}`}>
              <i />
              {status === "ok" ? "Healthy" : "Failed"}
            </span>
            <span className="row-ago">{ago}</span>
          </li>
        ))}
      </ul>
      <div ref={refs?.actions} className="actions">
        <div className="actions-inner" style={{ transform: fix.actions === 1 ? undefined : `translate3d(0, ${lerp(14, 0, fix.actions).toFixed(2)}px, 0)` }}>
          <button>
            Retry failed
            <Sheen p={fix.actionsSheen} alpha={sheen} />
          </button>
          <button className="primary">
            Promote
            <Sheen p={fix.actionsSheen - 0.12} alpha={sheen} />
          </button>
        </div>
      </div>
    </div>
  )
}

const icon = (d: ReactNode) => (
  <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
    {d}
  </svg>
)

const NAV: [string, ReactNode, string?][] = [
  ["Overview", icon(<><rect x="2.5" y="2.5" width="4.5" height="4.5" rx="1.2" /><rect x="9" y="2.5" width="4.5" height="4.5" rx="1.2" /><rect x="2.5" y="9" width="4.5" height="4.5" rx="1.2" /><rect x="9" y="9" width="4.5" height="4.5" rx="1.2" /></>)],
  ["Deploys", icon(<><circle cx="8" cy="8" r="5.6" /><path d="M8 10.8V5.4M5.6 7.6L8 5.2l2.4 2.4" /></>)],
  ["Services", icon(<><rect x="2.5" y="3" width="11" height="4" rx="1.2" /><rect x="2.5" y="9" width="11" height="4" rx="1.2" /><path d="M5 5h.01M5 11h.01" strokeWidth="1.8" /></>)],
  ["Incidents", icon(<><path d="M8 2.6l5.6 9.8H2.4z" /><path d="M8 6.6v2.6M8 11h.01" /></>), "1"],
  ["Logs", icon(<><path d="M3 4h10M3 8h10M3 12h6" /></>)],
]

const ENVS: [string, string][] = [
  ["production", "#30d158"],
  ["staging", "#ffd60a"],
  ["preview", "#8e8e93"],
]

const Spark = memo(function Spark() {
  const pts = [9, 12, 10, 14, 13, 17, 15, 19, 16, 21, 18, 22]
  const d = pts.map((v, i) => `${i ? "L" : "M"}${(i / (pts.length - 1)) * 64},${24 - v}`).join(" ")
  return (
    <svg className="spark" viewBox="0 0 64 24" aria-hidden>
      <path d={`${d} L64,24 L0,24 Z`} fill="rgba(10,132,255,.18)" />
      <path d={d} fill="none" stroke="#0a84ff" strokeWidth="1.4" strokeLinejoin="round" />
    </svg>
  )
})

export const MacWindow = memo(function MacWindow({ fix, sheen = 0, refs, inactive }: { fix: Fix; sheen?: number; refs?: CardRefs; inactive?: boolean }) {
  return (
    <div className={`win${inactive ? " inactive" : ""}`} style={{ left: MAC_WINDOW.x, top: MAC_WINDOW.y, width: MAC_WINDOW.w, height: MAC_WINDOW.h }}>
      <aside className="win-side">
        <TrafficLights inactive={inactive} />
        <nav>
          {NAV.map(([label, glyph, count]) => (
            <span key={label} className={label === "Deploys" ? "on" : ""}>
              {glyph}
              {label}
              {count && <em>{count}</em>}
            </span>
          ))}
          <b>Environments</b>
          {ENVS.map(([label, color]) => (
            <span key={label}>
              <i style={{ background: color }} />
              {label}
            </span>
          ))}
        </nav>
        <div className="win-user">
          <span>KL</span>
          <div>
            <b>Kit Langton</b>
            <em>pulse.dev</em>
          </div>
        </div>
      </aside>
      <div className="win-main">
        <header className="win-toolbar">
          <div className="win-title">
            <b>Deploys</b>
            <em>main · 3 services</em>
          </div>
          <span className="seg">
            <span className="on">24h</span>
            <span>7d</span>
            <span>30d</span>
          </span>
          <span className="win-search">
            {icon(<><circle cx="7" cy="7" r="4.2" /><path d="M10.2 10.2l3 3" /></>)}
            Search
          </span>
        </header>
        <div className="win-content">
          <div className="stats">
            <div>
              <em>Deploys</em>
              <b>128</b>
              <Spark />
            </div>
            <div>
              <em>Success rate</em>
              <b>97.6%</b>
              <small className="up">▲ 1.2%</small>
            </div>
            <div>
              <em>Median duration</em>
              <b>4m 12s</b>
              <small>p95 9m 40s</small>
            </div>
          </div>
          <DeployCard fix={fix} sheen={sheen} refs={refs} />
          <div className="card incidents">
            <div className="card-head">
              <h4>Incidents</h4>
              <span>1 open</span>
            </div>
            <ul>
              <li>
                <i className="sev-high" />
                billing-worker exited with code 137<em>9m</em>
              </li>
              <li>
                <i className="sev-low" />
                web-console p95 latency above 800 ms<em>2h</em>
              </li>
              <li>
                <i />
                api-gateway certificate renewed<em>1d</em>
              </li>
            </ul>
          </div>
        </div>
      </div>
    </div>
  )
})

const PhoneStatus = memo(function PhoneStatus() {
  return (
    <div className="phone-status">
      <b>9:41</b>
      <span className="phone-island" />
      <span className="phone-glyphs" aria-hidden>
        <svg viewBox="0 0 18 12">
          <rect x="0" y="8" width="3" height="4" rx="0.8" />
          <rect x="5" y="5.5" width="3" height="6.5" rx="0.8" />
          <rect x="10" y="3" width="3" height="9" rx="0.8" />
          <rect x="15" y="0" width="3" height="12" rx="0.8" />
        </svg>
        <svg viewBox="0 0 16 12">
          <path d="M8 11.4l2-2.3a2.9 2.9 0 00-4 0zM3.6 6.6l1.3 1.5a4.7 4.7 0 016.2 0l1.3-1.5a6.6 6.6 0 00-8.8 0zM.8 3.6l1.3 1.5a8.6 8.6 0 0111.8 0l1.3-1.5a10.6 10.6 0 00-14.4 0z" />
        </svg>
        <span className="phone-battery">
          <i />
        </span>
      </span>
    </div>
  )
})

export const PhoneMirror = memo(function PhoneMirror({ fix, sheen = 0, refs, live }: { fix: Fix; sheen?: number; refs?: CardRefs; live?: number }) {
  return (
    <div
      className="phone"
      style={{
        left: PHONE.x,
        top: PHONE.y,
        width: PHONE.w,
        height: PHONE.h,
        boxShadow: live
          ? `0 0 0 ${2 * live}px rgba(255,59,48,${0.85 * live}), 0 0 0 1px #3a3a3e, 0 30px 80px rgba(0,0,0,.55)`
          : undefined,
      }}
    >
      <div className="phone-screen">
        <PhoneStatus />
        <div className="phone-nav">
          <span>‹ Pulse</span>
          <h2>Deploys</h2>
        </div>
        <DeployCard fix={fix} sheen={sheen} refs={refs} compact />
        <div className="phone-section">
          <b>Incidents</b>
          <div className="phone-incident">
            <i />
            <span>billing-worker exited 137</span>
            <em>9m</em>
          </div>
          <div className="phone-incident ok">
            <i />
            <span>web-console p95 latency</span>
            <em>2h</em>
          </div>
        </div>
        <div className="phone-tabs">
          {["Overview", "Deploys", "Alerts", "Settings"].map((t) => (
            <span key={t} className={t === "Deploys" ? "on" : ""}>
              <i />
              {t}
            </span>
          ))}
        </div>
        <span className="phone-home" />
      </div>
    </div>
  )
})
