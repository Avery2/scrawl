import { type Anchors, type Rect, STAGE } from "./script"

export function measureAnchors(stage: Element, badge: Element, actions: Element, title: Element): Anchors {
  const base = stage.getBoundingClientRect()
  const s = base.width / STAGE.w
  const rect = (el: Element): Rect => {
    const r = el.getBoundingClientRect()
    return { x: (r.left - base.left) / s, y: (r.top - base.top) / s, w: r.width / s, h: r.height / s }
  }
  return { badge: rect(badge), actions: rect(actions), title: rect(title) }
}

const sameRect = (a: Rect, b: Rect) => Math.abs(a.x - b.x) + Math.abs(a.y - b.y) + Math.abs(a.w - b.w) + Math.abs(a.h - b.h) < 0.5

export const sameAnchors = (a: Anchors, b: Anchors) => sameRect(a.badge, b.badge) && sameRect(a.actions, b.actions) && sameRect(a.title, b.title)
