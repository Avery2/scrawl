import { choice, defineControls, knob, toggle } from "knobs"
import { useEffect, useRef, useState, useSyncExternalStore, type CSSProperties } from "react"
import * as THREE from "three"
import { RoomEnvironment } from "three/examples/jsm/environments/RoomEnvironment.js"
import { toCreasedNormals } from "three/examples/jsm/utils/BufferGeometryUtils.js"
import {
  BRISTLE_SLAB,
  BRISTLES,
  BRUSH,
  BRUSH_BOX,
  BrushFallback,
  BrushIcon,
  fillet,
  HANDLE,
  HANDLE_SLAB,
  TAPER,
  toLocal,
  VIEW,
  type Outline,
} from "./brush-paths"

export { BrushFallback }

const TAU = Math.PI * 2
const AXIS = new THREE.Vector3(1, 1, 0).normalize()
const PERP = new THREE.Vector3(-1, 1, 0).normalize()
const PAINT = 0xff3b30
const FLICK_EVERY = 5000
// After the first frame, hold the flat icon for a beat, then blur-crossfade it into the live brush.
const REVEAL_DELAY = 180
const REVEAL_MS = 650
const REVEAL_EASE = "cubic-bezier(0.22, 1, 0.36, 1)"
const SETTLE_K = 38
const SETTLE_C = 2 * Math.sqrt(SETTLE_K) * 0.5
const ROLL_K = 46
const ROLL_C = 2 * Math.sqrt(ROLL_K) * 0.58
const SWING_K = 70
const SWING_C = 2 * Math.sqrt(SWING_K) * 0.3
const GRAVITY = 24
const TRAIL_LIFE = 0.6
const TRAIL_MAX = 72
const TRAIL_WIDTH = 0.1
const DROPS = 12
// The middle of the bristle tip, in icon pixels, where the paint trail leaves the brush.
const TIP: [number, number] = [336, 771]
const DEG = Math.PI / 180

// Hover the brush for the ⚙ knobs chip, or press 1–5 for a finish. Dev only: the production
// build of knobs is always disabled and returns these locked defaults.
export const brushControls = defineControls(
  "brush",
  {
    handle: choice("brushed", ["brushed", "chrome", "titanium", "anodized", "ribbed"], { label: "Finish", group: "Handle" }),
    handleRough: knob(0.24, { min: 0.04, max: 0.75, step: 0.01, label: "Handle roughness", group: "Handle" }),
    grain: knob(0.55, { min: 0, max: 1, step: 0.01, label: "Metal grain", group: "Handle" }),
    chamfer: knob(0.45, { min: 0, max: 1, step: 0.01, label: "Edge chamfer", group: "Handle" }),
    dryLift: knob(0.42, { min: 0.15, max: 0.85, step: 0.01, label: "Dry bristle lift", group: "Bristles" }),
    strands: knob(42, { min: 12, max: 90, step: 1, label: "Bristle strands", group: "Bristles" }),
    groove: knob(0.65, { min: 0, max: 1, step: 0.01, label: "Bristle groove", group: "Bristles" }),
    bristleRough: knob(0.72, { min: 0.2, max: 0.95, step: 0.02, label: "Bristle roughness", group: "Bristles" }),
    dip: knob(0.38, { min: 0.15, max: 0.75, step: 0.01, label: "Paint dip", group: "Bristles" }),
    scale: knob(1, { min: 0.7, max: 1.4, step: 0.02, label: "Brush scale", group: "Pose" }),
    autoFlick: toggle(true, { label: "Auto flick", group: "Pose" }),
    twirl: knob(0, { min: -180, max: 180, step: 1, unit: "deg", label: "Twirl angle", group: "Pose" }),
    tilt: knob(0, { min: -35, max: 35, step: 1, unit: "deg", label: "Tilt angle", group: "Pose" }),
  },
  {
    enabled: true,
    // The default preset is the brushed handle, so 1–5 over the brush steps through all five finishes.
    presets: {
      chrome: { handle: "chrome", handleRough: 0.05, grain: 0, chamfer: 0.3 },
      titanium: { handle: "titanium", handleRough: 0.42, grain: 0.55, chamfer: 0.3 },
      spaceBlack: { handle: "anodized", handleRough: 0.32, grain: 0.3, chamfer: 0.85 },
      ribbed: { handle: "ribbed", handleRough: 0.22, grain: 0.75, chamfer: 0.5 },
    },
  },
)

type BrushValues = ReturnType<typeof brushControls.get>
type HandleFinish = BrushValues["handle"]

// Base metal color and clearcoat for each finish; the shader adds its grain, ribs, and chamfers.
const FINISHES: Record<HandleFinish, { index: number; color: number; clearcoat: number; clearcoatRoughness: number }> = {
  brushed: { index: 0, color: 0xe4e7ee, clearcoat: 0.25, clearcoatRoughness: 0.2 },
  chrome: { index: 1, color: 0xf4f6fa, clearcoat: 1, clearcoatRoughness: 0.03 },
  titanium: { index: 2, color: 0xc4c0b8, clearcoat: 0.08, clearcoatRoughness: 0.4 },
  anodized: { index: 3, color: 0x252830, clearcoat: 0.35, clearcoatRoughness: 0.18 },
  ribbed: { index: 4, color: 0xd8dbe2, clearcoat: 0.4, clearcoatRoughness: 0.12 },
}

// Dry bristle colors at the default lift; the lift knob scales them together.
const DRY_LIFT = 0.42
const DRY_BASE = new THREE.Color(0x565d6e)
const DRY_RIDGE = new THREE.Color(0x7c8599)
const DRY_SEAM = new THREE.Color(0x16181f)

const shape = (outline: Outline, radius: number) => new THREE.Shape(fillet(outline, radius).map(([x, y]) => new THREE.Vector2(x, y)))

function slab(outline: Outline, radius: number, depth: number, bevel: number) {
  // A negative offset keeps the side walls on the outline and rounds the caps inward.
  const inset = bevel * 0.8
  const geometry = new THREE.ExtrudeGeometry(shape(outline, radius), {
    depth,
    steps: 1,
    curveSegments: 1,
    bevelEnabled: true,
    bevelThickness: bevel,
    bevelSize: inset,
    bevelOffset: -inset,
    bevelSegments: 10,
  })
  geometry.translate(0, 0, -depth / 2)
  return geometry
}

function buildBrush() {
  const handle = slab(HANDLE, HANDLE_SLAB.radius, HANDLE_SLAB.depth, HANDLE_SLAB.bevel)
  const bristles = slab(BRISTLES, BRISTLE_SLAB.radius, BRISTLE_SLAB.depth, BRISTLE_SLAB.bevel)
  const { scale, tip, base } = BRUSH
  const handleZ = depths(handle)
  const bristleZ = depths(bristles)

  const pos = bristles.getAttribute("position")
  const v = new THREE.Vector3()
  for (let i = 0; i < pos.count; i++) {
    v.fromBufferAttribute(pos, i)
    pos.setZ(i, v.z * (1 - (TAPER * (base - v.dot(AXIS))) / (base - tip)))
  }
  const slope = ((BRISTLE_SLAB.depth / 2 + BRISTLE_SLAB.bevel) * TAPER) / (base - tip)

  // Normals are welded by position hash, so build them at icon scale where vertices are well apart.
  toCreasedNormals(handle, Math.PI / 3)
  toCreasedNormals(bristles, Math.PI / 3)
  roundEdges(handle, handleZ, HANDLE_SLAB, 0, BRUSH.handleMid, 0.5 * scale)
  roundEdges(bristles, bristleZ, BRISTLE_SLAB, slope, BRUSH.bristleMid, 0.35 * scale)

  const center = new THREE.Vector3(...BRUSH.center, 0)
  for (const g of [handle, bristles]) {
    g.translate(-center.x, -center.y, 0)
    g.scale(scale, scale, scale)
  }
  const brushTip = new THREE.Vector3(...toLocal(TIP), 0).sub(center).multiplyScalar(scale)
  // The ferrule ends at the handle's shoulders, where the outline steps in to the shaft.
  const shoulder = (alongAxis(HANDLE[3]) + alongAxis(HANDLE[6])) / 2
  return {
    handle,
    bristles,
    base: (base - center.dot(AXIS)) * scale,
    tip: (tip - center.dot(AXIS)) * scale,
    ferrule: (shoulder - center.dot(AXIS)) * scale,
    brushTip,
  }
}

const alongAxis = (p: readonly [number, number]) => new THREE.Vector3(...toLocal(p), 0).dot(AXIS)

const depths = (g: THREE.BufferGeometry) => Float32Array.from(g.getAttribute("position").array.filter((_, i) => i % 3 === 2))

// Rebuild cap and bevel normals analytically. Each bevel ring blends from the cap normal to the
// side wall's normal by its angle around the quarter-round, so highlights roll smoothly off the
// edge. The cap normal leans linearly across the width, which reads as a gently barrelled face
// (flat metal reflects one flat color) and stays exact across earcut's long cap triangles.
function roundEdges(
  g: THREE.BufferGeometry,
  z0: Float32Array,
  { depth, bevel }: { depth: number; bevel: number },
  slope: number,
  center: number,
  curve: number,
) {
  const pos = g.getAttribute("position")
  const normal = g.getAttribute("normal")
  const p = new THREE.Vector3()
  const cap = new THREE.Vector3()
  const side = new THREE.Vector3()
  for (let i = 0; i < pos.count; i++) {
    const z = Math.abs(z0[i])
    if (z < depth / 2 - 1e-4) continue
    const theta = Math.acos(THREE.MathUtils.clamp((z - depth / 2) / bevel, 0, 1))
    p.fromBufferAttribute(pos, i)
    cap
      .set(0, 0, Math.sign(z0[i]))
      .addScaledVector(AXIS, -slope)
      .addScaledVector(PERP, (p.dot(PERP) - center) * curve)
      .normalize()
    side.set(normal.getX(i), normal.getY(i), 0)
    if (side.lengthSq() < 1e-12) side.set(0, 0, 0)
    else side.normalize()
    cap.multiplyScalar(Math.cos(theta)).addScaledVector(side, Math.sin(theta)).normalize()
    normal.setXYZ(i, cap.x, cap.y, cap.z)
  }
}

const NOISE_GLSL = `
float hash3(vec3 p) {
  p = fract(p * 0.1031);
  p += dot(p, p.zyx + 31.32);
  return fract((p.x + p.y) * p.z);
}
float vnoise(vec3 x) {
  vec3 i = floor(x);
  vec3 f = fract(x);
  f = f * f * (3.0 - 2.0 * f);
  return mix(
    mix(mix(hash3(i), hash3(i + vec3(1, 0, 0)), f.x), mix(hash3(i + vec3(0, 1, 0)), hash3(i + vec3(1, 1, 0)), f.x), f.y),
    mix(mix(hash3(i + vec3(0, 0, 1)), hash3(i + vec3(1, 0, 1)), f.x), mix(hash3(i + vec3(0, 1, 1)), hash3(i + vec3(1, 1, 1)), f.x), f.y),
    f.z);
}`

const VERTEX_VARYINGS = [
  "#include <common>\nuniform vec3 uAxis;\nuniform vec3 uPerp;\nvarying vec3 vObj;\nvarying vec3 vAxisView;\nvarying vec3 vPerpView;\nvarying float vCap;",
  "#include <begin_vertex>\nvObj = position;\nvAxisView = normalize(normalMatrix * uAxis);\nvPerpView = normalize(normalMatrix * uPerp);\nvCap = abs(normal.z);",
] as const

function withVaryings(vertexShader: string) {
  return vertexShader.replace("#include <common>", VERTEX_VARYINGS[0]).replace("#include <begin_vertex>", VERTEX_VARYINGS[1])
}

// Silver-badger bristles near the ferrule, dipped in rich paint toward the tip. Bundles of hair
// with fine strands inside them, a grazing rim light so the dry band holds against a dark page,
// and paint that climbs each strand to a slightly different height.
// Matte satin throughout: the environment is bound explicitly because three ignores a material's
// envMapIntensity when it falls back to scene.environment, and that full-strength studio reflection
// is what washed the bristles out into white plastic.
function bristleMaterial(base: number, tip: number, envMap: THREE.Texture) {
  const uniforms = {
    uAxis: { value: AXIS },
    uPerp: { value: PERP },
    uBase: { value: base },
    uTip: { value: tip },
    uDryBase: { value: DRY_BASE.clone() },
    uDryRidge: { value: DRY_RIDGE.clone() },
    uDrySeam: { value: DRY_SEAM.clone() },
    uDryRough: { value: 0.86 },
    uDensity: { value: 42 },
    uGroove: { value: 0.65 },
    uDip: { value: 0.38 },
    uRim: { value: DRY_LIFT },
  }
  const material = new THREE.MeshPhysicalMaterial({
    color: PAINT,
    roughness: 0.72,
    metalness: 0,
    specularIntensity: 0.35,
    clearcoat: 0.03,
    clearcoatRoughness: 0.6,
    envMap,
    envMapIntensity: 0.06,
  })
  material.onBeforeCompile = (shader) => {
    Object.assign(shader.uniforms, uniforms)
    shader.vertexShader = withVaryings(shader.vertexShader)
    shader.fragmentShader = shader.fragmentShader
      .replace(
        "#include <common>",
        `#include <common>
uniform vec3 uAxis;
uniform vec3 uPerp;
uniform float uBase;
uniform float uTip;
uniform vec3 uDryBase;
uniform vec3 uDryRidge;
uniform vec3 uDrySeam;
uniform float uDryRough;
uniform float uDensity;
uniform float uGroove;
uniform float uDip;
uniform float uRim;
varying vec3 vObj;
varying vec3 vAxisView;
varying vec3 vPerpView;
varying float vCap;
float across() { return dot(vObj, uPerp); }
// Fine hair strands inside each bundle.
float strandPhase() { return across() * uDensity * 2.4; }
float wetness() {
  float w = across();
  float u = (dot(vObj, uAxis) - uBase) / (uTip - uBase);
  u += 0.035 * sin(w * 9.0 + 0.6) + 0.02 * sin(w * 23.0);
  // Each strand's tip carries paint a little higher or lower than its neighbours.
  float strand = floor(strandPhase() / 6.2831853);
  u += (fract(sin(strand * 91.7 + 3.1) * 43758.5453) - 0.5) * 0.08;
  float edge = 1.0 - uDip;
  return smoothstep(edge - 0.07, edge + 0.07, u);
}
// Position across one bundle, -1..1, with slightly uneven bundle widths.
float fiber() {
  float x = across() * uDensity * 0.18;
  return fract(x + 0.12 * sin(x * 2.1)) * 2.0 - 1.0;
}
// The dark seam between neighbouring bundles, antialiased to about a pixel.
float seam(float f) {
  float w = fwidth(across() * uDensity * 0.18) * 2.0;
  return smoothstep(0.84 - w, 0.84 + w, abs(f));
}
// Fine strands fade out before they would alias.
float strandDetail() {
  return 1.0 - smoothstep(0.25, 0.6, fwidth(strandPhase()) / 6.2831853);
}`,
      )
      .replace(
        "#include <color_fragment>",
        `#include <color_fragment>
float wet = wetness();
float fib = fiber();
float bundleSeam = seam(fib) * vCap;
float ph = strandPhase();
float hair = (0.5 + 0.5 * sin(ph + 1.7 * sin(ph * 0.155))) * strandDetail();
float ridge = 1.0 - fib * fib;
vec3 dry = mix(uDryBase, uDryRidge, clamp(ridge * 0.55 + hair * 0.45, 0.0, 1.0));
dry *= 1.0 - 0.35 * uGroove * (1.0 - hair) * strandDetail();
dry = mix(dry, uDrySeam, bundleSeam * uGroove);
vec3 paint = diffuseColor.rgb * (1.0 - 0.5 * bundleSeam * uGroove) * (1.0 - 0.12 * uGroove * (1.0 - hair));
diffuseColor.rgb = mix(dry, paint, wet);`,
      )
      .replace(
        "#include <roughnessmap_fragment>",
        "#include <roughnessmap_fragment>\nroughnessFactor = mix(uDryRough, roughnessFactor, wet);",
      )
      .replace(
        "#include <normal_fragment_maps>",
        `#include <normal_fragment_maps>
normal = normalize(normal + vPerpView * (fib * 0.45 + cos(ph) * 0.18 * strandDetail()) * vCap * uGroove);
// A grazing rim light picks the dry bristles out of the dark page.
float rimLight = pow(1.0 - abs(dot(normal, normalize(vViewPosition))), 2.2);
totalEmissiveRadiance += vec3(0.42, 0.47, 0.58) * rimLight * (0.25 + 0.9 * uRim) * (1.0 - 0.75 * wet) * (0.75 + 0.25 * hair);`,
      )
      .replace("material.clearcoat = clearcoat;", "material.clearcoat = clearcoat * wet;")
  }
  return { material, uniforms }
}

// Metal for the handle and ferrule, with five procedural finishes chosen by uPreset.
function handleMaterial(ferrule: number) {
  const uniforms = {
    uAxis: { value: AXIS },
    uPerp: { value: PERP },
    uFerrule: { value: ferrule },
    uPreset: { value: 0 },
    uGrain: { value: 0.55 },
    uBevel: { value: 0.45 },
  }
  const material = new THREE.MeshPhysicalMaterial({
    color: FINISHES.brushed.color,
    metalness: 1,
    roughness: 0.24,
    clearcoat: FINISHES.brushed.clearcoat,
    clearcoatRoughness: FINISHES.brushed.clearcoatRoughness,
    // ExtrudeGeometry's cap UVs are the outline's x and y, so 45° runs the grain along the handle.
    anisotropyRotation: Math.PI / 4,
  })
  material.onBeforeCompile = (shader) => {
    Object.assign(shader.uniforms, uniforms)
    shader.vertexShader = withVaryings(shader.vertexShader)
    shader.fragmentShader = shader.fragmentShader
      .replace(
        "#include <common>",
        `#include <common>
uniform vec3 uAxis;
uniform vec3 uPerp;
uniform float uFerrule;
uniform float uPreset;
uniform float uGrain;
uniform float uBevel;
varying vec3 vObj;
varying vec3 vAxisView;
varying vec3 vPerpView;
varying float vCap;
${NOISE_GLSL}`,
      )
      .replace(
        "#include <color_fragment>",
        `#include <color_fragment>
int preset = int(uPreset + 0.5);
float hu = dot(vObj, uAxis);
float hw = dot(vObj, uPerp);
float slope = 1.0 - vCap;
// The machined chamfer: the outer part of the rounded edge, not the flat face or the side wall.
float chamfer = smoothstep(0.08, 0.32, slope) * (1.0 - smoothstep(0.82, 0.98, slope));
float collar = 1.0 - smoothstep(uFerrule - 0.006, uFerrule + 0.006, hu);
float hRough = 0.0;
vec3 hBump = vec3(0.0);
if (preset == 0 || preset == 3) {
  // Brushed grain: long streaks along the handle, fine ones fading before they alias.
  float fineFreq = 520.0;
  float fade = 1.0 - smoothstep(0.35, 1.0, fwidth(hw) * fineFreq);
  float fine = mix(0.5, vnoise(vec3(hw * fineFreq, hu * 5.0, 1.3)), fade);
  float coarse = vnoise(vec3(hw * 140.0, hu * 1.5, 7.1));
  float g = (fine * 0.6 + coarse * 0.4 - 0.5) * uGrain * (preset == 3 ? 0.5 : 1.0);
  diffuseColor.rgb *= 1.0 + g * 0.45;
  hRough += g * 0.22;
  hBump += vPerpView * g * 0.35;
} else if (preset == 1) {
  float g = (vnoise(vObj * 60.0) - 0.5) * uGrain;
  diffuseColor.rgb *= 1.0 + g * 0.1;
  hRough += g * 0.05;
} else if (preset == 2) {
  // Bead-blasted: isotropic micro-grain.
  float fade = 1.0 - smoothstep(0.35, 1.0, fwidth(hw) * 360.0);
  float n1 = mix(0.5, vnoise(vObj * 360.0), fade);
  float n2 = mix(0.5, vnoise(vObj * 360.0 + 17.0), fade);
  float n3 = vnoise(vObj * 80.0 + 5.0);
  float g = (n1 * 0.65 + n3 * 0.35 - 0.5) * uGrain;
  diffuseColor.rgb *= 1.0 + g * 0.3;
  hRough += g * 0.3;
  hBump += (vPerpView * (n1 - 0.5) + vAxisView * (n2 - 0.5)) * 0.22 * uGrain;
} else {
  // CNC ribs along the shaft, with a polished collar on the ferrule.
  float shaft = 1.0 - collar;
  float phase = hu * 9.0 * 6.2831853;
  float aa = 1.0 - smoothstep(0.25, 0.6, fwidth(hu * 9.0));
  float rib = smoothstep(-0.35, 0.55, sin(phase));
  diffuseColor.rgb *= 1.0 - 0.5 * (1.0 - rib) * shaft * uGrain * aa;
  hBump += vAxisView * cos(phase) * 0.6 * shaft * uGrain * aa;
  hRough += mix(0.0, -0.18, collar) + (1.0 - rib) * 0.15 * shaft * uGrain;
  float line = 1.0 - smoothstep(0.0, 0.012, abs(hu - uFerrule));
  diffuseColor.rgb *= 1.0 - 0.7 * line;
}
// Anodizing is cut through to bright aluminium on the chamfers; other finishes just catch a rim.
vec3 silver = vec3(0.86, 0.88, 0.92);
float cut = chamfer * uBevel * (preset == 3 ? 1.0 : 0.55);
diffuseColor.rgb = mix(diffuseColor.rgb, silver, cut);
hRough -= cut * 0.25;`,
      )
      .replace(
        "#include <roughnessmap_fragment>",
        "#include <roughnessmap_fragment>\nroughnessFactor = clamp(roughnessFactor + hRough, 0.02, 1.0);",
      )
      .replace(
        "#include <normal_fragment_maps>",
        "#include <normal_fragment_maps>\nnormal = normalize(normal + hBump);",
      )
  }
  return { material, uniforms }
}

// A tapering ribbon of paint left behind the bristle tip, facing the camera.
function paintTrail() {
  const positions = new Float32Array(TRAIL_MAX * 2 * 3)
  const colors = new Float32Array(TRAIL_MAX * 2 * 4)
  const index: number[] = []
  for (let i = 0; i < TRAIL_MAX - 1; i++) {
    const a = i * 2
    index.push(a, a + 1, a + 2, a + 1, a + 3, a + 2)
  }
  const geometry = new THREE.BufferGeometry()
  geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3).setUsage(THREE.DynamicDrawUsage))
  geometry.setAttribute("color", new THREE.BufferAttribute(colors, 4).setUsage(THREE.DynamicDrawUsage))
  geometry.setIndex(index)
  geometry.setDrawRange(0, 0)
  const material = new THREE.MeshBasicMaterial({
    vertexColors: true,
    transparent: true,
    depthWrite: false,
    side: THREE.DoubleSide,
  })
  const mesh = new THREE.Mesh(geometry, material)
  mesh.frustumCulled = false
  mesh.renderOrder = 1

  const paint = new THREE.Color(PAINT)
  const samples: { p: THREE.Vector3; t: number; ink: number }[] = []
  const side = new THREE.Vector2()
  let hadSamples = false

  const update = (tip: THREE.Vector3, ink: number, now: number) => {
    if (ink > 0.01) samples.push({ p: tip.clone(), t: now, ink })
    while (samples.length && (now - samples[0].t > TRAIL_LIFE || samples.length > TRAIL_MAX)) samples.shift()
    const n = samples.length
    if (n === 0 && !hadSamples) return
    hadSamples = n > 0
    const pos = geometry.getAttribute("position") as THREE.BufferAttribute
    const col = geometry.getAttribute("color") as THREE.BufferAttribute
    samples.forEach((s, i) => {
      const a = samples[Math.max(0, i - 1)].p
      const b = samples[Math.min(n - 1, i + 1)].p
      side.set(a.y - b.y, b.x - a.x)
      if (side.lengthSq() > 1e-10) side.normalize()
      const age = Math.min(1, (now - s.t) / TRAIL_LIFE)
      // Paint narrows as it trails off, and tapers in where it leaves the bristles.
      const width = TRAIL_WIDTH * s.ink * (1 - age) ** 1.2 * Math.min(1, 0.35 + (n - 1 - i) / 4)
      pos.setXYZ(i * 2, s.p.x + side.x * width, s.p.y + side.y * width, s.p.z)
      pos.setXYZ(i * 2 + 1, s.p.x - side.x * width, s.p.y - side.y * width, s.p.z)
      const alpha = Math.min(1, s.ink * 1.4) * (1 - age) ** 1.4 * 0.9
      col.setXYZW(i * 2, paint.r, paint.g, paint.b, alpha)
      col.setXYZW(i * 2 + 1, paint.r, paint.g, paint.b, alpha)
    })
    pos.needsUpdate = true
    col.needsUpdate = true
    geometry.setDrawRange(0, Math.max(0, n - 1) * 6)
  }
  const dispose = () => {
    geometry.dispose()
    material.dispose()
  }
  return { mesh, update, dispose }
}

function droplets() {
  const geometry = new THREE.SphereGeometry(1, 14, 10)
  const material = new THREE.MeshPhysicalMaterial({ color: PAINT, roughness: 0.2, clearcoat: 1, clearcoatRoughness: 0.05 })
  const mesh = new THREE.InstancedMesh(geometry, material, DROPS)
  mesh.frustumCulled = false
  const drops = Array.from({ length: DROPS }, () => ({ p: new THREE.Vector3(), v: new THREE.Vector3(), life: 0, size: 0 }))
  const m = new THREE.Matrix4()
  const q = new THREE.Quaternion()
  const s = new THREE.Vector3()
  const up = new THREE.Vector3(0, 1, 0)
  const dir = new THREE.Vector3()
  let next = 0
  let hadActive = true
  const emit = (at: THREE.Vector3, vel: THREE.Vector3) => {
    const d = drops[next]
    next = (next + 1) % DROPS
    d.p.copy(at)
    d.v
      .copy(vel)
      .multiplyScalar(0.55 + Math.random() * 0.35)
      .add(dir.set((Math.random() - 0.5) * 1.2, Math.random() * 1.4, (Math.random() - 0.5) * 0.6))
    d.life = 1
    d.size = 0.045 + Math.random() * 0.04
    hadActive = true
  }
  const update = (dt: number) => {
    let anyActive = false
    for (let i = 0; i < drops.length; i++) {
      if (drops[i].life > 0) {
        anyActive = true
        break
      }
    }
    if (!anyActive && !hadActive) return
    hadActive = anyActive
    drops.forEach((d, i) => {
      if (d.life > 0) {
        d.life -= dt / 0.55
        d.v.y -= GRAVITY * 0.5 * dt
        d.p.addScaledVector(d.v, dt)
      }
      const k = Math.max(0, d.life)
      s.setScalar(d.size * Math.sqrt(k))
      // Stretch each drop along its flight path.
      q.setFromUnitVectors(up, d.v.lengthSq() > 1e-6 ? dir.copy(d.v).normalize() : up)
      s.y *= 1 + Math.min(1.4, d.v.length() * 0.25)
      m.compose(d.p, q, s)
      mesh.setMatrixAt(i, m)
    })
    mesh.instanceMatrix.needsUpdate = true
  }
  const dispose = () => {
    geometry.dispose()
    material.dispose()
    mesh.dispose()
  }
  return { mesh, emit, update, dispose }
}

const hostStyle: CSSProperties = {
  ...BRUSH_BOX,
  cursor: "grab",
  touchAction: "none",
  userSelect: "none",
  WebkitUserSelect: "none",
}

const fade = ["opacity", "filter", "transform"].map((p) => `${p} ${REVEAL_MS}ms ${REVEAL_EASE}`).join(", ")

const flatStyle: CSSProperties = {
  position: "absolute",
  inset: 0,
  pointerEvents: "none",
  transition: fade,
}

const liveStyle: CSSProperties = {
  position: "absolute",
  inset: 0,
  opacity: 0,
  filter: "blur(10px)",
  transform: "scale(0.96)",
  transition: fade,
}

const glowStyle: CSSProperties = {
  position: "absolute",
  left: "44%",
  bottom: 24,
  width: 70,
  height: 12,
  marginLeft: -35,
  borderRadius: "50%",
  background: "radial-gradient(closest-side, rgba(255, 59, 48, 0.2), transparent)",
  filter: "blur(4px)",
  pointerEvents: "none",
}

const useBrushControls = () => useSyncExternalStore(brushControls.subscribe, brushControls.get, brushControls.getServerSnapshot)

export function Brush3D() {
  const values = useBrushControls()
  const applyRef = useRef<(values: BrushValues) => void>(null)
  const hostRef = useRef<HTMLDivElement>(null)
  const flatRef = useRef<HTMLDivElement>(null)
  const liveRef = useRef<HTMLDivElement>(null)
  const glowRef = useRef<HTMLDivElement>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    const host = hostRef.current
    const flat = flatRef.current
    const live = liveRef.current
    if (!host || !flat || !live) return

    let renderer: THREE.WebGLRenderer
    try {
      renderer = new THREE.WebGLRenderer({ antialias: true, alpha: true, powerPreference: "low-power" })
    } catch {
      setFailed(true)
      return
    }
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 3))
    renderer.toneMapping = THREE.NeutralToneMapping
    renderer.toneMappingExposure = 1.05
    renderer.setClearColor(0x000000, 0)
    const canvas = renderer.domElement
    Object.assign(canvas.style, { position: "absolute", inset: "0", width: "100%", height: "100%" })
    live.appendChild(canvas)

    const scene = new THREE.Scene()
    const pmrem = new THREE.PMREMGenerator(renderer)
    const room = new RoomEnvironment()
    const envMap = pmrem.fromScene(room, 0.04).texture
    room.dispose()
    scene.environment = envMap
    scene.environmentIntensity = 1.1

    const camera = new THREE.PerspectiveCamera(VIEW.fov, VIEW.width / VIEW.height, 0.1, 100)
    camera.position.set(0, 0, VIEW.distance)

    const key = new THREE.DirectionalLight(0xffffff, 1.1)
    key.position.set(-4, 6, 6)
    const rim = new THREE.DirectionalLight(0xff8a80, 1.2)
    rim.position.set(5, -3, -4)
    // The metal handle reflects the room; the matte bristles take their fill from here instead.
    const fill = new THREE.AmbientLight(0xffffff, 0.9)
    scene.add(key, rim, fill)

    const { handle, bristles, base, tip, ferrule, brushTip } = buildBrush()
    const { material: metal, uniforms: metalUniforms } = handleMaterial(ferrule)
    const { material: paint, uniforms: paintUniforms } = bristleMaterial(base, tip, envMap)

    // root (idle drift, hop) → pivot (swish about the wrist) → spin (twirl about the handle)
    const root = new THREE.Group()
    const pivot = new THREE.Group()
    const offset = new THREE.Group()
    const spin = new THREE.Group()
    const wrist = AXIS.clone().multiplyScalar(0.9)
    pivot.position.copy(wrist)
    offset.position.copy(wrist).negate()
    spin.add(new THREE.Mesh(handle, metal), new THREE.Mesh(bristles, paint))
    offset.add(spin)
    pivot.add(offset)
    root.add(pivot)
    scene.add(root)

    const trail = paintTrail()
    const drops = droplets()
    scene.add(trail.mesh, drops.mesh)

    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches
    let roll = 0
    let rollVel = 0
    let rollTarget = 0
    let swing = 0
    let swingVel = 0
    let swingTarget = 0
    let hop = 0
    let hopVel = 0
    let release = 0
    let nextFlick = Infinity
    // The brush starts face-on, exactly under the flat icon, and settles into its tilt as it reveals.
    let settle = 0
    let settleVel = 0
    let settleTarget = 0
    let revealTimer = 0
    let visible = true
    let raf = 0
    let last = performance.now()
    let firstFrame = true
    const tipNow = new THREE.Vector3()
    const tipPrev = new THREE.Vector3()
    const tipVel = new THREE.Vector3()
    let hasPrev = false
    let dropClock = 0

    const drag = { active: false, id: -1, x: 0, y: 0, moved: 0, last: 0 }

    // Wind up, then flick the tip up and around in an arc, with a hop and a twirl.
    const flick = (now: number, windup = true) => {
      if (release) return
      swingTarget = windup ? 0.1 : 0
      release = now + (windup ? 190 : 0)
    }

    const reveal = () => {
      Object.assign(flat.style, { opacity: "0", filter: "blur(10px)", transform: "scale(1.03)" })
      Object.assign(live.style, { opacity: "1", filter: "blur(0px)", transform: "none" })
      settleTarget = 1
      if (reduce) settle = 1
      else rollVel = 3.6
      nextFlick = performance.now() + 2600
    }
    const revealed = (e: TransitionEvent) => {
      if (e.target === live && e.propertyName === "filter") live.style.filter = "none"
    }
    live.addEventListener("transitionend", revealed)

    const resize = () => {
      const width = host.clientWidth
      const height = host.clientHeight
      if (!width || !height) return
      renderer.setSize(width, height, false)
      camera.aspect = width / height
      camera.updateProjectionMatrix()
    }
    resize()
    const ro = new ResizeObserver(resize)
    let pixelScale = 1

    applyRef.current = (v) => {
      const finish = FINISHES[v.handle]
      metal.color.setHex(finish.color)
      metal.roughness = v.handleRough
      metal.clearcoat = finish.clearcoat
      metal.clearcoatRoughness = finish.clearcoatRoughness
      metal.anisotropy = v.handle === "brushed" ? 0.6 * v.grain : 0
      metalUniforms.uPreset.value = finish.index
      metalUniforms.uGrain.value = v.grain
      metalUniforms.uBevel.value = v.chamfer
      const lift = v.dryLift / DRY_LIFT
      paintUniforms.uDryBase.value.copy(DRY_BASE).multiplyScalar(lift)
      paintUniforms.uDryRidge.value.copy(DRY_RIDGE).multiplyScalar(lift)
      paintUniforms.uRim.value = v.dryLift
      paintUniforms.uDensity.value = v.strands
      paintUniforms.uGroove.value = v.groove
      paintUniforms.uDryRough.value = Math.min(0.95, v.bristleRough + 0.14)
      paintUniforms.uDip.value = v.dip
      paint.roughness = v.bristleRough
      // The host is scaled with CSS; render at the matching resolution so it stays crisp.
      if (v.scale !== pixelScale) {
        pixelScale = v.scale
        renderer.setPixelRatio(Math.min(window.devicePixelRatio, 3) * pixelScale)
        resize()
      }
    }
    applyRef.current(brushControls.get())
    ro.observe(host)

    const frame = (now: number) => {
      const dt = Math.min(0.05, (now - last) / 1000)
      last = now

      const pose = brushControls.get()
      if (!reduce && pose.autoFlick && !drag.active && now >= nextFlick) {
        flick(now)
        nextFlick = now + FLICK_EVERY
      }
      if (release && now >= release) {
        release = 0
        swingTarget = 0
        swingVel -= 3.1
        hopVel += 2.4
        if (!drag.active) rollTarget = Math.round(roll / TAU) * TAU + TAU
      }

      // Semi-implicit springs in small steps stay stable at low frame rates.
      const steps = Math.ceil(dt / (1 / 240))
      const h = dt / steps
      for (let i = 0; i < steps; i++) {
        if (!drag.active) {
          rollVel += (ROLL_K * (rollTarget - roll) - ROLL_C * rollVel) * h
          roll += rollVel * h
        }
        settleVel += (SETTLE_K * (settleTarget - settle) - SETTLE_C * settleVel) * h
        settle += settleVel * h
        swingVel += (SWING_K * (swingTarget - swing) - SWING_C * swingVel) * h
        swing += swingVel * h
        hopVel -= GRAVITY * h
        hop += hopVel * h
        if (hop < 0) {
          hop = 0
          hopVel = hopVel < -0.6 ? -hopVel * 0.36 : 0
        }
      }
      if (!drag.active && Math.abs(rollTarget - roll) < 1e-4 && Math.abs(rollVel) < 1e-4) {
        const wraps = Math.round(rollTarget / TAU) * TAU
        roll -= wraps
        rollTarget -= wraps
        rollVel = 0
      }

      const s = now / 1000
      const idle = reduce || !pose.autoFlick ? 0 : 1
      root.position.y = settle * idle * Math.sin(s * 1.25) * 0.06 + hop
      root.rotation.x = settle * (-0.2 + idle * Math.sin(s * 0.7) * 0.08)
      root.rotation.y = settle * (0.24 + idle * Math.sin(s * 0.55 + 1) * 0.12) + pose.tilt * DEG
      root.rotation.z = settle * idle * Math.sin(s * 0.9) * 0.03
      pivot.rotation.z = swing
      spin.quaternion.setFromAxisAngle(AXIS, roll + pose.twirl * DEG)
      scene.updateMatrixWorld()

      tipNow.copy(brushTip).applyMatrix4(spin.matrixWorld)
      if (hasPrev && dt > 0) tipVel.subVectors(tipNow, tipPrev).divideScalar(dt)
      tipPrev.copy(tipNow)
      hasPrev = true
      const speed = tipVel.length()
      const ink = reduce ? 0 : THREE.MathUtils.smoothstep(speed, 1.2, 4)
      trail.update(tipNow, ink, s)
      dropClock -= dt
      if (!reduce && speed > 4.2 && dropClock <= 0) {
        drops.emit(tipNow, tipVel)
        dropClock = 0.05 + Math.random() * 0.07
      }
      drops.update(dt)

      const glow = glowRef.current
      if (glow) {
        const lift = Math.min(1, hop * 2.5)
        glow.style.opacity = String(1 - lift * 0.6)
        glow.style.transform = `scaleX(${1 - lift * 0.25})`
      }

      renderer.render(scene, camera)
      if (firstFrame) {
        firstFrame = false
        revealTimer = window.setTimeout(reveal, REVEAL_DELAY)
      }
      if (visible) raf = requestAnimationFrame(frame)
    }
    raf = requestAnimationFrame(frame)

    const io = new IntersectionObserver(([entry]) => {
      const was = visible
      visible = entry.isIntersecting && !document.hidden
      if (visible && !was) {
        last = performance.now()
        nextFlick = Math.max(nextFlick, last + 600)
        raf = requestAnimationFrame(frame)
      }
    })
    io.observe(host)

    const onDown = (e: PointerEvent) => {
      drag.active = true
      drag.id = e.pointerId
      drag.x = e.clientX
      drag.y = e.clientY
      drag.moved = 0
      drag.last = performance.now()
      rollVel = 0
      canvas.setPointerCapture(e.pointerId)
      host.style.cursor = "grabbing"
    }
    const onMove = (e: PointerEvent) => {
      if (!drag.active || e.pointerId !== drag.id) return
      const dx = e.clientX - drag.x
      const dy = e.clientY - drag.y
      drag.x = e.clientX
      drag.y = e.clientY
      drag.moved += Math.hypot(dx, dy)
      // A drag across the handle (screen ↘) rolls the front face the same way.
      const delta = ((dx + dy) / Math.SQRT2) * 0.022
      const now = performance.now()
      const dts = Math.max(0.008, (now - drag.last) / 1000)
      drag.last = now
      roll += delta
      rollTarget = roll
      rollVel = rollVel * 0.6 + (delta / dts) * 0.4
    }
    const onUp = (e: PointerEvent) => {
      if (!drag.active || e.pointerId !== drag.id) return
      drag.active = false
      host.style.cursor = "grab"
      const now = performance.now()
      if (drag.moved < 5) {
        flick(now, false)
      } else {
        if (now - drag.last > 90) rollVel = 0
        rollTarget = Math.round((roll + rollVel * 0.3) / TAU) * TAU
      }
      last = now
      nextFlick = now + FLICK_EVERY
    }
    canvas.addEventListener("pointerdown", onDown)
    canvas.addEventListener("pointermove", onMove)
    canvas.addEventListener("pointerup", onUp)
    canvas.addEventListener("pointercancel", onUp)

    return () => {
      applyRef.current = null
      cancelAnimationFrame(raf)
      clearTimeout(revealTimer)
      visible = false
      live.removeEventListener("transitionend", revealed)
      ro.disconnect()
      io.disconnect()
      canvas.removeEventListener("pointerdown", onDown)
      canvas.removeEventListener("pointermove", onMove)
      canvas.removeEventListener("pointerup", onUp)
      canvas.removeEventListener("pointercancel", onUp)
      handle.dispose()
      bristles.dispose()
      metal.dispose()
      paint.dispose()
      trail.dispose()
      drops.dispose()
      envMap.dispose()
      pmrem.dispose()
      renderer.dispose()
      canvas.remove()
    }
  }, [])

  useEffect(() => applyRef.current?.(values), [values])

  useEffect(() => {
    const host = hostRef.current
    if (!host || !brushControls.enabled) return
    const detach = brushControls.attach(host)
    let detachSwitcher: (() => void) | undefined
    let disposed = false
    if (import.meta.env.DEV)
      void import("knobs/panel").then(({ attachVariantSwitcher }) => {
        if (!disposed) detachSwitcher = attachVariantSwitcher(host, brushControls)
      })
    return () => {
      disposed = true
      detach()
      detachSwitcher?.()
    }
  }, [failed])

  if (failed) return <BrushFallback />

  return (
    <div ref={hostRef} style={values.scale === 1 ? hostStyle : { ...hostStyle, transform: `scale(${values.scale})` }} role="img" aria-label="Scrawl">
      <div ref={flatRef} style={flatStyle}>
        <BrushIcon />
      </div>
      <div ref={liveRef} style={liveStyle}>
        <div ref={glowRef} style={glowStyle} />
      </div>
    </div>
  )
}
