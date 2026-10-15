import { type RefObject, useEffect, useRef, useState } from "react"
import { AREA, clamp, easeOut, MAC_WINDOW, type Mode, PHONE, PILL, PILL_CY, pillWidth, progress, type Rect, type Script, STAGE, strokeAt, T } from "./script"
import { fixAmount } from "./stage-parts"

export type GpuBackend = "webgpu" | "webgl2" | "css" | "initializing"

export type GpuDiagnostics = {
  backend: GpuBackend
  canvasWidth: number
  canvasHeight: number
  dpr: number
  drawCalls: number
  avgRenderMs: number
  p95RenderMs: number
  activeEffects: string[]
  paused: boolean
  reducedMotion: boolean
  lastT: number
}

declare global {
  interface Window {
    __scrawlGpu?: () => GpuDiagnostics
    __scrawlForceFx?: "webgpu" | "webgl2" | "css"
  }
}

type CachedRects = {
  win: Rect
  card: Rect
  badge: Rect
  actions: Rect
  diff: Rect
  sheet: Rect
  prompt: Rect
}

const PARTICLE_COUNT = 96

function defaultRects(mode: Mode): CachedRects {
  const win = mode === "mac" ? MAC_WINDOW : PHONE
  const card =
    mode === "mac"
      ? { x: MAC_WINDOW.x + 178, y: MAC_WINDOW.y + 132, w: 482, h: 232 }
      : { x: PHONE.x + 18, y: PHONE.y + 92, w: PHONE.w - 36, h: 220 }
  return {
    win,
    card,
    badge: { x: card.x + 128, y: card.y + 14, w: 44, h: 20 },
    actions:
      mode === "mac"
        ? { x: card.x + card.w - 200, y: card.y + card.h - 42, w: 184, h: 28 }
        : { x: card.x + 16, y: card.y + card.h - 42, w: card.w - 32, h: 28 },
    diff: { x: 786, y: 208, w: 374, h: 126 },
    sheet: { x: 788, y: 68, w: 300, h: 98 },
    prompt: { x: 778, y: 360, w: 386, h: 74 },
  }
}

/* ---------- WGSL Shaders (WebGPU) ---------- */

const WGSL_SHADER = /* wgsl */ `
struct Uniforms {
  data: array<vec4<f32>, 16>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) stagePos: vec2<f32>,
};

@vertex
fn vsMain(@builtin(vertex_index) vid: u32) -> VsOut {
  var corners = array<vec2<f32>, 3>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(3.0, -1.0),
    vec2<f32>(-1.0, 3.0)
  );
  let ndc = corners[vid];
  let uv = vec2<f32>((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5);
  var out: VsOut;
  out.pos = vec4<f32>(ndc, 0.0, 1.0);
  out.stagePos = uv * u.data[0].xy;
  return out;
}

fn hash21(p: vec2<f32>) -> f32 {
  var q = fract(p * vec2<f32>(123.34, 456.21));
  q += dot(q, q + 45.32);
  return fract(q.x * q.y);
}

fn noise2(p: vec2<f32>) -> f32 {
  let i = floor(p);
  let f = fract(p);
  let u2 = f * f * (3.0 - 2.0 * f);
  let a = hash21(i);
  let b = hash21(i + vec2<f32>(1.0, 0.0));
  let c = hash21(i + vec2<f32>(0.0, 1.0));
  let d = hash21(i + vec2<f32>(1.0, 1.0));
  return mix(mix(a, b, u2.x), mix(c, d, u2.x), u2.y);
}

fn fbm(p: vec2<f32>) -> f32 {
  var v = 0.0;
  var a = 0.55;
  var q = p;
  for (var i = 0; i < 2; i = i + 1) {
    v += a * noise2(q);
    q = q * 2.07 + vec2<f32>(13.7, 9.2);
    a *= 0.45;
  }
  return v;
}

fn sdRoundBox(p: vec2<f32>, rect: vec4<f32>, r: f32) -> f32 {
  let halfSize = rect.zw * 0.5;
  let center = rect.xy + halfSize;
  let d = abs(p - center) - halfSize + vec2<f32>(r);
  return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

fn sdSegment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
  let pa = p - a;
  let ba = b - a;
  let h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0);
  return length(pa - ba * h);
}

@fragment
fn fsMain(in: VsOut) -> @location(0) vec4<f32> {
  let p = in.stagePos;
  let time = u.data[0].z;
  let reduced = u.data[0].w > 0.5;

  let winRect = u.data[1];
  let cardRect = u.data[2];
  let badgeNow = u.data[3];
  let badgeStart = u.data[4];
  let actionsNow = u.data[5];
  let diffRect = u.data[6];
  let sheetRect = u.data[7];
  let promptRect = u.data[8];
  let inkTip = u.data[9];
  let pin1 = u.data[10];
  let pin2 = u.data[11];
  let capWave = u.data[12];
  let fixWave = u.data[13];
  let extra = u.data[15];

  var col = vec3<f32>(0.0);
  var alpha = 0.0;

  // 1. Live Ink Nib Bleed & Warm Glow
  if (inkTip.z > 0.005) {
    let dTip = length(p - inkTip.xy);
    if (dTip < 64.0) {
      let n = select(fbm(p * 0.09 + vec2<f32>(time * 7.0, -time * 5.0)), 0.5, reduced);
      let warped = max(0.0, dTip - (n - 0.5) * 7.5);
      let core = exp(-warped * 0.16) * 0.68 * inkTip.z;
      let halo = exp(-dTip * 0.055) * 0.28 * inkTip.z;
      let inkCol = mix(vec3<f32>(1.0, 0.23, 0.19), vec3<f32>(1.0, 0.72, 0.45), clamp(core * 1.4, 0.0, 1.0));
      col += inkCol * (core + halo);
      alpha += (core + halo) * 0.55;
    }
  }

  // 2. Pin Drop Caustic Shockwave Rings (#1 and #2)
  for (var i = 0; i < 2; i = i + 1) {
    let pin = select(pin2, pin1, i == 0);
    if (pin.w > 0.005) {
      let prog = pin.z;
      let dp = p - pin.xy;
      let dist = length(dp);
      if (dist < 110.0) {
        let fade = (1.0 - prog) * (1.0 - prog);
        let rad = 8.0 + prog * 68.0;
        let n = select(noise2(dp * 0.08 + vec2<f32>(prog * 5.0)), 0.5, reduced);
        let ringDist = abs(dist - rad + (n - 0.5) * 4.5);
        let ring1 = exp(-ringDist * 0.32) * fade * 0.85;
        let ring2 = exp(-abs(dist - rad * 0.62) * 0.38) * fade * 0.45;
        let centerFlash = exp(-dist * 0.09) * max(0.0, 1.0 - prog * 2.6) * 0.75;
        let pinCol = mix(vec3<f32>(1.0, 0.27, 0.22), vec3<f32>(1.0, 0.84, 0.55), clamp(ring1 + centerFlash, 0.0, 1.0));
        let w = ring1 + ring2 + centerFlash;
        col += pinCol * w;
        alpha += w * 0.5;
      }
    }
  }

  // 3. Capture Shutter Wave & HUD "Copied" Emerald Aura
  if (capWave.x > 0.005) {
    let dWin = sdRoundBox(p, winRect, extra.x);
    if (dWin < 18.0) {
      let prog = capWave.x;
      let env = sin(prog * 3.14159265);
      let targetEdge = -prog * 140.0;
      let wave = exp(-abs(dWin - targetEdge) * 0.09) * env * 0.52;
      let rim = exp(-abs(dWin) * 0.22) * (1.0 - prog) * 0.75;
      let c = mix(vec3<f32>(0.35, 0.78, 0.98), vec3<f32>(1.0, 1.0, 1.0), 0.55);
      col += c * (wave + rim);
      alpha += (wave + rim) * 0.45;
    }
  }
  if (capWave.y > 0.005) {
    let hudRect = vec4<f32>(extra.y - extra.w * 0.5, extra.z - 20.0, extra.w, 40.0);
    let dHud = sdRoundBox(p, hudRect, 20.0);
    if (dHud < 48.0 && dHud > -16.0) {
      let env = sin(capWave.y * 3.14159265);
      let glow = exp(-abs(dHud - capWave.y * 14.0) * 0.14) * env * 0.75;
      col += vec3<f32>(0.19, 0.82, 0.35) * glow;
      alpha += glow * 0.48;
    }
  }

  // 4. Prompt Paste Amber Pulse & Contact Sheet Photo-Develop Sweep
  if (capWave.z > 0.005) {
    let dPrompt = sdRoundBox(p, promptRect, 4.0);
    if (dPrompt < 22.0 && dPrompt > -14.0) {
      let env = sin(capWave.z * 3.14159265);
      let rim = exp(-abs(dPrompt) * 0.24) * env * 0.58;
      let outer = exp(-max(0.0, dPrompt) * 0.12) * env * 0.22;
      let w = rim + outer;
      col += vec3<f32>(0.96, 0.65, 0.26) * w;
      alpha += w * 0.42;
    }
  }
  if (capWave.w > 0.005) {
    let dSheet = sdRoundBox(p, sheetRect, 6.0);
    if (dSheet < 14.0) {
      let prog = capWave.w;
      let mask = smoothstep(8.0, -1.0, dSheet);
      let rel = (p - sheetRect.xy) / max(sheetRect.zw, vec2<f32>(1.0));
      let sweepCoord = rel.x * 0.82 + rel.y * 0.35;
      let sweepCenter = -0.15 + prog * 1.45;
      let band = exp(-abs(sweepCoord - sweepCenter) * 10.5) * sin(prog * 3.14159265);
      let rim = exp(-abs(dSheet) * 0.26) * sin(prog * 3.14159265) * 0.45;
      let w = band * 0.48 * mask + rim;
      col += mix(vec3<f32>(0.45, 0.78, 1.0), vec3<f32>(1.0, 0.98, 0.94), 0.6) * w;
      alpha += w * 0.42;
    }
  }

  // 5. OpenCode TUI "Diff Applied" Shimmer Sweep
  if (fixWave.x > 0.005) {
    let dDiff = sdRoundBox(p, diffRect, 4.0);
    if (dDiff < 10.0) {
      let prog = fixWave.x;
      let mask = smoothstep(4.0, -1.0, dDiff);
      let rel = (p - diffRect.xy) / max(diffRect.zw, vec2<f32>(1.0));
      let diag = rel.x * 0.78 + rel.y * 0.42;
      let front = -0.18 + prog * 1.45;
      let beam = exp(-abs(diag - front) * 10.0) * sin(prog * 3.14159265);
      let leftBar = exp(-abs(p.x - diffRect.x) * 0.35) * mask * sin(prog * 3.14159265) * 0.55;
      let c = mix(vec3<f32>(0.19, 0.82, 0.35), vec3<f32>(0.36, 0.78, 0.98), clamp(rel.x, 0.0, 1.0));
      let w = beam * 0.42 * mask + leftBar;
      col += c * w;
      alpha += w * 0.4;
    }
  }

  // 6. THE PAYOFF: Window & DeployCard Refraction Light Sweep + Motion Trails + Lock-In Bloom
  if (fixWave.y > 0.005 || fixWave.z > 0.005 || fixWave.w > 0.005) {
    let dWin = sdRoundBox(p, winRect, extra.x);
    if (dWin < 20.0) {
      let winMask = smoothstep(10.0, -2.0, dWin);
      let dCard = sdRoundBox(p, cardRect, 12.0);
      let cardMask = smoothstep(14.0, -2.0, dCard);

      // 6a. Noise-displaced Caustic Refraction Wave rolling across DeployCard & Window
      if (fixWave.y > 0.005) {
        let prog = fixWave.y;
        let env = sin(prog * 3.14159265);
        let rel = (p - cardRect.xy) / max(cardRect.zw, vec2<f32>(1.0));
        let warp = select((fbm(p * 0.024 + vec2<f32>(prog * 2.8, -prog * 1.9)) - 0.5) * 0.14, 0.0, reduced);
        let coord = rel.x * 0.72 + rel.y * 0.42 + warp;
        let front = -0.22 + prog * 1.56;
        let dist = coord - front;

        // Chromatic dispersion: emerald leading edge, warm white crest, cyan/blue wake
        let leadR = exp(-abs(dist - 0.02) * 16.0) * env;
        let crestG = exp(-abs(dist) * 18.0) * env;
        let wakeB = exp(-abs(dist + 0.025) * 13.0) * env;
        let broadWake = exp(-max(0.0, -dist) * 5.0) * step(dist, 0.02) * env * 0.12;

        let sweepCol =
          vec3<f32>(0.19, 0.88, 0.48) * leadR * 0.34 +
          vec3<f32>(0.95, 0.99, 1.0) * crestG * 0.38 +
          vec3<f32>(0.08, 0.58, 1.0) * (wakeB * 0.36 + broadWake);

        // Card perimeter caustic rim wave (crisp luminous trace along the rounded border)
        let rimDist = abs(dCard);
        let rimWave = exp(-rimDist * 0.28) * exp(-abs(dist) * 5.5) * env * 0.92;
        let rimAfter = exp(-rimDist * 0.22) * env * 0.34;
        let rimCol = mix(vec3<f32>(0.19, 0.85, 0.42), vec3<f32>(0.35, 0.78, 0.98), clamp(rel.x, 0.0, 1.0));

        let maskCombined = mix(winMask * 0.18, 1.0, cardMask);
        let totalSweep = sweepCol * maskCombined + rimCol * (rimWave + rimAfter);
        let sweepLum = dot(totalSweep, vec3<f32>(0.333));
        col += totalSweep;
        alpha += sweepLum * 0.44;
      }

      // 6b. Luminous Motion-Trail Ribbons & Alignment Lock-In Rim/Halo on Badge & Action Row
      if (fixWave.z > 0.005 || fixWave.w > 0.005) {
        let trail = fixWave.z;
        let bloom = fixWave.w;

        // Badge capsule perimeter rim + outer halo + motion wake from broken start to current position
        let badgeHalf = badgeNow.zw * 0.5;
        let badgeCenterNow = badgeNow.xy + badgeHalf;
        let badgeCenterStart = badgeStart.xy + badgeHalf;
        let dBadgeBox = sdRoundBox(p, badgeNow, badgeHalf.y);
        let dBadgeWake = sdSegment(p, badgeCenterStart, badgeCenterNow) - badgeHalf.y * 0.75;
        let badgeOutside = smoothstep(-2.0, 2.0, dBadgeBox);

        let badgeRim = exp(-abs(dBadgeBox) * 0.42) * (trail * 0.55 + bloom * 0.85);
        let badgeHalo = exp(-max(0.0, dBadgeBox) * 0.14) * badgeOutside * (trail * 0.42 + bloom * 0.65);
        let badgeWakeGlow = exp(-max(0.0, dBadgeWake) * 0.13) * badgeOutside * trail * 0.48;

        // Actions buttons row perimeter rim + outer halo + upward motion wake
        let actRightRect = actionsNow;
        let dActBox = sdRoundBox(p, actRightRect, 8.0);
        let actOutside = smoothstep(-2.0, 2.0, dActBox);
        let actCenterNow = actRightRect.xy + actRightRect.zw * 0.5;
        let actCenterStart = vec2<f32>(actCenterNow.x, badgeStart.z + actRightRect.w * 0.5);
        let dActWake = sdSegment(p, actCenterStart, actCenterNow) - 10.0;

        let actRim = exp(-abs(dActBox) * 0.36) * (trail * 0.48 + bloom * 0.78);
        let actHalo = exp(-max(0.0, dActBox) * 0.12) * actOutside * (trail * 0.36 + bloom * 0.52);
        let actWakeGlow = exp(-max(0.0, dActWake) * 0.11) * actOutside * trail * 0.38;

        let emerald = vec3<f32>(0.19, 0.85, 0.42);
        let electricBlue = vec3<f32>(0.22, 0.68, 1.0);
        let bTotal = badgeRim + badgeHalo + badgeWakeGlow;
        let aTotal = actRim + actHalo + actWakeGlow;

        col += mix(emerald, vec3<f32>(0.85, 1.0, 0.92), bloom * 0.4) * bTotal;
        col += mix(electricBlue, emerald, 0.38) * aTotal;
        alpha += (bTotal + aTotal) * 0.44;
      }
    }
  }

  alpha = clamp(alpha, 0.0, 0.92);
  return vec4<f32>(col, alpha);
}

/* ---------- Pipeline 2: Instanced Quad Particles ---------- */

struct ParticleOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
  @location(1) color: vec4<f32>,
  @location(2) angle: f32,
};

fn hash11(n: f32) -> f32 {
  return fract(sin(n * 127.1 + 311.7) * 43758.5453123);
}

@vertex
fn vsParticle(@builtin(vertex_index) vid: u32, @builtin(instance_index) iid: u32) -> ParticleOut {
  var quad = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0),
    vec2<f32>(1.0, -1.0),
    vec2<f32>(-1.0, 1.0),
    vec2<f32>(-1.0, 1.0),
    vec2<f32>(1.0, -1.0),
    vec2<f32>(1.0, 1.0)
  );

  var out: ParticleOut;
  let stage = u.data[0].xy;
  let reduced = u.data[0].w > 0.5;
  if (reduced) {
    out.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(0.0);
    out.color = vec4<f32>(0.0);
    out.angle = 0.0;
    return out;
  }

  let fi = f32(iid);
  let r1 = hash11(fi * 1.13 + 0.7);
  let r2 = hash11(fi * 2.41 + 1.9);
  let r3 = hash11(fi * 3.89 + 4.3);
  let r4 = hash11(fi * 5.17 + 8.1);

  let badgeNow = u.data[3];
  let actionsNow = u.data[5];
  let inkTip = u.data[9];
  let pin1 = u.data[10];
  let pin2 = u.data[11];
  let pCtrl = u.data[14];

  var center = vec2<f32>(-100.0, -100.0);
  var radius = 0.0;
  var pCol = vec3<f32>(1.0);
  var pAlpha = 0.0;
  var spin = r4 * 6.28318;

  if (iid < 48u) {
    // Group A (0..47): Fix Payoff Alignment Sparkles on Badge & Action Row
    let prog = pCtrl.x;
    if (prog > 0.001 && prog < 0.999) {
      let delay = r1 * 0.36;
      let dur = 0.42 + r2 * 0.38;
      let localP = clamp((prog - delay) / dur, 0.0, 1.0);
      if (localP > 0.0 && localP < 1.0) {
        let isBadge = iid < 22u;
        let box = select(
          vec4<f32>(actionsNow.x - 4.0, actionsNow.y - 2.0, actionsNow.z + 8.0, actionsNow.w + 4.0),
          vec4<f32>(badgeNow.x - 4.0, badgeNow.y - 2.0, badgeNow.z + 8.0, badgeNow.w + 4.0),
          isBadge
        );
        // Spawn around perimeter of the aligned badge or action buttons
        let perim = fract(r1 * 3.7 + r2);
        var spawn = box.xy + vec2<f32>(r2 * box.z, r3 * box.w);
        if (perim < 0.5) {
          spawn.y = select(box.y + box.w, box.y, perim < 0.25);
        }
        let dirAngle = (r3 - 0.5) * 3.14159 - 1.57079;
        let speed = 14.0 + r4 * 36.0;
        let easeP = 1.0 - pow(1.0 - localP, 2.4);
        let drift = vec2<f32>(cos(dirAngle), sin(dirAngle)) * speed * easeP + vec2<f32>(sin(localP * 8.0 + r1 * 6.0) * 4.5, -localP * 10.0);
        center = spawn + drift;
        let env = sin(localP * 3.14159265);
        let twinkle = 0.65 + 0.35 * sin(localP * 22.0 + r4 * 12.0);
        radius = (5.5 + r2 * 8.5) * env;
        pAlpha = env * twinkle;
        spin += localP * 2.4;

        let emerald = vec3<f32>(0.25, 0.95, 0.52);
        let cyan = vec3<f32>(0.42, 0.86, 1.0);
        let gold = vec3<f32>(1.0, 0.92, 0.68);
        pCol = select(mix(cyan, gold, r3), mix(emerald, gold, r2 * 0.6), isBadge);
      }
    }
  } else if (iid < 72u) {
    // Group B (48..71): Pin Drop Radial Burst Sparks
    let isPin1 = iid < 60u;
    let prog = select(pCtrl.z, pCtrl.y, isPin1);
    let pin = select(pin2, pin1, isPin1);
    if (prog > 0.001 && prog < 0.999 && pin.w > 0.005) {
      let delay = r1 * 0.12;
      let localP = clamp((prog - delay) / (0.75 + r2 * 0.22), 0.0, 1.0);
      if (localP > 0.0 && localP < 1.0) {
        let ang = (f32(iid % 12u) / 12.0) * 6.28318 + (r1 - 0.5) * 0.45;
        let dist = (10.0 + r2 * 44.0) * (1.0 - pow(1.0 - localP, 2.8));
        center = pin.xy + vec2<f32>(cos(ang), sin(ang)) * dist + vec2<f32>(0.0, localP * localP * 8.0);
        let env = sin(localP * 3.14159265);
        radius = (4.5 + r3 * 6.0) * env;
        pAlpha = env * (1.0 - localP * 0.3);
        spin += localP * 3.0;
        pCol = mix(vec3<f32>(1.0, 0.34, 0.26), vec3<f32>(1.0, 0.88, 0.62), r2);
      }
    }
  } else {
    // Group C (72..95): Live Ink Nib Trailing Sparks
    if (inkTip.z > 0.01 && pCtrl.w >= 0.0) {
      let cycle = fract(pCtrl.w * 2.2 + r1);
      let ang = r2 * 6.28318;
      let spread = (2.0 + r3 * 16.0) * cycle;
      center = inkTip.xy + vec2<f32>(cos(ang) * spread, sin(ang) * spread - cycle * 12.0);
      let env = sin(cycle * 3.14159265);
      radius = (3.5 + r4 * 4.5) * env * inkTip.z;
      pAlpha = env * 0.85 * inkTip.z;
      pCol = mix(vec3<f32>(1.0, 0.28, 0.2), vec3<f32>(1.0, 0.82, 0.52), cycle);
    }
  }

  if (pAlpha <= 0.005 || radius <= 0.1) {
    out.pos = vec4<f32>(2.0, 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(0.0);
    out.color = vec4<f32>(0.0);
    out.angle = 0.0;
    return out;
  }

  let corner = quad[vid];
  let worldPos = center + corner * radius;
  let ndc = vec2<f32>(
    (worldPos.x / stage.x) * 2.0 - 1.0,
    1.0 - (worldPos.y / stage.y) * 2.0
  );
  out.pos = vec4<f32>(ndc, 0.0, 1.0);
  out.uv = corner;
  out.color = vec4<f32>(pCol, pAlpha);
  out.angle = spin;
  return out;
}

@fragment
fn fsParticle(in: ParticleOut) -> @location(0) vec4<f32> {
  let c = cos(in.angle);
  let s = sin(in.angle);
  let q = vec2<f32>(in.uv.x * c - in.uv.y * s, in.uv.x * s + in.uv.y * c);
  let d = length(q);
  if (d > 1.0) {
    discard;
  }
  // 4-point optical star glint + soft circular core
  let rayX = exp(-abs(q.y) * 7.5) * exp(-abs(q.x) * 1.8);
  let rayY = exp(-abs(q.x) * 7.5) * exp(-abs(q.y) * 1.8);
  let core = exp(-d * d * 6.5);
  let halo = max(0.0, 1.0 - d) * 0.35;
  let intensity = clamp(core * 1.1 + (rayX + rayY) * 0.75 + halo, 0.0, 1.4) * in.color.w;
  let rgb = mix(in.color.xyz, vec3<f32>(1.0, 1.0, 1.0), clamp(core * 0.75, 0.0, 1.0)) * intensity;
  return vec4<f32>(rgb, clamp(intensity * 0.45, 0.0, 0.9));
}
`

/* ---------- GLSL ES 3.00 Shaders (WebGL2 Fallback) ---------- */

const GLSL_FULLSCREEN_VS = /* glsl */ `#version 300 es
precision highp float;
uniform vec4 u_data[16];
out vec2 v_stagePos;
void main() {
  vec2 corners[3] = vec2[3](
    vec2(-1.0, -1.0),
    vec2(3.0, -1.0),
    vec2(-1.0, 3.0)
  );
  vec2 ndc = corners[gl_VertexID];
  vec2 uv = vec2((ndc.x + 1.0) * 0.5, (1.0 - ndc.y) * 0.5);
  gl_Position = vec4(ndc, 0.0, 1.0);
  v_stagePos = uv * u_data[0].xy;
}
`

const GLSL_FULLSCREEN_FS = /* glsl */ `#version 300 es
precision highp float;
uniform vec4 u_data[16];
in vec2 v_stagePos;
out vec4 fragColor;

float hash21(vec2 p) {
  vec2 q = fract(p * vec2(123.34, 456.21));
  q += dot(q, q + 45.32);
  return fract(q.x * q.y);
}

float noise2(vec2 p) {
  vec2 i = floor(p);
  vec2 f = fract(p);
  vec2 u2 = f * f * (3.0 - 2.0 * f);
  float a = hash21(i);
  float b = hash21(i + vec2(1.0, 0.0));
  float c = hash21(i + vec2(0.0, 1.0));
  float d = hash21(i + vec2(1.0, 1.0));
  return mix(mix(a, b, u2.x), mix(c, d, u2.x), u2.y);
}

float fbm(vec2 p) {
  float v = 0.0;
  float a = 0.55;
  vec2 q = p;
  for (int i = 0; i < 2; i++) {
    v += a * noise2(q);
    q = q * 2.07 + vec2(13.7, 9.2);
    a *= 0.45;
  }
  return v;
}

float sdRoundBox(vec2 p, vec4 rect, float r) {
  vec2 halfSize = rect.zw * 0.5;
  vec2 center = rect.xy + halfSize;
  vec2 d = abs(p - center) - halfSize + vec2(r);
  return length(max(d, vec2(0.0))) + min(max(d.x, d.y), 0.0) - r;
}

float sdSegment(vec2 p, vec2 a, vec2 b) {
  vec2 pa = p - a;
  vec2 ba = b - a;
  float h = clamp(dot(pa, ba) / max(dot(ba, ba), 0.0001), 0.0, 1.0);
  return length(pa - ba * h);
}

void main() {
  vec2 p = v_stagePos;
  float time = u_data[0].z;
  bool reduced = u_data[0].w > 0.5;

  vec4 winRect = u_data[1];
  vec4 cardRect = u_data[2];
  vec4 badgeNow = u_data[3];
  vec4 badgeStart = u_data[4];
  vec4 actionsNow = u_data[5];
  vec4 diffRect = u_data[6];
  vec4 sheetRect = u_data[7];
  vec4 promptRect = u_data[8];
  vec4 inkTip = u_data[9];
  vec4 pin1 = u_data[10];
  vec4 pin2 = u_data[11];
  vec4 capWave = u_data[12];
  vec4 fixWave = u_data[13];
  vec4 extra = u_data[15];

  vec3 col = vec3(0.0);
  float alpha = 0.0;

  if (inkTip.z > 0.005) {
    float dTip = length(p - inkTip.xy);
    if (dTip < 64.0) {
      float n = reduced ? 0.5 : fbm(p * 0.09 + vec2(time * 7.0, -time * 5.0));
      float warped = max(0.0, dTip - (n - 0.5) * 7.5);
      float core = exp(-warped * 0.16) * 0.68 * inkTip.z;
      float halo = exp(-dTip * 0.055) * 0.28 * inkTip.z;
      vec3 inkCol = mix(vec3(1.0, 0.23, 0.19), vec3(1.0, 0.72, 0.45), clamp(core * 1.4, 0.0, 1.0));
      col += inkCol * (core + halo);
      alpha += (core + halo) * 0.55;
    }
  }

  for (int i = 0; i < 2; i++) {
    vec4 pin = (i == 0) ? pin1 : pin2;
    if (pin.w > 0.005) {
      float prog = pin.z;
      vec2 dp = p - pin.xy;
      float dist = length(dp);
      if (dist < 110.0) {
        float fade = (1.0 - prog) * (1.0 - prog);
        float rad = 8.0 + prog * 68.0;
        float n = reduced ? 0.5 : noise2(dp * 0.08 + vec2(prog * 5.0));
        float ringDist = abs(dist - rad + (n - 0.5) * 4.5);
        float ring1 = exp(-ringDist * 0.32) * fade * 0.85;
        float ring2 = exp(-abs(dist - rad * 0.62) * 0.38) * fade * 0.45;
        float centerFlash = exp(-dist * 0.09) * max(0.0, 1.0 - prog * 2.6) * 0.75;
        vec3 pinCol = mix(vec3(1.0, 0.27, 0.22), vec3(1.0, 0.84, 0.55), clamp(ring1 + centerFlash, 0.0, 1.0));
        float w = ring1 + ring2 + centerFlash;
        col += pinCol * w;
        alpha += w * 0.5;
      }
    }
  }

  if (capWave.x > 0.005) {
    float dWin = sdRoundBox(p, winRect, extra.x);
    if (dWin < 18.0) {
      float prog = capWave.x;
      float env = sin(prog * 3.14159265);
      float targetEdge = -prog * 140.0;
      float wave = exp(-abs(dWin - targetEdge) * 0.09) * env * 0.52;
      float rim = exp(-abs(dWin) * 0.22) * (1.0 - prog) * 0.75;
      vec3 c = mix(vec3(0.35, 0.78, 0.98), vec3(1.0, 1.0, 1.0), 0.55);
      col += c * (wave + rim);
      alpha += (wave + rim) * 0.45;
    }
  }
  if (capWave.y > 0.005) {
    vec4 hudRect = vec4(extra.y - extra.w * 0.5, extra.z - 20.0, extra.w, 40.0);
    float dHud = sdRoundBox(p, hudRect, 20.0);
    if (dHud < 48.0 && dHud > -16.0) {
      float env = sin(capWave.y * 3.14159265);
      float glow = exp(-abs(dHud - capWave.y * 14.0) * 0.14) * env * 0.75;
      col += vec3(0.19, 0.82, 0.35) * glow;
      alpha += glow * 0.48;
    }
  }

  if (capWave.z > 0.005) {
    float dPrompt = sdRoundBox(p, promptRect, 4.0);
    if (dPrompt < 22.0 && dPrompt > -14.0) {
      float env = sin(capWave.z * 3.14159265);
      float rim = exp(-abs(dPrompt) * 0.24) * env * 0.58;
      float outer = exp(-max(0.0, dPrompt) * 0.12) * env * 0.22;
      float w = rim + outer;
      col += vec3(0.96, 0.65, 0.26) * w;
      alpha += w * 0.42;
    }
  }
  if (capWave.w > 0.005) {
    float dSheet = sdRoundBox(p, sheetRect, 6.0);
    if (dSheet < 14.0) {
      float prog = capWave.w;
      float mask = smoothstep(8.0, -1.0, dSheet);
      vec2 rel = (p - sheetRect.xy) / max(sheetRect.zw, vec2(1.0));
      float sweepCoord = rel.x * 0.82 + rel.y * 0.35;
      float sweepCenter = -0.15 + prog * 1.45;
      float band = exp(-abs(sweepCoord - sweepCenter) * 10.5) * sin(prog * 3.14159265);
      float rim = exp(-abs(dSheet) * 0.26) * sin(prog * 3.14159265) * 0.45;
      float w = band * 0.48 * mask + rim;
      col += mix(vec3(0.45, 0.78, 1.0), vec3(1.0, 0.98, 0.94), 0.6) * w;
      alpha += w * 0.42;
    }
  }

  if (fixWave.x > 0.005) {
    float dDiff = sdRoundBox(p, diffRect, 4.0);
    if (dDiff < 10.0) {
      float prog = fixWave.x;
      float mask = smoothstep(4.0, -1.0, dDiff);
      vec2 rel = (p - diffRect.xy) / max(diffRect.zw, vec2(1.0));
      float diag = rel.x * 0.78 + rel.y * 0.42;
      float front = -0.18 + prog * 1.45;
      float beam = exp(-abs(diag - front) * 10.0) * sin(prog * 3.14159265);
      float leftBar = exp(-abs(p.x - diffRect.x) * 0.35) * mask * sin(prog * 3.14159265) * 0.55;
      vec3 c = mix(vec3(0.19, 0.82, 0.35), vec3(0.36, 0.78, 0.98), clamp(rel.x, 0.0, 1.0));
      float w = beam * 0.42 * mask + leftBar;
      col += c * w;
      alpha += w * 0.4;
    }
  }

  if (fixWave.y > 0.005 || fixWave.z > 0.005 || fixWave.w > 0.005) {
    float dWin = sdRoundBox(p, winRect, extra.x);
    if (dWin < 20.0) {
      float winMask = smoothstep(10.0, -2.0, dWin);
      float dCard = sdRoundBox(p, cardRect, 12.0);
      float cardMask = smoothstep(14.0, -2.0, dCard);

      if (fixWave.y > 0.005) {
        float prog = fixWave.y;
        float env = sin(prog * 3.14159265);
        vec2 rel = (p - cardRect.xy) / max(cardRect.zw, vec2(1.0));
        float warp = reduced ? 0.0 : (fbm(p * 0.024 + vec2(prog * 2.8, -prog * 1.9)) - 0.5) * 0.14;
        float coord = rel.x * 0.72 + rel.y * 0.42 + warp;
        float front = -0.22 + prog * 1.56;
        float dist = coord - front;

        float leadR = exp(-abs(dist - 0.02) * 16.0) * env;
        float crestG = exp(-abs(dist) * 18.0) * env;
        float wakeB = exp(-abs(dist + 0.025) * 13.0) * env;
        float broadWake = exp(-max(0.0, -dist) * 5.0) * step(dist, 0.02) * env * 0.12;

        vec3 sweepCol =
          vec3(0.19, 0.88, 0.48) * leadR * 0.34 +
          vec3(0.95, 0.99, 1.0) * crestG * 0.38 +
          vec3(0.08, 0.58, 1.0) * (wakeB * 0.36 + broadWake);

        float rimDist = abs(dCard);
        float rimWave = exp(-rimDist * 0.28) * exp(-abs(dist) * 5.5) * env * 0.92;
        float rimAfter = exp(-rimDist * 0.22) * env * 0.34;
        vec3 rimCol = mix(vec3(0.19, 0.85, 0.42), vec3(0.35, 0.78, 0.98), clamp(rel.x, 0.0, 1.0));

        float maskCombined = mix(winMask * 0.18, 1.0, cardMask);
        vec3 totalSweep = sweepCol * maskCombined + rimCol * (rimWave + rimAfter);
        float sweepLum = dot(totalSweep, vec3(0.333));
        col += totalSweep;
        alpha += sweepLum * 0.44;
      }

      if (fixWave.z > 0.005 || fixWave.w > 0.005) {
        float trail = fixWave.z;
        float bloom = fixWave.w;

        vec2 badgeHalf = badgeNow.zw * 0.5;
        vec2 badgeCenterNow = badgeNow.xy + badgeHalf;
        vec2 badgeCenterStart = badgeStart.xy + badgeHalf;
        float dBadgeBox = sdRoundBox(p, badgeNow, badgeHalf.y);
        float dBadgeWake = sdSegment(p, badgeCenterStart, badgeCenterNow) - badgeHalf.y * 0.75;
        float badgeOutside = smoothstep(-2.0, 2.0, dBadgeBox);

        float badgeRim = exp(-abs(dBadgeBox) * 0.42) * (trail * 0.55 + bloom * 0.85);
        float badgeHalo = exp(-max(0.0, dBadgeBox) * 0.14) * badgeOutside * (trail * 0.42 + bloom * 0.65);
        float badgeWakeGlow = exp(-max(0.0, dBadgeWake) * 0.13) * badgeOutside * trail * 0.48;

        vec4 actRightRect = actionsNow;
        float dActBox = sdRoundBox(p, actRightRect, 8.0);
        float actOutside = smoothstep(-2.0, 2.0, dActBox);
        vec2 actCenterNow = actRightRect.xy + actRightRect.zw * 0.5;
        vec2 actCenterStart = vec2(actCenterNow.x, badgeStart.z + actRightRect.w * 0.5);
        float dActWake = sdSegment(p, actCenterStart, actCenterNow) - 10.0;

        float actRim = exp(-abs(dActBox) * 0.36) * (trail * 0.48 + bloom * 0.78);
        float actHalo = exp(-max(0.0, dActBox) * 0.12) * actOutside * (trail * 0.36 + bloom * 0.52);
        float actWakeGlow = exp(-max(0.0, dActWake) * 0.11) * actOutside * trail * 0.38;

        vec3 emerald = vec3(0.19, 0.85, 0.42);
        vec3 electricBlue = vec3(0.22, 0.68, 1.0);
        float bTotal = badgeRim + badgeHalo + badgeWakeGlow;
        float aTotal = actRim + actHalo + actWakeGlow;

        col += mix(emerald, vec3(0.85, 1.0, 0.92), bloom * 0.4) * bTotal;
        col += mix(electricBlue, emerald, 0.38) * aTotal;
        alpha += (bTotal + aTotal) * 0.44;
      }
    }
  }

  alpha = clamp(alpha, 0.0, 0.92);
  fragColor = vec4(col, alpha);
}
`

const GLSL_PARTICLE_VS = /* glsl */ `#version 300 es
precision highp float;
uniform vec4 u_data[16];
out vec2 v_uv;
out vec4 v_color;
out float v_angle;

float hash11(float n) {
  return fract(sin(n * 127.1 + 311.7) * 43758.5453123);
}

void main() {
  vec2 quad[6] = vec2[6](
    vec2(-1.0, -1.0),
    vec2(1.0, -1.0),
    vec2(-1.0, 1.0),
    vec2(-1.0, 1.0),
    vec2(1.0, -1.0),
    vec2(1.0, 1.0)
  );

  vec2 stage = u_data[0].xy;
  bool reduced = u_data[0].w > 0.5;
  if (reduced) {
    gl_Position = vec4(2.0, 2.0, 0.0, 1.0);
    v_uv = vec2(0.0);
    v_color = vec4(0.0);
    v_angle = 0.0;
    return;
  }

  int iid = gl_InstanceID;
  float fi = float(iid);
  float r1 = hash11(fi * 1.13 + 0.7);
  float r2 = hash11(fi * 2.41 + 1.9);
  float r3 = hash11(fi * 3.89 + 4.3);
  float r4 = hash11(fi * 5.17 + 8.1);

  vec4 badgeNow = u_data[3];
  vec4 actionsNow = u_data[5];
  vec4 inkTip = u_data[9];
  vec4 pin1 = u_data[10];
  vec4 pin2 = u_data[11];
  vec4 pCtrl = u_data[14];

  vec2 center = vec2(-100.0, -100.0);
  float radius = 0.0;
  vec3 pCol = vec3(1.0);
  float pAlpha = 0.0;
  float spin = r4 * 6.28318;

  if (iid < 48) {
    float prog = pCtrl.x;
    if (prog > 0.001 && prog < 0.999) {
      float delay = r1 * 0.36;
      float dur = 0.42 + r2 * 0.38;
      float localP = clamp((prog - delay) / dur, 0.0, 1.0);
      if (localP > 0.0 && localP < 1.0) {
        bool isBadge = iid < 22;
        vec4 box = isBadge
          ? vec4(badgeNow.x - 4.0, badgeNow.y - 2.0, badgeNow.z + 8.0, badgeNow.w + 4.0)
          : vec4(actionsNow.x - 4.0, actionsNow.y - 2.0, actionsNow.z + 8.0, actionsNow.w + 4.0);
        float perim = fract(r1 * 3.7 + r2);
        vec2 spawn = box.xy + vec2(r2 * box.z, r3 * box.w);
        if (perim < 0.5) {
          spawn.y = (perim < 0.25) ? box.y : (box.y + box.w);
        }
        float dirAngle = (r3 - 0.5) * 3.14159 - 1.57079;
        float speed = 14.0 + r4 * 36.0;
        float easeP = 1.0 - pow(1.0 - localP, 2.4);
        vec2 drift = vec2(cos(dirAngle), sin(dirAngle)) * speed * easeP + vec2(sin(localP * 8.0 + r1 * 6.0) * 4.5, -localP * 10.0);
        center = spawn + drift;
        float env = sin(localP * 3.14159265);
        float twinkle = 0.65 + 0.35 * sin(localP * 22.0 + r4 * 12.0);
        radius = (5.5 + r2 * 8.5) * env;
        pAlpha = env * twinkle;
        spin += localP * 2.4;

        vec3 emerald = vec3(0.25, 0.95, 0.52);
        vec3 cyan = vec3(0.42, 0.86, 1.0);
        vec3 gold = vec3(1.0, 0.92, 0.68);
        pCol = isBadge ? mix(emerald, gold, r2 * 0.6) : mix(cyan, gold, r3);
      }
    }
  } else if (iid < 72) {
    bool isPin1 = iid < 60;
    float prog = isPin1 ? pCtrl.y : pCtrl.z;
    vec4 pin = isPin1 ? pin1 : pin2;
    if (prog > 0.001 && prog < 0.999 && pin.w > 0.005) {
      float delay = r1 * 0.12;
      float localP = clamp((prog - delay) / (0.75 + r2 * 0.22), 0.0, 1.0);
      if (localP > 0.0 && localP < 1.0) {
        float ang = (float(iid % 12) / 12.0) * 6.28318 + (r1 - 0.5) * 0.45;
        float dist = (10.0 + r2 * 44.0) * (1.0 - pow(1.0 - localP, 2.8));
        center = pin.xy + vec2(cos(ang), sin(ang)) * dist + vec2(0.0, localP * localP * 8.0);
        float env = sin(localP * 3.14159265);
        radius = (4.5 + r3 * 6.0) * env;
        pAlpha = env * (1.0 - localP * 0.3);
        spin += localP * 3.0;
        pCol = mix(vec3(1.0, 0.34, 0.26), vec3(1.0, 0.88, 0.62), r2);
      }
    }
  } else {
    if (inkTip.z > 0.01 && pCtrl.w >= 0.0) {
      float cycle = fract(pCtrl.w * 2.2 + r1);
      float ang = r2 * 6.28318;
      float spread = (2.0 + r3 * 16.0) * cycle;
      center = inkTip.xy + vec2(cos(ang) * spread, sin(ang) * spread - cycle * 12.0);
      float env = sin(cycle * 3.14159265);
      radius = (3.5 + r4 * 4.5) * env * inkTip.z;
      pAlpha = env * 0.85 * inkTip.z;
      pCol = mix(vec3(1.0, 0.28, 0.2), vec3(1.0, 0.82, 0.52), cycle);
    }
  }

  if (pAlpha <= 0.005 || radius <= 0.1) {
    gl_Position = vec4(2.0, 2.0, 0.0, 1.0);
    v_uv = vec2(0.0);
    v_color = vec4(0.0);
    v_angle = 0.0;
    return;
  }

  vec2 corner = quad[gl_VertexID];
  vec2 worldPos = center + corner * radius;
  vec2 ndc = vec2(
    (worldPos.x / stage.x) * 2.0 - 1.0,
    1.0 - (worldPos.y / stage.y) * 2.0
  );
  gl_Position = vec4(ndc, 0.0, 1.0);
  v_uv = corner;
  v_color = vec4(pCol, pAlpha);
  v_angle = spin;
}
`

const GLSL_PARTICLE_FS = /* glsl */ `#version 300 es
precision highp float;
in vec2 v_uv;
in vec4 v_color;
in float v_angle;
out vec4 fragColor;

void main() {
  float c = cos(v_angle);
  float s = sin(v_angle);
  vec2 q = vec2(v_uv.x * c - v_uv.y * s, v_uv.x * s + v_uv.y * c);
  float d = length(q);
  if (d > 1.0) discard;
  float rayX = exp(-abs(q.y) * 7.5) * exp(-abs(q.x) * 1.8);
  float rayY = exp(-abs(q.x) * 7.5) * exp(-abs(q.y) * 1.8);
  float core = exp(-d * d * 6.5);
  float halo = max(0.0, 1.0 - d) * 0.35;
  float intensity = clamp(core * 1.1 + (rayX + rayY) * 0.75 + halo, 0.0, 1.4) * v_color.w;
  vec3 rgb = mix(v_color.xyz, vec3(1.0), clamp(core * 0.75, 0.0, 1.0)) * intensity;
  fragColor = vec4(rgb, clamp(intensity * 0.45, 0.0, 0.9));
}
`

/* ---------- Renderer Abstractions ---------- */

interface GpuRenderer {
  backend: "webgpu" | "webgl2"
  render(uniforms: Float32Array, hasParticles: boolean): void
  clear(): void
  dispose(): void
}

async function createWebGpuRenderer(canvas: HTMLCanvasElement): Promise<GpuRenderer | null> {
  if (typeof navigator === "undefined" || !("gpu" in navigator)) return null
  try {
    const gpu = (navigator as Navigator & { gpu: GPU }).gpu
    const adapter = await gpu.requestAdapter()
    if (!adapter) return null
    const device = await adapter.requestDevice()
    const ctx = canvas.getContext("webgpu") as GPUCanvasContext | null
    if (!ctx) return null

    const format = gpu.getPreferredCanvasFormat()
    ctx.configure({
      device,
      format,
      alphaMode: "premultiplied",
    })

    const module = device.createShaderModule({ code: WGSL_SHADER })
    // GPUBufferUsage.UNIFORM (0x0040) | GPUBufferUsage.COPY_DST (0x0008)
    const uniformBuffer = device.createBuffer({
      size: 64 * 4,
      usage: 0x0040 | 0x0008,
    })

    // GPUShaderStage.VERTEX (0x1) | GPUShaderStage.FRAGMENT (0x2)
    const bindGroupLayout = device.createBindGroupLayout({
      entries: [
        {
          binding: 0,
          visibility: 0x1 | 0x2,
          buffer: { type: "uniform" },
        },
      ],
    })
    const pipelineLayout = device.createPipelineLayout({
      bindGroupLayouts: [bindGroupLayout],
    })
    const bindGroup = device.createBindGroup({
      layout: bindGroupLayout,
      entries: [{ binding: 0, resource: { buffer: uniformBuffer } }],
    })

    const blend: GPUBlendState = {
      color: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
      alpha: { srcFactor: "one", dstFactor: "one-minus-src-alpha", operation: "add" },
    }

    const fullscreenPipeline = device.createRenderPipeline({
      layout: pipelineLayout,
      vertex: { module, entryPoint: "vsMain" },
      fragment: { module, entryPoint: "fsMain", targets: [{ format, blend }] },
      primitive: { topology: "triangle-list" },
    })

    const particlePipeline = device.createRenderPipeline({
      layout: pipelineLayout,
      vertex: { module, entryPoint: "vsParticle" },
      fragment: { module, entryPoint: "fsParticle", targets: [{ format, blend }] },
      primitive: { topology: "triangle-list" },
    })

    return {
      backend: "webgpu",
      render(uniforms, hasParticles) {
        device.queue.writeBuffer(uniformBuffer, 0, uniforms.buffer, uniforms.byteOffset, uniforms.byteLength)
        const encoder = device.createCommandEncoder()
        const view = ctx.getCurrentTexture().createView()
        const pass = encoder.beginRenderPass({
          colorAttachments: [
            {
              view,
              clearValue: { r: 0, g: 0, b: 0, a: 0 },
              loadOp: "clear",
              storeOp: "store",
            },
          ],
        })
        pass.setBindGroup(0, bindGroup)
        pass.setPipeline(fullscreenPipeline)
        pass.draw(3, 1, 0, 0)
        if (hasParticles) {
          pass.setPipeline(particlePipeline)
          pass.draw(6, PARTICLE_COUNT, 0, 0)
        }
        pass.end()
        device.queue.submit([encoder.finish()])
      },
      clear() {
        const encoder = device.createCommandEncoder()
        const view = ctx.getCurrentTexture().createView()
        const pass = encoder.beginRenderPass({
          colorAttachments: [
            {
              view,
              clearValue: { r: 0, g: 0, b: 0, a: 0 },
              loadOp: "clear",
              storeOp: "store",
            },
          ],
        })
        pass.end()
        device.queue.submit([encoder.finish()])
      },
      dispose() {
        try {
          uniformBuffer.destroy()
          device.destroy()
        } catch {
          // ignore
        }
      },
    }
  } catch {
    return null
  }
}

function createWebGl2Renderer(canvas: HTMLCanvasElement): GpuRenderer | null {
  try {
    const gl = canvas.getContext("webgl2", {
      alpha: true,
      premultipliedAlpha: true,
      antialias: false,
      depth: false,
      stencil: false,
      powerPreference: "low-power",
    })
    if (!gl) return null

    const compile = (type: number, src: string) => {
      const sh = gl.createShader(type)!
      gl.shaderSource(sh, src)
      gl.compileShader(sh)
      if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
        gl.deleteShader(sh)
        return null
      }
      return sh
    }

    const link = (vsSrc: string, fsSrc: string) => {
      const vs = compile(gl.VERTEX_SHADER, vsSrc)
      const fs = compile(gl.FRAGMENT_SHADER, fsSrc)
      if (!vs || !fs) return null
      const prog = gl.createProgram()!
      gl.attachShader(prog, vs)
      gl.attachShader(prog, fs)
      gl.linkProgram(prog)
      gl.deleteShader(vs)
      gl.deleteShader(fs)
      if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
        gl.deleteProgram(prog)
        return null
      }
      return prog
    }

    const fsProg = link(GLSL_FULLSCREEN_VS, GLSL_FULLSCREEN_FS)
    const ptProg = link(GLSL_PARTICLE_VS, GLSL_PARTICLE_FS)
    if (!fsProg || !ptProg) return null

    const fsUniLoc = gl.getUniformLocation(fsProg, "u_data")
    const ptUniLoc = gl.getUniformLocation(ptProg, "u_data")
    const vao = gl.createVertexArray()

    gl.enable(gl.BLEND)
    gl.blendFunc(gl.ONE, gl.ONE_MINUS_SRC_ALPHA)

    return {
      backend: "webgl2",
      render(uniforms, hasParticles) {
        gl.viewport(0, 0, canvas.width, canvas.height)
        gl.clearColor(0, 0, 0, 0)
        gl.clear(gl.COLOR_BUFFER_BIT)
        gl.bindVertexArray(vao)

        gl.useProgram(fsProg)
        gl.uniform4fv(fsUniLoc, uniforms)
        gl.drawArrays(gl.TRIANGLES, 0, 3)

        if (hasParticles) {
          gl.useProgram(ptProg)
          gl.uniform4fv(ptUniLoc, uniforms)
          gl.drawArraysInstanced(gl.TRIANGLES, 0, 6, PARTICLE_COUNT)
        }
      },
      clear() {
        gl.viewport(0, 0, canvas.width, canvas.height)
        gl.clearColor(0, 0, 0, 0)
        gl.clear(gl.COLOR_BUFFER_BIT)
      },
      dispose() {
        try {
          gl.deleteProgram(fsProg)
          gl.deleteProgram(ptProg)
          if (vao) gl.deleteVertexArray(vao)
        } catch {
          // ignore
        }
      },
    }
  } catch {
    return null
  }
}

function getForcedBackend(): "webgpu" | "webgl2" | "css" | null {
  if (typeof window === "undefined") return null
  if (window.__scrawlForceFx) return window.__scrawlForceFx
  try {
    const q = new URLSearchParams(window.location.search).get("fx")
    if (q === "webgpu" || q === "webgl2" || q === "css") return q
  } catch {
    // ignore
  }
  return null
}

/* ---------- React Overlay Component ---------- */

export function StageGpuOverlay({
  t,
  mode,
  script,
  userStrokes,
  stageRef,
  wrapRef,
}: {
  t: number
  mode: Mode
  script: Script | null
  userStrokes: { points: [number, number, number][]; done: boolean }[]
  stageRef: RefObject<HTMLDivElement | null>
  wrapRef: RefObject<HTMLDivElement | null>
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  const rendererRef = useRef<GpuRenderer | null>(null)
  const [backend, setBackend] = useState<GpuBackend>("initializing")
  const rectsRef = useRef<CachedRects>(defaultRects(mode))
  const uniformsRef = useRef(new Float32Array(64))
  const wasClearedRef = useRef(false)
  const visibleRef = useRef(true)
  const reducedMotionRef = useRef(false)
  const renderSamplesRef = useRef<number[]>([])
  const diagRef = useRef<GpuDiagnostics>({
    backend: "initializing",
    canvasWidth: 0,
    canvasHeight: 0,
    dpr: 1,
    drawCalls: 0,
    avgRenderMs: 0,
    p95RenderMs: 0,
    activeEffects: [],
    paused: false,
    reducedMotion: false,
    lastT: 0,
  })

  useEffect(() => {
    window.__scrawlGpu = () => diagRef.current
  }, [])

  // Measure and cache DOM rects in stage space (1200x720) only when layout/mode/terminal steps change.
  const hasDiff = t >= T.diff
  const hasSubmitted = t >= T.submit
  useEffect(() => {
    const stageEl = stageRef.current
    if (!stageEl) return

    const updateRects = () => {
      const base = stageEl.getBoundingClientRect()
      const s = base.width / STAGE.w
      if (s <= 0) return
      const fallback = defaultRects(mode)
      const measure = (sel: string, fb: Rect): Rect => {
        const el = stageEl.querySelector(sel)
        if (!el) return fb
        const r = el.getBoundingClientRect()
        if (r.width <= 0 || r.height <= 0) return fb
        return {
          x: (r.left - base.left) / s,
          y: (r.top - base.top) / s,
          w: r.width / s,
          h: r.height / s,
        }
      }

      const { fix, rowFix } = fixAmount(t)
      const badgeX = mode === "iphone" ? -46 : -60
      const curBadgeDx = badgeX * (1 - fix)
      const curBadgeDy = -8 * (1 - fix)
      const curActionsDy = 14 * (1 - fix)
      const curRowGrow = (mode === "iphone" ? 32 : 44) * rowFix

      const rawCard = measure(".card", fallback.card)
      const rawBadge = measure(".card .badge", fallback.badge)
      const measureButtons = (fb: Rect): Rect => {
        const inner = stageEl.querySelector(".card .actions-inner")
        if (!inner || !inner.firstElementChild || !inner.lastElementChild) return measure(".card .actions-inner", fb)
        const r1 = inner.firstElementChild.getBoundingClientRect()
        const r2 = inner.lastElementChild.getBoundingClientRect()
        if (r1.width <= 0 || r2.width <= 0) return fb
        const left = Math.min(r1.left, r2.left)
        const top = Math.min(r1.top, r2.top)
        const right = Math.max(r1.right, r2.right)
        const bottom = Math.max(r1.bottom, r2.bottom)
        return {
          x: (left - base.left) / s,
          y: (top - base.top) / s,
          w: (right - left) / s,
          h: (bottom - top) / s,
        }
      }
      const rawActions = measureButtons(fallback.actions)

      rectsRef.current = {
        win: measure(mode === "mac" ? ".win" : ".phone", fallback.win),
        card: {
          x: rawCard.x,
          y: rawCard.y,
          w: rawCard.w,
          h: rawCard.h - curRowGrow,
        },
        // Store the un-shifted aligned rect (fix = 1, rowFix = 0) so each frame can apply exact fix offsets without querying DOM.
        badge: {
          x: rawBadge.x - curBadgeDx,
          y: rawBadge.y - curBadgeDy,
          w: rawBadge.w,
          h: rawBadge.h,
        },
        actions: {
          x: rawActions.x,
          y: rawActions.y - curActionsDy - curRowGrow,
          w: rawActions.w,
          h: rawActions.h,
        },
        diff: measure(".tdiff", fallback.diff),
        sheet: measure(".tu-images .sheet", fallback.sheet),
        prompt: measure(".tp-box", fallback.prompt),
      }
    }

    updateRects()
    const ro = new ResizeObserver(updateRects)
    ro.observe(stageEl)
    return () => ro.disconnect()
  }, [mode, hasDiff, hasSubmitted, stageRef])

  // Track reduced-motion and offscreen/visibility pause state.
  useEffect(() => {
    const mq = window.matchMedia?.("(prefers-reduced-motion: reduce)")
    const onMq = () => {
      reducedMotionRef.current = !!mq?.matches
      diagRef.current.reducedMotion = reducedMotionRef.current
    }
    onMq()
    mq?.addEventListener?.("change", onMq)

    const wrapEl = wrapRef.current
    const io =
      wrapEl && typeof IntersectionObserver !== "undefined"
        ? new IntersectionObserver(
            ([entry]) => {
              visibleRef.current = entry.isIntersecting
              diagRef.current.paused = !visibleRef.current || document.hidden
            },
            { threshold: 0.01 },
          )
        : null
    if (wrapEl && io) io.observe(wrapEl)

    const onVis = () => {
      diagRef.current.paused = !visibleRef.current || document.hidden
    }
    document.addEventListener("visibilitychange", onVis)

    return () => {
      mq?.removeEventListener?.("change", onMq)
      io?.disconnect()
      document.removeEventListener("visibilitychange", onVis)
    }
  }, [wrapRef])

  // Initialize WebGPU -> WebGL2 -> CSS fallback.
  useEffect(() => {
    let cancelled = false
    const canvas = canvasRef.current
    if (!canvas) return

    const init = async () => {
      const forced = getForcedBackend()
      if (forced === "css") {
        if (!cancelled) {
          setBackend("css")
          diagRef.current.backend = "css"
        }
        return
      }

      let renderer: GpuRenderer | null = null
      if (forced !== "webgl2") {
        renderer = await createWebGpuRenderer(canvas)
      }
      if (!renderer && forced !== "webgpu") {
        renderer = createWebGl2Renderer(canvas)
      }

      if (cancelled) {
        renderer?.dispose()
        return
      }

      if (renderer) {
        rendererRef.current = renderer
        setBackend(renderer.backend)
        diagRef.current.backend = renderer.backend
      } else {
        setBackend("css")
        diagRef.current.backend = "css"
      }
    }

    init()
    return () => {
      cancelled = true
      rendererRef.current?.dispose()
      rendererRef.current = null
    }
  }, [])

  // Keep canvas sized to stage * capped DPR (<= 2).
  useEffect(() => {
    const canvas = canvasRef.current
    const wrap = wrapRef.current
    if (!canvas || !wrap) return

    const syncSize = () => {
      const rect = wrap.getBoundingClientRect()
      const dpr = Math.min(window.devicePixelRatio || 1, 2)
      const w = Math.max(1, Math.min(2400, Math.round(rect.width * dpr)))
      const h = Math.max(1, Math.min(1440, Math.round(rect.height * dpr)))
      if (canvas.width !== w || canvas.height !== h) {
        canvas.width = w
        canvas.height = h
        wasClearedRef.current = false
      }
      diagRef.current.canvasWidth = w
      diagRef.current.canvasHeight = h
      diagRef.current.dpr = dpr
    }

    syncSize()
    const ro = new ResizeObserver(syncSize)
    ro.observe(wrap)
    return () => ro.disconnect()
  }, [wrapRef, backend])

  // Clear cleanly on mode switch or replay to t = 0.
  useEffect(() => {
    wasClearedRef.current = false
  }, [mode])

  // Render frame locked to timeline `t`.
  useEffect(() => {
    diagRef.current.lastT = t
    const renderer = rendererRef.current
    if (!renderer) return
    if (!visibleRef.current || document.hidden) {
      diagRef.current.paused = true
      return
    }
    diagRef.current.paused = false

    const rects = rectsRef.current
    const u = uniformsRef.current
    const activeEffects: string[] = []

    // 1. Active ink tip (scripted or user stroke)
    let tipX = 0
    let tipY = 0
    let tipIntensity = 0
    const activeUser = userStrokes.find((s) => !s.done && s.points.length > 0)
    if (activeUser) {
      const pt = activeUser.points[activeUser.points.length - 1]
      tipX = pt[0]
      tipY = pt[1]
      tipIntensity = 1
      activeEffects.push("ink-nib")
    } else if (script) {
      for (const s of script.strokes) {
        if (t > s.t0 - 30 && t < s.t1 + 140) {
          const st = strokeAt(s, clamp(t, s.t0 + 1, s.t1))
          if (st) {
            tipX = st.tip[0]
            tipY = st.tip[1]
            const fadeOut = t > s.t1 ? 1 - (t - s.t1) / 140 : 1
            tipIntensity = clamp(fadeOut, 0, 1)
            activeEffects.push("ink-nib")
          }
          break
        }
      }
    }

    // 2. Pin drops (#1 and #2)
    let pin1X = 0
    let pin1Y = 0
    let pin1Prog = 0
    let pin1Active = 0
    let pin2X = 0
    let pin2Y = 0
    let pin2Prog = 0
    let pin2Active = 0
    if (script) {
      const p1 = script.pins[0]
      if (p1 && t >= p1.t && t <= p1.t + 720) {
        pin1X = p1.at[0]
        pin1Y = p1.at[1]
        pin1Prog = progress(t, p1.t, p1.t + 720)
        pin1Active = 1
        activeEffects.push("pin-1")
      }
      const p2 = script.pins[1]
      if (p2 && t >= p2.t && t <= p2.t + 720) {
        pin2X = p2.at[0]
        pin2Y = p2.at[1]
        pin2Prog = progress(t, p2.t, p2.t + 720)
        pin2Active = 1
        activeEffects.push("pin-2")
      }
    }

    // 3. Capture finish shutter wave, HUD copied aura, prompt paste pulse, contact sheet photo-develop sweep
    const finishProg = t >= T.finish - 30 && t <= T.finish + 640 ? progress(t, T.finish - 30, T.finish + 640) : 0
    if (finishProg > 0) activeEffects.push("capture-shutter")

    const copiedProg = t >= T.copied - 30 && t <= T.copied + 680 ? progress(t, T.copied - 30, T.copied + 680) : 0
    if (copiedProg > 0) activeEffects.push("hud-copied")

    const pasteProg = t >= T.paste - 20 && t <= T.paste + 540 ? progress(t, T.paste - 20, T.paste + 540) : 0
    if (pasteProg > 0) activeEffects.push("prompt-paste")

    const sheetDevelopProg = t >= T.submit && t <= T.submit + 900 ? progress(t, T.submit, T.submit + 900) : 0
    if (sheetDevelopProg > 0) activeEffects.push("sheet-develop")

    // 4. Fix Payoff: Window/Card refraction light sweep + Badge/Actions motion trails + Lock-in bloom + Particles
    const diffSweepProg = 0
    const fixSweepProg = t >= T.fix[0] - 80 && t <= T.fix[1] + 280 ? easeOut(progress(t, T.fix[0] - 80, T.fix[1] + 280)) : 0
    if (fixSweepProg > 0) activeEffects.push("fix-light-sweep")

    const trailProg = t >= T.fix[0] && t <= T.fix[0] + 820 ? progress(t, T.fix[0], T.fix[0] + 820) : 0
    const trailIntensity = trailProg > 0 ? Math.sin(trailProg * Math.PI) : 0
    if (trailIntensity > 0) activeEffects.push("fix-motion-trails")

    const bloomProg = t >= T.fix[0] + 260 && t <= T.fix[1] + 350 ? progress(t, T.fix[0] + 260, T.fix[1] + 350) : 0
    const bloomPulse = bloomProg > 0 ? Math.sin(bloomProg * Math.PI) : 0
    if (bloomPulse > 0) activeEffects.push("fix-bloom")

    const fixSparkleProg = t >= T.fix[0] + 20 && t <= T.fix[1] + 450 ? progress(t, T.fix[0] + 20, T.fix[1] + 450) : 0
    if (fixSparkleProg > 0) activeEffects.push("fix-particles")

    diagRef.current.activeEffects = activeEffects

    if (activeEffects.length === 0) {
      if (!wasClearedRef.current) {
        renderer.clear()
        wasClearedRef.current = true
      }
      return
    }
    wasClearedRef.current = false

    const { fix, rowFix } = fixAmount(t)
    const badgeShiftX = (mode === "iphone" ? -46 : -60) * (1 - fix)
    const badgeShiftY = -8 * (1 - fix)
    const actionsShiftY = 14 * (1 - fix)
    const rowGrow = (mode === "iphone" ? 32 : 44) * rowFix

    // Keep promptRect & diffRect aligned when .tp-box expands on paste or .ts-content scrolls during diff/fix.
    if ((pasteProg > 0 || diffSweepProg > 0) && stageRef.current) {
      const base = stageRef.current.getBoundingClientRect()
      const s = base.width / STAGE.w
      if (s > 0) {
        if (pasteProg > 0) {
          const el = stageRef.current.querySelector(".tp-box")
          if (el) {
            const r = el.getBoundingClientRect()
            rects.prompt = { x: (r.left - base.left) / s, y: (r.top - base.top) / s, w: r.width / s, h: r.height / s }
          }
        }
        if (diffSweepProg > 0) {
          const el = stageRef.current.querySelector(".tdiff")
          if (el) {
            const r = el.getBoundingClientRect()
            rects.diff = { x: (r.left - base.left) / s, y: (r.top - base.top) / s, w: r.width / s, h: r.height / s }
          }
        }
      }
    }

    // u[0]: stageW, stageH, timeSec, reducedMotion
    u[0] = STAGE.w
    u[1] = STAGE.h
    u[2] = t / 1000
    u[3] = reducedMotionRef.current ? 1 : 0

    // u[1]: winRect (or capture AREA during shutter flash)
    const winR = finishProg > 0 && mode === "mac" ? AREA : rects.win
    u[4] = winR.x
    u[5] = winR.y
    u[6] = winR.w
    u[7] = winR.h

    // u[2]: cardRect
    u[8] = rects.card.x
    u[9] = rects.card.y
    u[10] = rects.card.w
    u[11] = rects.card.h + rowGrow

    // u[3]: badgeNow
    u[12] = rects.badge.x + badgeShiftX
    u[13] = rects.badge.y + badgeShiftY
    u[14] = rects.badge.w
    u[15] = rects.badge.h

    // u[4]: badgeStart (broken x, broken y, broken actionsY, current actionsY)
    u[16] = rects.badge.x + (mode === "iphone" ? -46 : -60)
    u[17] = rects.badge.y - 8
    u[18] = rects.actions.y + rowGrow + 14
    u[19] = rects.actions.y + rowGrow + actionsShiftY

    // u[5]: actionsNow
    u[20] = rects.actions.x
    u[21] = rects.actions.y + rowGrow + actionsShiftY
    u[22] = rects.actions.w
    u[23] = rects.actions.h

    // u[6]: diffRect
    u[24] = rects.diff.x
    u[25] = rects.diff.y
    u[26] = rects.diff.w
    u[27] = rects.diff.h

    // u[7]: sheetRect
    u[28] = rects.sheet.x
    u[29] = rects.sheet.y
    u[30] = rects.sheet.w
    u[31] = rects.sheet.h

    // u[8]: promptRect
    u[32] = rects.prompt.x
    u[33] = rects.prompt.y
    u[34] = rects.prompt.w
    u[35] = rects.prompt.h

    // u[9]: inkTip
    u[36] = tipX
    u[37] = tipY
    u[38] = tipIntensity
    u[39] = 1

    // u[10]: pin1
    u[40] = pin1X
    u[41] = pin1Y
    u[42] = pin1Prog
    u[43] = pin1Active

    // u[11]: pin2
    u[44] = pin2X
    u[45] = pin2Y
    u[46] = pin2Prog
    u[47] = pin2Active

    // u[12]: captureWave
    u[48] = finishProg
    u[49] = copiedProg
    u[50] = pasteProg
    u[51] = sheetDevelopProg

    // u[13]: fixWave
    u[52] = diffSweepProg
    u[53] = fixSweepProg
    u[54] = trailIntensity
    u[55] = bloomPulse

    // u[14]: particles
    u[56] = fixSparkleProg
    u[57] = pin1Prog
    u[58] = pin2Prog
    u[59] = tipIntensity > 0 ? t / 1000 : -1

    // u[15]: extra (winRadius, hudX, hudY, hudW)
    u[60] = mode === "mac" ? 12 : 38
    u[61] = PILL.cx
    u[62] = PILL_CY
    u[63] = pillWidth(PILL.items.copied)

    const hasParticles = !reducedMotionRef.current && (fixSparkleProg > 0 || pin1Active > 0 || pin2Active > 0 || tipIntensity > 0)
    const t0 = performance.now()
    renderer.render(u, hasParticles)
    const dtRender = performance.now() - t0
    const samples = renderSamplesRef.current
    samples.push(dtRender)
    if (samples.length > 300) samples.shift()
    const sum = samples.reduce((a, b) => a + b, 0)
    const sorted = [...samples].sort((a, b) => a - b)
    diagRef.current.avgRenderMs = Number((sum / samples.length).toFixed(3))
    diagRef.current.p95RenderMs = Number((sorted[Math.floor(sorted.length * 0.95)] ?? 0).toFixed(3))
    diagRef.current.drawCalls += 1
  }, [t, mode, script, userStrokes])

  if (backend === "css") {
    const fixSweep = t >= T.fix[0] - 80 && t <= T.fix[1] + 280
    const finishFlash = t >= T.finish - 30 && t <= T.finish + 550
    const rects = rectsRef.current
    const { rowFix } = fixAmount(t)
    const rowGrow = (mode === "iphone" ? 32 : 44) * rowFix
    return (
      <div className="gpu-css-fallback" aria-hidden>
        {finishFlash && (
          <div
            className="gpu-css-shutter"
            style={{
              left: `${(rects.win.x / STAGE.w) * 100}%`,
              top: `${(rects.win.y / STAGE.h) * 100}%`,
              width: `${(rects.win.w / STAGE.w) * 100}%`,
              height: `${(rects.win.h / STAGE.h) * 100}%`,
            }}
          />
        )}
        {fixSweep && (
          <div
            className="gpu-css-sweep"
            style={{
              left: `${(rects.card.x / STAGE.w) * 100}%`,
              top: `${(rects.card.y / STAGE.h) * 100}%`,
              width: `${(rects.card.w / STAGE.w) * 100}%`,
              height: `${((rects.card.h + rowGrow) / STAGE.h) * 100}%`,
            }}
          />
        )}
      </div>
    )
  }

  return <canvas ref={canvasRef} className="stage-gpu-canvas" aria-hidden />
}
export default StageGpuOverlay
