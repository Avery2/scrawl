import { memo } from "react"
import { getStroke } from "perfect-freehand"
import { INK, type Point } from "./script"

const avg = (a: number, b: number) => (a + b) / 2

function outlinePath(points: number[][]) {
  const len = points.length
  if (len < 4) return ""
  let a = points[0]
  let b = points[1]
  const c = points[2]
  let d = `M${a[0].toFixed(2)},${a[1].toFixed(2)} Q${b[0].toFixed(2)},${b[1].toFixed(2)} ${avg(b[0], c[0]).toFixed(2)},${avg(b[1], c[1]).toFixed(2)}`
  for (let i = 2; i < len - 1; i++) {
    a = points[i]
    b = points[i + 1]
    d += ` Q${a[0].toFixed(2)},${a[1].toFixed(2)} ${avg(a[0], b[0]).toFixed(2)},${avg(a[1], b[1]).toFixed(2)}`
  }
  return d + " Z"
}

// A uniform, round-capped marker line. Scripted strokes are analytical curves (`streamline = 0`)
// so the growing ink tip never lags or steps at segment boundaries; user strokes smooth raw input.
export function inkPath(points: (Point | [number, number, number])[], size = 4.5, streamline = 0) {
  return outlinePath(
    getStroke(points, {
      size,
      thinning: 0.12,
      smoothing: 0.5,
      streamline,
      simulatePressure: false,
      last: true,
      start: { cap: true, taper: 0 },
      end: { cap: true, taper: 0 },
    }),
  )
}

export const InkPath = memo(function InkPath({ d }: { d: string }) {
  if (!d) return null
  return <path d={d} fill={INK} />
})

// Strokes share one shadow at the layer level so overlapping strokes read as one shape.
export const INK_SHADOW = { filter: "drop-shadow(0 1px 2px rgba(0, 0, 0, 0.35))" }

export const PinMark = memo(function PinMark({ n, at, scale = 1 }: { n: number; at: Point; scale?: number }) {
  return (
    <g transform={`translate(${at[0]} ${at[1]}) scale(${scale})`}>
      <circle r={10} fill={INK} stroke="#fff" strokeWidth={2} />
      <text y={4.2} textAnchor="middle" fontSize={11.5} fontWeight={700} fill="#fff" fontFamily="-apple-system, system-ui, sans-serif">
        {n}
      </text>
    </g>
  )
})
