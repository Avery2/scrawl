import { getStroke } from "perfect-freehand"
import { INK, type Point } from "./script"

const avg = (a: number, b: number) => (a + b) / 2

function outlinePath(points: number[][]) {
  const len = points.length
  if (len < 4) return ""
  let a = points[0]
  let b = points[1]
  const c = points[2]
  let d = `M${a[0].toFixed(1)},${a[1].toFixed(1)} Q${b[0].toFixed(1)},${b[1].toFixed(1)} ${avg(b[0], c[0]).toFixed(1)},${avg(b[1], c[1]).toFixed(1)} T`
  for (let i = 2; i < len - 1; i++) {
    a = points[i]
    b = points[i + 1]
    d += `${avg(a[0], b[0]).toFixed(1)},${avg(a[1], b[1]).toFixed(1)} `
  }
  return d + "Z"
}

// A uniform, round-capped marker line. The options never depend on whether the stroke is finished,
// so the last frame of a stroke can't snap to a different shape.
export function inkPath(points: (Point | [number, number, number])[], size = 4.5) {
  return outlinePath(
    getStroke(points, {
      size,
      thinning: 0.25,
      smoothing: 0.7,
      streamline: 0.6,
      simulatePressure: false,
      last: true,
      start: { cap: true, taper: 0 },
      end: { cap: true, taper: 0 },
    }),
  )
}

export function InkPath({ d }: { d: string }) {
  if (!d) return null
  return <path d={d} fill={INK} />
}

// Strokes share one shadow at the layer level so overlapping strokes read as one shape.
export const INK_SHADOW = { filter: "drop-shadow(0 1px 2px rgba(0, 0, 0, 0.35))" }

export function PinMark({ n, at, scale = 1 }: { n: number; at: Point; scale?: number }) {
  return (
    <g transform={`translate(${at[0]} ${at[1]}) scale(${scale})`}>
      <circle r={10} fill={INK} stroke="#fff" strokeWidth={2} />
      <text y={4.2} textAnchor="middle" fontSize={11.5} fontWeight={700} fill="#fff" fontFamily="-apple-system, system-ui, sans-serif">
        {n}
      </text>
    </g>
  )
}
