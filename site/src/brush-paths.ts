import { createElement, type CSSProperties } from "react"

// Outlines of assets/icon.svg, simplified from its potrace paths to their true corners
// (icon pixels, 1024 viewBox, y down). Brush3D fillets and extrudes them; BrushIcon projects the
// same fillets through the same camera, so the flat icon covers the 3D brush's face exactly.
export type Point = readonly [number, number]
export type Outline = readonly Point[]

export const HANDLE: Outline = [
  [774, 183],
  [715, 193],
  [528, 418],
  [413, 391],
  [334, 480],
  [569, 691],
  [653, 600],
  [615, 493],
  [827, 284],
  [829, 229],
]

export const BRISTLES: Outline = [
  [323, 490],
  [195, 623],
  [230, 702],
  [282, 755],
  [342, 734],
  [313, 782],
  [371, 821],
  [442, 842],
  [559, 700],
]

export const VIEW = { width: 184, height: 132, fov: 34, distance: 10 }
export const HANDLE_SLAB = { radius: 20, depth: 34, bevel: 15 }
export const BRISTLE_SLAB = { radius: 16, depth: 30, bevel: 14 }
// The bristles thin toward the tip like a real flat brush.
export const TAPER = 0.4
const HALF_LENGTH = 2.15

export const BRUSH_BOX: CSSProperties = {
  position: "relative",
  width: VIEW.width,
  height: VIEW.height,
  margin: "-26px auto -18px",
}

// Icon pixels to centered, y-up coordinates.
export const toLocal = ([x, y]: Point): Point => [x - 512, 512 - y]
// Distances along the handle (↗ in local space) and across it.
const along = ([x, y]: Point) => (x + y) * Math.SQRT1_2
const across = ([x, y]: Point) => (y - x) * Math.SQRT1_2

// Fillet every corner with a true circular arc so bevels and normals stay crease-free. Straight runs
// stay single segments: ExtrudeGeometry reuses the outline's triangulation for the inset caps, and
// collinear points there become slivers that fold out through the bevel. (A quadratic fillet is also
// much tighter than its nominal radius at the apex, which folds the inset bevel the same way.)
export function fillet(outline: Outline, radius: number): Point[] {
  const pts = outline.map(toLocal)
  const n = pts.length
  const out: Point[] = []
  pts.forEach(([px, py], i) => {
    const [ax, ay] = pts[(i + n - 1) % n]
    const [bx, by] = pts[(i + 1) % n]
    const la = Math.hypot(ax - px, ay - py)
    const lb = Math.hypot(bx - px, by - py)
    const da = [(ax - px) / la, (ay - py) / la]
    const db = [(bx - px) / lb, (by - py) / lb]
    const half = Math.acos(Math.min(1, Math.max(-1, da[0] * db[0] + da[1] * db[1]))) / 2
    const d = Math.min(radius / Math.tan(half), la * 0.45, lb * 0.45)
    const r = d * Math.tan(half)
    const bis = [da[0] + db[0], da[1] + db[1]]
    const bl = Math.hypot(bis[0], bis[1])
    const cx = px + (bis[0] / bl) * (r / Math.sin(half))
    const cy = py + (bis[1] / bl) * (r / Math.sin(half))
    const sweep = Math.PI - 2 * half
    const turn = Math.sign(da[0] * db[1] - da[1] * db[0])
    const start = Math.atan2(py + da[1] * d - cy, px + da[0] * d - cx)
    const arc = Math.max(2, Math.ceil(sweep / (Math.PI / 24)))
    for (let s = 0; s <= arc; s++) {
      const angle = start - turn * sweep * (s / arc)
      out.push([cx + Math.cos(angle) * r, cy + Math.sin(angle) * r])
    }
  })
  return out
}

function span(points: readonly Point[], f: (p: Point) => number) {
  let min = Infinity
  let max = -Infinity
  for (const p of points) {
    min = Math.min(min, f(p))
    max = Math.max(max, f(p))
  }
  return [min, max] as const
}

// The extruded walls sit exactly on these fillets (the caps are inset), so their spans are the mesh's.
function layout() {
  const handle = fillet(HANDLE, HANDLE_SLAB.radius)
  const bristles = fillet(BRISTLES, BRISTLE_SLAB.radius)
  const [hu0, hu1] = span(handle, along)
  const [tip, base] = span(bristles, along)
  const uMin = Math.min(hu0, tip)
  const uMax = Math.max(hu1, base)
  const [wMin, wMax] = span(handle, across)
  const [bwMin, bwMax] = span(bristles, across)
  const cu = (uMin + uMax) / 2
  const cw = (wMin + wMax) / 2
  return {
    handle,
    bristles,
    scale: HALF_LENGTH / ((uMax - uMin) / 2),
    // Centered on the handle's centerline so a roll about the axis twirls the brush in place.
    center: [(cu - cw) * Math.SQRT1_2, (cu + cw) * Math.SQRT1_2] as Point,
    handleMid: cw,
    bristleMid: (bwMin + bwMax) / 2,
    tip,
    base,
  }
}

export const BRUSH = layout()

// The face-on brush as the camera sees it: each fillet at its front wall's depth, through the
// perspective camera, in CSS pixels.
function project(points: readonly Point[], depth: (p: Point) => number) {
  const { width, height, fov, distance } = VIEW
  const focal = height / 2 / Math.tan((fov * Math.PI) / 360)
  const { scale, center } = BRUSH
  return (
    points
      .map((p, i) => {
        const k = focal / (distance - depth(p) * scale)
        const x = width / 2 + (p[0] - center[0]) * scale * k
        const y = height / 2 - (p[1] - center[1]) * scale * k
        return `${i ? "L" : "M"}${x.toFixed(2)} ${y.toFixed(2)}`
      })
      .join("") + "Z"
  )
}

const HANDLE_PATH = project(BRUSH.handle, () => HANDLE_SLAB.depth / 2)
const BRISTLES_PATH = project(
  BRUSH.bristles,
  (p) => (BRISTLE_SLAB.depth / 2) * (1 - (TAPER * (BRUSH.base - along(p))) / (BRUSH.base - BRUSH.tip)),
)

export function BrushIcon({ style }: { style?: CSSProperties }) {
  return createElement(
    "svg",
    {
      width: VIEW.width,
      height: VIEW.height,
      viewBox: `0 0 ${VIEW.width} ${VIEW.height}`,
      "aria-hidden": true,
      style: { position: "absolute", inset: 0, display: "block", pointerEvents: "none", ...style },
    },
    createElement("path", { d: HANDLE_PATH, fill: "#d4d6dc" }),
    createElement("path", { d: BRISTLES_PATH, fill: "#9ea2ae" }),
  )
}

// The flat icon in the 3D brush's place, light enough to render before three.js loads.
export function BrushFallback() {
  return createElement("div", { style: BRUSH_BOX, role: "img", "aria-label": "Scrawl" }, createElement(BrushIcon))
}
