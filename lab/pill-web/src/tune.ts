// `motion::Tune` (src/ui/motion.rs): the same keys, defaults and presets as `scrawl tune`, as a
// knobs panel. `pill.*` drive the Metal pill (`Tune::pill_opts`); `island.*`, `scale.*`,
// `reveal.*` and `switch.*` are the GPUI capsule's and kept so exports round-trip.

import { choice, defineControls, flatSpring, knob, toggle } from "knobs"
import type { Opts } from "./wasm"

/** `Tune::DEFAULT`, by tune.json key (`timeScale` is the playground's speed). */
export const DEFAULTS: Record<string, number> = {
  "island.duration": 0.32,
  "island.bounce": 0.0,
  "scale.duration": 0.22,
  "scale.bounce": 0.0,
  "reveal.open.duration": 0.36,
  "reveal.open.bounce": 0.16,
  "reveal.close.duration": 0.32,
  "reveal.close.bounce": 0.08,
  "switch.width.duration": 0.3,
  "switch.width.bounce": 0.0,
  "switch.slide.duration": 0.36,
  "switch.slide.bounce": 0.0,
  "reveal.enter": 0.16,
  "reveal.exit": 0.13,
  "reveal.blur": 2.5,
  timeScale: 1.0,
  "pill.glass": -1.0,
  "pill.origin": 1.0,
  "pill.anchor": 1.0,
  "pill.blur": 1.5,
  "pill.shellBlur": 0.0,
  "pill.enter.duration": 0.25,
  "pill.enter.bounce": 0.0,
  "pill.exit.duration": 0.2,
  "pill.exit.bounce": 0.0,
  "pill.morph.open.duration": 0.42,
  "pill.morph.open.bounce": 0.16,
  "pill.morph.close.duration": 0.38,
  "pill.morph.close.bounce": 0.08,
  "pill.highlight.lead.duration": 0.18,
  "pill.highlight.lead.bounce": 0.0,
  "pill.highlight.trail.duration": 0.25,
  "pill.highlight.trail.bounce": 0.0,
  "pill.punch": 2.0,
}

/** `motion::KEYS` order, as `scrawl tune list` prints them. */
export const KEYS = Object.keys(DEFAULTS)

/** `motion::PRESETS`, numbered from 1 (1 is what ships). */
export const PRESETS: [string, Record<string, number>][] = [
  ["Today", {}],
  ["Before", {
    "island.duration": 0.28, "island.bounce": 0.18, "scale.bounce": 0.14,
    "switch.width.duration": 0.28, "switch.width.bounce": 0.18,
    "switch.slide.duration": 0.28, "switch.slide.bounce": 0.18,
  }],
  ["Calm", {
    "island.duration": 0.4, "switch.width.duration": 0.38, "switch.slide.duration": 0.45,
    "reveal.open.duration": 0.42, "reveal.close.duration": 0.38,
  }],
  ["Brisk", {
    "island.duration": 0.25, "switch.width.duration": 0.22, "switch.slide.duration": 0.28,
    "reveal.open.duration": 0.3, "reveal.close.duration": 0.26,
  }],
  ["Shape", {
    "switch.width.duration": 0.42, "switch.width.bounce": 0.16,
    "switch.slide.duration": 0.42, "switch.slide.bounce": 0.08,
  }],
  ["Glass", { "pill.glass": 1 }],
  ["Centre", { "pill.origin": 0 }],
  ["Unroll", { "pill.origin": 2 }],
  ["Centred chips", { "pill.anchor": 0 }],
  ["0.3.3", {
    "pill.origin": 0, "pill.anchor": 0, "pill.blur": 5, "pill.shellBlur": 4, "pill.punch": 0,
    "pill.exit.duration": 0.21, "pill.morph.open.duration": 0.3, "pill.morph.open.bounce": 0.14,
    "pill.morph.close.duration": 0.3, "pill.morph.close.bounce": 0, "pill.highlight.lead.duration": 0.25,
  }],
  ["Drop", {
    "pill.punch": 0, "pill.morph.open.duration": 0.3, "pill.morph.open.bounce": 0,
    "pill.morph.close.duration": 0.25, "pill.morph.close.bounce": 0, "pill.highlight.lead.duration": 0.25,
  }],
  ["Punch", {
    "pill.punch": 4, "pill.enter.duration": 0.3, "pill.exit.duration": 0.24,
    "pill.highlight.lead.duration": 0.15, "pill.highlight.trail.duration": 0.3,
  }],
]

const STYLE = ["config", "black", "glass"] as const
const ORIGIN = ["centre", "top", "unroll"] as const

const spring = (key: string, label: string, group: string) => {
  const s = flatSpring(
    {
      duration: knob(DEFAULTS[`${key}.duration`], { min: 0.05, max: 1.5, step: 0.01, unit: "s", label: `${label} duration` }),
      bounce: knob(DEFAULTS[`${key}.bounce`], { min: 0, max: 0.6, step: 0.01, label: `${label} bounce` }),
    },
    { group, id: key },
  )
  return { [`${key}.duration`]: s.duration, [`${key}.bounce`]: s.bounce }
}

const PILL = "Metal pill"
const SPRINGS = "Metal pill springs"
const GPUI = "GPUI capsule"

const schema = {
  // "config" is `pillStyle` in config.json, black unless set.
  "pill.glass": choice("config", [...STYLE], { label: "Style", group: PILL, random: false }),
  "pill.origin": choice("top", [...ORIGIN], { label: "Origin", group: PILL }),
  "pill.anchor": toggle(true, { label: "Hold chips into picker", group: PILL }),
  "pill.blur": knob(DEFAULTS["pill.blur"], { min: 0, max: 8, step: 0.1, unit: "pt", label: "Content blur", group: PILL }),
  "pill.shellBlur": knob(DEFAULTS["pill.shellBlur"], { min: 0, max: 8, step: 0.1, unit: "pt", label: "Shell blur", group: PILL }),
  "pill.punch": knob(DEFAULTS["pill.punch"], { min: 0, max: 8, step: 0.25, unit: "pt", label: "Punch", group: PILL }),
  ...spring("pill.enter", "Enter", SPRINGS),
  ...spring("pill.exit", "Exit", SPRINGS),
  ...spring("pill.morph.open", "Morph open", SPRINGS),
  ...spring("pill.morph.close", "Morph close", SPRINGS),
  ...spring("pill.highlight.lead", "Highlight lead", SPRINGS),
  ...spring("pill.highlight.trail", "Highlight trail", SPRINGS),
  ...spring("island", "island", GPUI),
  ...spring("scale", "scale", GPUI),
  ...spring("reveal.open", "reveal.open", GPUI),
  ...spring("reveal.close", "reveal.close", GPUI),
  ...spring("switch.width", "switch.width", GPUI),
  ...spring("switch.slide", "switch.slide", GPUI),
  "reveal.enter": knob(DEFAULTS["reveal.enter"], { min: 0, max: 1, step: 0.01, unit: "s", label: "reveal.enter", group: GPUI }),
  "reveal.exit": knob(DEFAULTS["reveal.exit"], { min: 0, max: 1, step: 0.01, unit: "s", label: "reveal.exit", group: GPUI }),
  "reveal.blur": knob(DEFAULTS["reveal.blur"], { min: 0, max: 8, step: 0.1, unit: "px", label: "reveal.blur", group: GPUI }),
}

type Values = Record<string, number | string | boolean>

/** Knob values → tune.json numbers (every key but `timeScale`). */
export function toTune(v: Values): Record<string, number> {
  const out: Record<string, number> = {}
  for (const key of KEYS) {
    if (key === "timeScale") continue
    const x = v[key]
    if (key === "pill.glass") out[key] = STYLE.indexOf(x as (typeof STYLE)[number]) - 1
    else if (key === "pill.origin") out[key] = ORIGIN.indexOf(x as (typeof ORIGIN)[number])
    else if (key === "pill.anchor") out[key] = x ? 1 : 0
    else out[key] = Number(x)
  }
  return out
}

/** tune.json numbers → knob values (`Tune::from_json`: preset, then keys, over the defaults). */
export function fromTune(t: Record<string, unknown>): Values {
  const merged: Record<string, number> = { ...DEFAULTS }
  const n = typeof t.preset === "number" ? t.preset : 0
  Object.assign(merged, PRESETS[n - 1]?.[1] ?? {})
  for (const [k, x] of Object.entries(t)) {
    if (k === "pillStyle" && typeof x === "string") merged["pill.glass"] = x === "glass" ? 1 : 0
    else if (typeof x === "number" && k in DEFAULTS) merged[k] = x
  }
  const out: Values = {}
  for (const key of KEYS) {
    if (key === "timeScale") continue
    const x = merged[key]
    if (key === "pill.glass") out[key] = STYLE[Math.round(Math.max(-1, Math.min(1, x))) + 1]
    else if (key === "pill.origin") out[key] = ORIGIN[Math.round(Math.max(0, Math.min(2, x)))]
    else if (key === "pill.anchor") out[key] = x >= 0.5
    else out[key] = Math.round(x * 1e4) / 1e4
  }
  return out
}

const presetId = (name: string) => name.toLowerCase().replace(/[^a-z0-9]+/g, "-")

export const tune = defineControls("scrawl-pill-tune", schema, {
  enabled: true,
  defaultLabel: "Today",
  presets: Object.fromEntries(PRESETS.slice(1).map(([name, patch]) => [presetId(name), fromTune(patch)])),
  presetLabels: Object.fromEntries(PRESETS.slice(1).map(([name]) => [presetId(name), name])),
  groupFolded: { [GPUI]: true },
} as never)

/** `Tune::pill_opts`. */
export function pillOpts(t: Record<string, number>): Opts {
  const pair = (k: string): [number, number] => [Math.max(0.02, t[`${k}.duration`]), Math.min(0.95, Math.max(0, t[`${k}.bounce`]))]
  return {
    origin: Math.round(t["pill.origin"]) === 0 ? 0 : Math.round(t["pill.origin"]) === 2 ? 2 : 1,
    holdChips: t["pill.anchor"] >= 0.5,
    contentBlur: t["pill.blur"],
    shellBlur: t["pill.shellBlur"],
    enter: pair("pill.enter"),
    exit: pair("pill.exit"),
    morphOpen: pair("pill.morph.open"),
    morphClose: pair("pill.morph.close"),
    highlightLead: pair("pill.highlight.lead"),
    highlightTrail: pair("pill.highlight.trail"),
    punch: Math.max(0, t["pill.punch"]),
  }
}

/** Only the keys that differ from `Tune::DEFAULT`, plus `timeScale` when it's not 1. */
export function tuneJson(t: Record<string, number>, timeScale: number, all = false): string {
  const out: Record<string, number> = {}
  for (const key of KEYS) {
    const v = key === "timeScale" ? timeScale : t[key]
    if (all || Math.abs(v - DEFAULTS[key]) > 1e-6) out[key] = Math.round(v * 1e4) / 1e4
  }
  return `${JSON.stringify(out, null, 2)}\n`
}

const f = (v: number) => {
  const s = String(Math.round(v * 1e4) / 1e4)
  return s.includes(".") || s.includes("e") ? s : `${s}.0`
}

/** `Tune::DEFAULT` as Rust (`scrawl tune rust`'s format, with the pill keys too). */
export function tuneRust(t: Record<string, number>): string {
  const sp = (k: string) => `Spring::visual(${f(t[`${k}.duration`])}, ${f(t[`${k}.bounce`])})`
  const pair = (k: string) => `[${f(t[`${k}.duration`])}, ${f(t[`${k}.bounce`])}]`
  return [
    "// Paste into `Tune::DEFAULT` in src/ui/motion.rs.",
    `island: ${sp("island")},`,
    `island_scale: ${sp("scale")},`,
    `reveal_open: ${sp("reveal.open")},`,
    `reveal_close: ${sp("reveal.close")},`,
    `switch_width: ${sp("switch.width")},`,
    `switch_slide: ${sp("switch.slide")},`,
    `reveal_enter_s: ${f(t["reveal.enter"])},`,
    `reveal_exit_s: ${f(t["reveal.exit"])},`,
    `reveal_blur_px: ${f(t["reveal.blur"])},`,
    "time_scale: 1.0,",
    `pill_glass: ${f(t["pill.glass"])},`,
    `pill_origin: ${f(t["pill.origin"])},`,
    `pill_anchor: ${f(t["pill.anchor"])},`,
    `pill_blur: ${f(t["pill.blur"])},`,
    `pill_shell_blur: ${f(t["pill.shellBlur"])},`,
    `pill_enter: ${pair("pill.enter")},`,
    `pill_exit: ${pair("pill.exit")},`,
    `pill_morph_open: ${pair("pill.morph.open")},`,
    `pill_morph_close: ${pair("pill.morph.close")},`,
    `pill_highlight_lead: ${pair("pill.highlight.lead")},`,
    `pill_highlight_trail: ${pair("pill.highlight.trail")},`,
    `pill_punch: ${f(t["pill.punch"])},`,
    "",
  ].join("\n")
}
