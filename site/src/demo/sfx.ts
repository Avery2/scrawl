// Cuelume's synthesizer (cuelume/src/audio/engine.ts and sounds/recipes.ts), trimmed to the
// sounds the demo uses, plus a continuous pen/drag friction voice and the narration mute gate.
import { type Mode, NARRATION, T } from "./script"

type BaseLayer = { offset?: number; attack: number; decay: number; peak: number }
type ToneLayer = BaseLayer & { kind: "tone"; waveform: OscillatorType; frequency: number; detune?: number; glideTo?: number; glideTime?: number }
type NoiseLayer = BaseLayer & { kind: "noise"; filterType: BiquadFilterType; filterFrequency: number; filterQ?: number }
type Shimmer = { delay: number; feedback: number; wet: number; lowpass: number }
type Recipe = { masterGain: number; layers: (ToneLayer | NoiseLayer)[]; shimmer?: Shimmer }

const RECIPES = {
  chime: {
    masterGain: 0.5,
    layers: [
      { kind: "tone", waveform: "sine", frequency: 1046.5, attack: 0.006, decay: 0.22, peak: 0.09 },
      { kind: "tone", waveform: "sine", frequency: 1568, offset: 0.09, attack: 0.006, decay: 0.26, peak: 0.08 },
    ],
    shimmer: { delay: 0.12, feedback: 0.25, wet: 0.18, lowpass: 4000 },
  },
  droplet: {
    masterGain: 0.55,
    layers: [{ kind: "tone", waveform: "sine", frequency: 1200, glideTo: 550, glideTime: 0.14, attack: 0.004, decay: 0.2, peak: 0.075 }],
    shimmer: { delay: 0.09, feedback: 0.2, wet: 0.15, lowpass: 3000 },
  },
  whisper: {
    masterGain: 0.48,
    layers: [
      { kind: "noise", filterType: "lowpass", filterFrequency: 1600, filterQ: 0.7, attack: 0.025, decay: 0.13, peak: 0.04 },
      { kind: "tone", waveform: "sine", frequency: 880, glideTo: 660, glideTime: 0.14, offset: 0.01, attack: 0.012, decay: 0.14, peak: 0.025 },
    ],
  },
  tick: {
    masterGain: 0.4,
    layers: [
      { kind: "noise", filterType: "bandpass", filterFrequency: 5400, filterQ: 1.8, attack: 0.001, decay: 0.018, peak: 0.14 },
      { kind: "tone", waveform: "sine", frequency: 2600, attack: 0.001, decay: 0.012, peak: 0.018 },
    ],
  },
  press: {
    masterGain: 0.4,
    layers: [{ kind: "noise", filterType: "bandpass", filterFrequency: 1700, filterQ: 1.4, attack: 0.001, decay: 0.02, peak: 0.13 }],
  },
  release: {
    masterGain: 0.4,
    layers: [
      { kind: "noise", filterType: "bandpass", filterFrequency: 4600, filterQ: 1.8, attack: 0.001, decay: 0.016, peak: 0.12 },
      { kind: "tone", waveform: "sine", frequency: 3200, offset: 0.006, attack: 0.001, decay: 0.05, peak: 0.02 },
    ],
  },
  toggle: {
    masterGain: 0.4,
    layers: [
      { kind: "noise", filterType: "bandpass", filterFrequency: 2200, filterQ: 1.6, attack: 0.001, decay: 0.016, peak: 0.12 },
      { kind: "noise", filterType: "bandpass", filterFrequency: 3800, filterQ: 1.6, offset: 0.024, attack: 0.001, decay: 0.02, peak: 0.1 },
    ],
  },
  success: {
    masterGain: 0.5,
    layers: [
      { kind: "tone", waveform: "sine", frequency: 880, attack: 0.004, decay: 0.09, peak: 0.06 },
      { kind: "tone", waveform: "sine", frequency: 1108.73, offset: 0.06, attack: 0.004, decay: 0.1, peak: 0.06 },
      { kind: "tone", waveform: "sine", frequency: 1318.51, offset: 0.12, attack: 0.004, decay: 0.18, peak: 0.07 },
    ],
    shimmer: { delay: 0.1, feedback: 0.22, wet: 0.16, lowpass: 4500 },
  },
  pulse: {
    masterGain: 0.42,
    layers: [
      { kind: "noise", filterType: "bandpass", filterFrequency: 2600, filterQ: 2.4, attack: 0.001, decay: 0.022, peak: 0.08 },
      { kind: "tone", waveform: "triangle", frequency: 620, glideTo: 1240, glideTime: 0.07, attack: 0.002, decay: 0.085, peak: 0.055 },
    ],
  },
} as const satisfies Record<string, Recipe>

export type Sound = keyof typeof RECIPES

const SOURCE_STOP_PADDING = 0.05
const CLEANUP_MARGIN = 0.05
const INAUDIBLE_GAIN = 0.001
const OUTPUT_GAIN = 4
// Effects sit under the narration.
const SFX_VOLUME = 0.6

let ctx: AudioContext | null = null
let output: GainNode | null = null
let enabled = true

function context(): AudioContext | null {
  if (ctx) return ctx
  if (typeof window === "undefined") return null
  const Ctor = window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
  if (!Ctor) return null
  try {
    ctx = new Ctor()
  } catch {
    return null
  }
  return ctx
}

function out(c: AudioContext): GainNode {
  if (output) return output
  output = c.createGain()
  output.gain.value = OUTPUT_GAIN * SFX_VOLUME
  const limiter = c.createDynamicsCompressor()
  limiter.threshold.value = -8
  limiter.knee.value = 6
  limiter.ratio.value = 12
  limiter.attack.value = 0.002
  limiter.release.value = 0.08
  output.connect(limiter).connect(c.destination)
  return output
}

// Must run inside a user gesture: creates or resumes the shared context.
export function unlock() {
  const c = context()
  if (c && c.state !== "running") c.resume().catch(() => {})
}

export function setEnabled(v: boolean) {
  enabled = v
  if (!v) for (const ch of channels.keys()) scribbleStop(ch)
}

const live = () => (enabled && ctx && ctx.state === "running" ? ctx : null)

function noiseBuffer(c: AudioContext, seconds: number) {
  const length = Math.max(1, Math.floor(seconds * c.sampleRate))
  const buffer = c.createBuffer(1, length, c.sampleRate)
  const data = buffer.getChannelData(0)
  for (let i = 0; i < length; i++) data[i] = 2 * Math.random() - 1
  return buffer
}

function renderTone(c: AudioContext, dest: AudioNode, l: ToneLayer, start: number) {
  const osc = c.createOscillator()
  osc.type = l.waveform
  osc.frequency.setValueAtTime(l.frequency, start)
  if (l.detune) osc.detune.value = l.detune
  if (l.glideTo !== undefined) osc.frequency.exponentialRampToValueAtTime(l.glideTo, start + (l.glideTime ?? l.attack + l.decay))
  const gain = c.createGain()
  gain.gain.setValueAtTime(0.0001, start)
  gain.gain.exponentialRampToValueAtTime(l.peak, start + l.attack)
  gain.gain.exponentialRampToValueAtTime(0.0001, start + l.attack + l.decay)
  osc.connect(gain).connect(dest)
  osc.start(start)
  osc.stop(start + l.attack + l.decay + SOURCE_STOP_PADDING)
}

function renderNoise(c: AudioContext, dest: AudioNode, l: NoiseLayer, start: number) {
  const duration = l.attack + l.decay + SOURCE_STOP_PADDING
  const source = c.createBufferSource()
  source.buffer = noiseBuffer(c, duration)
  const filter = c.createBiquadFilter()
  filter.type = l.filterType
  filter.frequency.value = l.filterFrequency
  if (l.filterQ !== undefined) filter.Q.value = l.filterQ
  const gain = c.createGain()
  gain.gain.setValueAtTime(0.0001, start)
  gain.gain.exponentialRampToValueAtTime(l.peak, start + l.attack)
  gain.gain.exponentialRampToValueAtTime(0.0001, start + l.attack + l.decay)
  source.connect(filter).connect(gain).connect(dest)
  source.start(start)
  source.stop(start + duration)
}

function attachShimmer(c: AudioContext, source: AudioNode, dest: AudioNode, s: Shimmer): AudioNode[] {
  const delay = c.createDelay(1)
  delay.delayTime.value = s.delay
  const filter = c.createBiquadFilter()
  filter.type = "lowpass"
  filter.frequency.value = s.lowpass
  const feedback = c.createGain()
  feedback.gain.value = s.feedback
  const wet = c.createGain()
  wet.gain.value = s.wet
  source.connect(delay)
  delay.connect(filter)
  filter.connect(feedback)
  feedback.connect(delay)
  filter.connect(wet)
  wet.connect(dest)
  return [delay, filter, feedback, wet]
}

export function play(sound: Sound, volume = 1, delay = 0) {
  const c = live()
  if (!c) return
  const recipe: Recipe = RECIPES[sound]
  const now = c.currentTime + delay
  const dest = out(c)
  const master = c.createGain()
  master.gain.value = recipe.masterGain * volume
  master.connect(dest)
  const shimmer = recipe.shimmer ? attachShimmer(c, master, dest, recipe.shimmer) : []
  for (const l of recipe.layers) {
    const start = now + (l.offset ?? 0)
    if (l.kind === "tone") renderTone(c, master, l, start)
    else renderNoise(c, master, l, start)
  }
  const end = Math.max(...recipe.layers.map((l) => (l.offset ?? 0) + l.attack + l.decay + SOURCE_STOP_PADDING))
  const s = recipe.shimmer
  const tail = s && s.feedback > 0 ? s.delay * (1 + Math.ceil(Math.log(INAUDIBLE_GAIN) / Math.log(s.feedback))) : 0
  setTimeout(() => {
    master.disconnect()
    for (const n of shimmer) n.disconnect()
  }, (delay + end + tail + CLEANUP_MARGIN) * 1000)
}

// ---------- Continuous friction: a dragged selection or a marker on glass ----------

export type Texture = "glass" | "marker"

const TEXTURE = {
  // A soft, low-passed slide.
  glass: { type: "lowpass" as BiquadFilterType, lo: 600, hi: 1200, q: 0.8, gain: 0.07, hum: 0.008, humLo: 90, humHi: 140 },
  // A felt tip squeaking across glass.
  marker: { type: "bandpass" as BiquadFilterType, lo: 1400, hi: 2800, q: 1.3, gain: 0.16, hum: 0.012, humLo: 170, humHi: 260 },
}

type Voice = { texture: Texture; noise: AudioBufferSourceNode; filter: BiquadFilterNode; gain: GainNode; hum: OscillatorNode; humGain: GainNode }

const channels = new Map<string, Voice>()
let pink: AudioBuffer | null = null

function pinkBuffer(c: AudioContext) {
  if (pink && pink.sampleRate === c.sampleRate) return pink
  const length = c.sampleRate * 2
  pink = c.createBuffer(1, length, c.sampleRate)
  const d = pink.getChannelData(0)
  let b0 = 0, b1 = 0, b2 = 0, b3 = 0, b4 = 0, b5 = 0, b6 = 0
  for (let i = 0; i < length; i++) {
    const w = Math.random() * 2 - 1
    b0 = 0.99886 * b0 + w * 0.0555179
    b1 = 0.99332 * b1 + w * 0.0750759
    b2 = 0.969 * b2 + w * 0.153852
    b3 = 0.8665 * b3 + w * 0.3104856
    b4 = 0.55 * b4 + w * 0.5329522
    b5 = -0.7616 * b5 - w * 0.016898
    d[i] = (b0 + b1 + b2 + b3 + b4 + b5 + b6 + w * 0.5362) * 0.11
    b6 = w * 0.115926
  }
  return pink
}

// Moves a param toward `value`, replacing anything scheduled after now.
function glide(p: AudioParam, value: number, now: number, tc: number) {
  const hold = (p as AudioParam & { cancelAndHoldAtTime?: (t: number) => void }).cancelAndHoldAtTime
  if (hold) hold.call(p, now)
  else {
    p.cancelScheduledValues(now)
    p.setValueAtTime(p.value, now)
  }
  p.setTargetAtTime(value, now, tc)
}

function voice(c: AudioContext, ch: string, texture: Texture): Voice {
  const existing = channels.get(ch)
  if (existing && existing.texture === texture) return existing
  if (existing) scribbleStop(ch)
  const cfg = TEXTURE[texture]
  const noise = c.createBufferSource()
  noise.buffer = pinkBuffer(c)
  noise.loop = true
  const filter = c.createBiquadFilter()
  filter.type = cfg.type
  filter.frequency.value = cfg.lo
  filter.Q.value = cfg.q
  const gain = c.createGain()
  gain.gain.value = 0
  const hum = c.createOscillator()
  hum.type = "sine"
  hum.frequency.value = cfg.humLo
  const humGain = c.createGain()
  humGain.gain.value = 0
  const dest = out(c)
  noise.connect(filter).connect(gain).connect(dest)
  hum.connect(humGain).connect(dest)
  noise.start(c.currentTime, Math.random() * 1.5)
  hum.start()
  const v = { texture, noise, filter, gain, hum, humGain }
  channels.set(ch, v)
  return v
}

// `speed` is 0..1. Each update also schedules a fade, so a pen that stops moving goes quiet.
export function scribble(ch: string, texture: Texture, speed: number) {
  const c = live()
  if (!c) return
  const v = voice(c, ch, texture)
  const cfg = TEXTURE[texture]
  const s = Math.min(1, Math.max(0, speed))
  const now = c.currentTime
  const level = s < 0.02 ? 0 : Math.pow(s, 0.7)
  glide(v.gain.gain, cfg.gain * level, now, 0.025)
  v.gain.gain.setTargetAtTime(0, now + 0.12, 0.05)
  glide(v.filter.frequency, cfg.lo + (cfg.hi - cfg.lo) * s, now, 0.03)
  glide(v.humGain.gain, cfg.hum * level, now, 0.04)
  v.humGain.gain.setTargetAtTime(0, now + 0.12, 0.05)
  glide(v.hum.frequency, cfg.humLo + (cfg.humHi - cfg.humLo) * s, now, 0.05)
}

export function scribbleStop(ch: string) {
  const v = channels.get(ch)
  if (!v || !ctx) return
  channels.delete(ch)
  const now = ctx.currentTime
  glide(v.gain.gain, 0, now, 0.03)
  glide(v.humGain.gain, 0, now, 0.03)
  v.noise.stop(now + 0.25)
  v.hum.stop(now + 0.25)
  setTimeout(() => {
    v.gain.disconnect()
    v.humGain.disconnect()
  }, 400)
}

// ---------- Narration ----------

const routed = new WeakMap<HTMLMediaElement, GainNode>()

// Routes the narration element through a gain node so the direction-tag gaps can be held silent.
export function routeNarration(el: HTMLMediaElement): GainNode | null {
  const existing = routed.get(el)
  if (existing) return existing
  const c = context()
  if (!c) return null
  try {
    const gain = c.createGain()
    c.createMediaElementSource(el).connect(gain).connect(c.destination)
    routed.set(el, gain)
    return gain
  } catch {
    return null
  }
}

// Called every frame. Edges in the next 400 ms are scheduled on the audio clock, because frames
// (and media currentTime updates) are too coarse to land the unmute right before a word.
export function gateNarration(el: HTMLMediaElement, mode: Mode) {
  const s = el.currentTime
  const windows = NARRATION[mode].mute
  const muted = windows.some(([a, b]) => s >= a && s < b)
  const gain = routed.get(el)
  if (!gain || !ctx) {
    el.volume = muted ? 0 : 1
    return
  }
  const now = ctx.currentTime
  glide(gain.gain, muted ? 0 : 1, now, 0.008)
  for (const [a, b] of windows) {
    if (a > s && a - s < 0.4) gain.gain.setTargetAtTime(0, now + (a - s), 0.008)
    if (b > s && b - s < 0.4) gain.gain.setTargetAtTime(1, now + (b - s), 0.008)
  }
}

// ---------- Scripted cues ----------

export type Cue = { t: number; sound: Sound; volume?: number }

export function demoCues(mode: Mode): Cue[] {
  const keys = (down: number[], up: number[]): Cue[] => [
    ...down.map((t) => ({ t, sound: "press" as const })),
    ...up.map((t) => ({ t, sound: "release" as const })),
  ]
  const cues: Cue[] = [
    ...keys([T.keys, T.keys + 150, T.keys + 300], [T.keysUp, T.keysUp + 45, T.keysUp + 90]),
    { t: T.select, sound: "whisper" },
    { t: T.select + 30, sound: "toggle", volume: 0.6 },
    { t: T.pin1, sound: "droplet" },
    { t: T.pin2, sound: "droplet" },
    { t: T.finish, sound: "toggle" },
    { t: T.copied, sound: "chime" },
    { t: T.promptClick, sound: "tick" },
    ...keys([T.paste - 300, T.paste - 150, T.paste], [T.paste + 200, T.paste + 250, T.paste + 270]),
    { t: T.paste + 20, sound: "pulse", volume: 0.8 },
    ...keys([T.submit], [T.submit + 90]),
    { t: T.fix[0], sound: "success" },
  ]
  if (mode === "iphone") cues.push({ t: T.phoneClick, sound: "pulse" }, { t: T.phoneRecord, sound: "pulse" })
  else cues.push({ t: T.dragStart, sound: "press", volume: 0.7 }, { t: T.dragEnd, sound: "release", volume: 0.7 }, { t: T.record, sound: "pulse", volume: 0.7 })
  return cues.sort((a, b) => a.t - b.t)
}
