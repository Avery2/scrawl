import type { Ref } from "react"
import { lerp, MAC_WINDOW, PHONE } from "./script"

export type CardRefs = {
  badge?: Ref<HTMLSpanElement>
  actions?: Ref<HTMLDivElement>
  title?: Ref<HTMLHeadingElement>
}

const ROWS = [
  ["api-gateway", "prod", "ok", "2m"],
  ["billing-worker", "prod", "fail", "9m"],
  ["web-console", "staging", "ok", "14m"],
  ["search-indexer", "prod", "ok", "31m"],
] as const

export function DeployCard({ fix, settle, refs, compact }: { fix: number; settle: number; refs?: CardRefs; compact?: boolean }) {
  const badgeX = compact ? -46 : -60
  return (
    <div className={`card${compact ? " compact" : ""}`} style={{ boxShadow: settle > 0 && settle < 1 ? `0 0 0 ${1.5}px rgba(52,199,89,${0.55 * (1 - settle)})` : undefined }}>
      <div className="card-head">
        <h3 ref={refs?.title} className="card-title">
          Deploy Activity
        </h3>
        <span
          ref={refs?.badge}
          className="badge"
          style={{ transform: `translate(${lerp(badgeX, 0, fix)}px, ${lerp(-8, 0, fix)}px)` }}
        >
          <i />
          Live
        </span>
      </div>
      <p className="card-sub">Last 24 hours · 128 deploys</p>
      <ul className="rows">
        {ROWS.map(([name, env, status, ago]) => (
          <li key={name} style={{ paddingBlock: lerp(1, compact ? 8 : 9, Math.min(1, Math.max(0, fix))) }}>
            <span className="row-name">{name}</span>
            {!compact && <span className="row-env">{env}</span>}
            <span className={`row-status ${status}`}>
              <i />
              {status === "ok" ? "Healthy" : "Failed"}
            </span>
            <span className="row-ago">{ago}</span>
          </li>
        ))}
      </ul>
      <div ref={refs?.actions} className="actions">
        <div className="actions-inner" style={{ transform: `translateY(${lerp(14, 0, fix)}px)` }}>
          <button>Retry failed</button>
          <button className="primary">Promote</button>
        </div>
      </div>
    </div>
  )
}

export function MacWindow({ fix, settle, refs }: { fix: number; settle: number; refs?: CardRefs }) {
  return (
    <div className="win" style={{ left: MAC_WINDOW.x, top: MAC_WINDOW.y, width: MAC_WINDOW.w, height: MAC_WINDOW.h }}>
      <div className="win-side">
        <div className="lights">
          <i />
          <i />
          <i />
        </div>
        <nav>
          <span>Overview</span>
          <span className="on">Deploys</span>
          <span>Services</span>
          <span>Incidents</span>
          <span>Settings</span>
        </nav>
      </div>
      <div className="win-main">
        <div className="win-toolbar">
          <b>Deploys</b>
          <span className="seg">
            <span className="on">24h</span>
            <span>7d</span>
            <span>30d</span>
          </span>
        </div>
        <DeployCard fix={fix} settle={settle} refs={refs} />
      </div>
    </div>
  )
}

export function PhoneMirror({ fix, settle, refs, live }: { fix: number; settle: number; refs?: CardRefs; live?: number }) {
  return (
    <div
      className="phone"
      style={{
        left: PHONE.x,
        top: PHONE.y,
        width: PHONE.w,
        height: PHONE.h,
        boxShadow: live ? `0 0 0 ${2 * live}px rgba(255,59,48,${0.85 * live}), 0 30px 80px rgba(0,0,0,.55)` : undefined,
      }}
    >
      <div className="phone-screen">
        <div className="phone-status">
          <b>9:41</b>
          <span className="phone-island" />
          <span className="phone-glyphs">
            <i />
            <i />
          </span>
        </div>
        <div className="phone-nav">
          <span>Pulse</span>
          <h2>Deploys</h2>
        </div>
        <DeployCard fix={fix} settle={settle} refs={refs} compact />
        <div className="phone-section">
          <b>Incidents</b>
          <div className="phone-incident">
            <i />
            <span>billing-worker exited 137</span>
            <em>9m</em>
          </div>
          <div className="phone-incident ok">
            <i />
            <span>web-console latency</span>
            <em>2h</em>
          </div>
        </div>
        <div className="phone-tabs">
          <span />
          <span className="on" />
          <span />
          <span />
        </div>
      </div>
    </div>
  )
}
