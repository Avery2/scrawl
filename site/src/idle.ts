import { useSyncExternalStore } from "react"

// Runs `fn` once the page has loaded and the main thread is idle, so optional
// work (the 3D brush, the wet-ink shader, narration audio) never competes with
// the first paint. Returns a cancel function for effect cleanup.
export function afterLoad(fn: () => void, timeout = 3000): () => void {
  let cancelled = false
  let idle: number | undefined
  const call = () => {
    if (!cancelled) fn()
  }
  const schedule = () => {
    idle = typeof requestIdleCallback === "function" ? requestIdleCallback(call, { timeout }) : window.setTimeout(call, 200)
  }
  if (document.readyState === "complete") schedule()
  else window.addEventListener("load", schedule, { once: true })
  return () => {
    cancelled = true
    window.removeEventListener("load", schedule)
    if (idle !== undefined) (typeof cancelIdleCallback === "function" ? cancelIdleCallback : window.clearTimeout)(idle)
  }
}

const noSubscribe = () => () => {}

// False in the prerendered HTML and while React hydrates it, then true. Parts
// that only make sense with scripts running (the live demo stage, the
// animated diagrams) render once this flips, so hydration matches the HTML.
export const useHydrated = () => useSyncExternalStore(noSubscribe, () => true, () => false)
