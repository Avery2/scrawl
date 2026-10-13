import { useEffect, useRef, useState, type CSSProperties } from "react"
import * as THREE from "three"
import { RoomEnvironment } from "three/examples/jsm/environments/RoomEnvironment.js"
import { toCreasedNormals } from "three/examples/jsm/utils/BufferGeometryUtils.js"
import { BRISTLES, HANDLE, type Outline } from "./brush-paths"

const TAU = Math.PI * 2
const AXIS = new THREE.Vector3(1, 1, 0).normalize()
const PERP = new THREE.Vector3(-1, 1, 0).normalize()
const PAINT = 0xff3b30
const WIDTH = 184
const HEIGHT = 132
const FLICK_EVERY = 5000
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

const toLocal = ([x, y]: readonly [number, number]) => new THREE.Vector2(x - 512, 512 - y)

// Fillet every corner with a true circular arc so bevels and normals stay crease-free. Straight runs
// stay single segments: ExtrudeGeometry reuses the outline's triangulation for the inset caps, and
// collinear points there become slivers that fold out through the bevel. (A quadratic fillet is also
// much tighter than its nominal radius at the apex, which folds the inset bevel the same way.)
function rounded(outline: Outline, radius: number) {
  const pts = outline.map(toLocal)
  const n = pts.length
  const corners = pts.map((p, i) => {
    const da = pts[(i + n - 1) % n].clone().sub(p)
    const db = pts[(i + 1) % n].clone().sub(p)
    const la = da.length()
    const lb = db.length()
    da.divideScalar(la)
    db.divideScalar(lb)
    const half = Math.acos(THREE.MathUtils.clamp(da.dot(db), -1, 1)) / 2
    const d = Math.min(radius / Math.tan(half), la * 0.45, lb * 0.45)
    const r = d * Math.tan(half)
    const center = p.clone().addScaledVector(da.clone().add(db).normalize(), r / Math.sin(half))
    const a = p.clone().addScaledVector(da, d)
    const b = p.clone().addScaledVector(db, d)
    return { a, b, center, r, sweep: Math.PI - 2 * half, turn: Math.sign(da.cross(db)) }
  })
  const out: THREE.Vector2[] = []
  corners.forEach((c) => {
    const start = Math.atan2(c.a.y - c.center.y, c.a.x - c.center.x)
    const arc = Math.max(2, Math.ceil(c.sweep / (Math.PI / 24)))
    for (let s = 0; s <= arc; s++) {
      const angle = start - c.turn * c.sweep * (s / arc)
      out.push(new THREE.Vector2(c.center.x + Math.cos(angle) * c.r, c.center.y + Math.sin(angle) * c.r))
    }
  })
  return new THREE.Shape(out)
}

function slab(outline: Outline, radius: number, depth: number, bevel: number) {
  // A negative offset keeps the side walls on the outline and rounds the caps inward.
  const inset = bevel * 0.8
  const geometry = new THREE.ExtrudeGeometry(rounded(outline, radius), {
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

const HANDLE_SLAB = { radius: 20, depth: 34, bevel: 15 }
const BRISTLE_SLAB = { radius: 16, depth: 30, bevel: 14 }

function buildBrush() {
  const handle = slab(HANDLE, HANDLE_SLAB.radius, HANDLE_SLAB.depth, HANDLE_SLAB.bevel)
  const bristles = slab(BRISTLES, BRISTLE_SLAB.radius, BRISTLE_SLAB.depth, BRISTLE_SLAB.bevel)

  const [uMin, uMax] = [span(handle, AXIS), span(bristles, AXIS)].reduce((a, b) => [Math.min(a[0], b[0]), Math.max(a[1], b[1])])
  const [wMin, wMax] = span(handle, PERP)
  const scale = 2.15 / ((uMax - uMin) / 2)
  const handleZ = depths(handle)
  const bristleZ = depths(bristles)

  // The bristles thin toward the tip like a real flat brush.
  const taper = 0.4
  const [tip, base] = span(bristles, AXIS)
  const [bwMin, bwMax] = span(bristles, PERP)
  const pos = bristles.getAttribute("position")
  const v = new THREE.Vector3()
  for (let i = 0; i < pos.count; i++) {
    v.fromBufferAttribute(pos, i)
    pos.setZ(i, v.z * (1 - (taper * (base - v.dot(AXIS))) / (base - tip)))
  }
  const slope = ((BRISTLE_SLAB.depth / 2 + BRISTLE_SLAB.bevel) * taper) / (base - tip)

  // Normals are welded by position hash, so build them at icon scale where vertices are well apart.
  toCreasedNormals(handle, Math.PI / 3)
  toCreasedNormals(bristles, Math.PI / 3)
  roundEdges(handle, handleZ, HANDLE_SLAB, 0, (wMin + wMax) / 2, 0.5 * scale)
  roundEdges(bristles, bristleZ, BRISTLE_SLAB, slope, (bwMin + bwMax) / 2, 0.35 * scale)

  // Center on the handle's centerline so a roll about AXIS twirls the brush in place.
  const center = AXIS.clone()
    .multiplyScalar((uMin + uMax) / 2)
    .addScaledVector(PERP, (wMin + wMax) / 2)
  for (const g of [handle, bristles]) {
    g.translate(-center.x, -center.y, 0)
    g.scale(scale, scale, scale)
  }
  const l = toLocal(TIP)
  const brushTip = new THREE.Vector3(l.x, l.y, 0).sub(center).multiplyScalar(scale)
  return { handle, bristles, base: (base - center.dot(AXIS)) * scale, tip: (tip - center.dot(AXIS)) * scale, brushTip }
}

function span(g: THREE.BufferGeometry, along: THREE.Vector3) {
  const pos = g.getAttribute("position")
  const v = new THREE.Vector3()
  let min = Infinity
  let max = -Infinity
  for (let i = 0; i < pos.count; i++) {
    const u = v.fromBufferAttribute(pos, i).dot(along)
    min = Math.min(min, u)
    max = Math.max(max, u)
  }
  return [min, max]
}

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

// Charcoal bristles near the ferrule, dipped in wet red paint toward the tip, with fine grooves.
function bristleMaterial(base: number, tip: number) {
  const material = new THREE.MeshPhysicalMaterial({
    color: PAINT,
    roughness: 0.3,
    metalness: 0,
    clearcoat: 0.6,
    clearcoatRoughness: 0.06,
    envMapIntensity: 0.4,
  })
  material.onBeforeCompile = (shader) => {
    Object.assign(shader.uniforms, {
      uAxis: { value: AXIS },
      uPerp: { value: PERP },
      uBase: { value: base },
      uTip: { value: tip },
      uDry: { value: new THREE.Color(0x2c2f36) },
    })
    shader.vertexShader = shader.vertexShader
      .replace(
        "#include <common>",
        "#include <common>\nuniform vec3 uPerp;\nvarying vec3 vObj;\nvarying vec3 vPerpView;\nvarying float vCap;",
      )
      .replace(
        "#include <begin_vertex>",
        "#include <begin_vertex>\nvObj = position;\nvPerpView = normalize(normalMatrix * uPerp);\nvCap = abs(normal.z);",
      )
    shader.fragmentShader = shader.fragmentShader
      .replace(
        "#include <common>",
        `#include <common>
uniform vec3 uAxis;
uniform vec3 uPerp;
uniform float uBase;
uniform float uTip;
uniform vec3 uDry;
varying vec3 vObj;
varying vec3 vPerpView;
varying float vCap;
float wetness() {
  float w = dot(vObj, uPerp);
  float u = (dot(vObj, uAxis) - uBase) / (uTip - uBase);
  u += 0.035 * sin(w * 9.0 + 0.6) + 0.02 * sin(w * 23.0);
  return smoothstep(0.26, 0.5, u);
}`,
      )
      .replace(
        "#include <color_fragment>",
        "#include <color_fragment>\ndiffuseColor.rgb = mix(uDry, diffuseColor.rgb, wetness());",
      )
      .replace(
        "#include <roughnessmap_fragment>",
        "#include <roughnessmap_fragment>\nroughnessFactor = mix(0.5, roughnessFactor, wetness());",
      )
      .replace(
        "#include <metalnessmap_fragment>",
        "#include <metalnessmap_fragment>\nmetalnessFactor = mix(0.55, metalnessFactor, wetness());",
      )
      .replace(
        "#include <normal_fragment_maps>",
        `#include <normal_fragment_maps>
float groove = dot(vObj, uPerp) * 34.0;
float g = sin(groove) * 0.7 + sin(groove * 2.3 + 1.7) * 0.3;
normal = normalize(normal + vPerpView * g * 0.32 * vCap * (1.0 - 0.7 * wetness()));`,
      )
      .replace("material.clearcoat = clearcoat;", "material.clearcoat = clearcoat * wetness();")
  }
  return material
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

  const update = (tip: THREE.Vector3, ink: number, now: number) => {
    if (ink > 0.01) samples.push({ p: tip.clone(), t: now, ink })
    while (samples.length && (now - samples[0].t > TRAIL_LIFE || samples.length > TRAIL_MAX)) samples.shift()
    const pos = geometry.getAttribute("position") as THREE.BufferAttribute
    const col = geometry.getAttribute("color") as THREE.BufferAttribute
    const n = samples.length
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
  let next = 0
  const emit = (at: THREE.Vector3, vel: THREE.Vector3) => {
    const d = drops[next]
    next = (next + 1) % DROPS
    d.p.copy(at)
    d.v
      .copy(vel)
      .multiplyScalar(0.55 + Math.random() * 0.35)
      .add(new THREE.Vector3((Math.random() - 0.5) * 1.2, Math.random() * 1.4, (Math.random() - 0.5) * 0.6))
    d.life = 1
    d.size = 0.045 + Math.random() * 0.04
  }
  const update = (dt: number) => {
    drops.forEach((d, i) => {
      if (d.life > 0) {
        d.life -= dt / 0.55
        d.v.y -= GRAVITY * 0.5 * dt
        d.p.addScaledVector(d.v, dt)
      }
      const k = Math.max(0, d.life)
      s.setScalar(d.size * Math.sqrt(k))
      // Stretch each drop along its flight path.
      q.setFromUnitVectors(new THREE.Vector3(0, 1, 0), d.v.lengthSq() > 1e-6 ? d.v.clone().normalize() : new THREE.Vector3(0, 1, 0))
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
  position: "relative",
  width: WIDTH,
  height: HEIGHT,
  margin: "-26px 0 -18px",
  cursor: "grab",
  touchAction: "none",
  userSelect: "none",
  WebkitUserSelect: "none",
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

export function Brush3D() {
  const hostRef = useRef<HTMLDivElement>(null)
  const glowRef = useRef<HTMLDivElement>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    const host = hostRef.current
    if (!host) return

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
    Object.assign(canvas.style, {
      position: "absolute",
      inset: "0",
      width: "100%",
      height: "100%",
      opacity: "0",
      transform: "scale(0.92)",
      transition: "opacity 0.5s ease, transform 0.7s cubic-bezier(0.2, 0.9, 0.3, 1.2)",
    })
    host.appendChild(canvas)

    const scene = new THREE.Scene()
    const pmrem = new THREE.PMREMGenerator(renderer)
    const room = new RoomEnvironment()
    const envMap = pmrem.fromScene(room, 0.04).texture
    room.dispose()
    scene.environment = envMap
    scene.environmentIntensity = 1.1

    const camera = new THREE.PerspectiveCamera(34, WIDTH / HEIGHT, 0.1, 100)
    camera.position.set(0, 0, 10)

    const key = new THREE.DirectionalLight(0xffffff, 1.1)
    key.position.set(-4, 6, 6)
    const rim = new THREE.DirectionalLight(0xff8a80, 1.2)
    rim.position.set(5, -3, -4)
    scene.add(key, rim)

    const { handle, bristles, base, tip, brushTip } = buildBrush()
    const metal = new THREE.MeshPhysicalMaterial({
      color: 0xe4e7ee,
      metalness: 1,
      roughness: 0.14,
      clearcoat: 0.8,
      clearcoatRoughness: 0.08,
    })
    const paint = bristleMaterial(base, tip)

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
    let nextFlick = performance.now() + 1200
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

    const resize = () => {
      const { width, height } = host.getBoundingClientRect()
      if (!width || !height) return
      renderer.setSize(width, height, false)
      camera.aspect = width / height
      camera.updateProjectionMatrix()
    }
    resize()
    const ro = new ResizeObserver(resize)
    ro.observe(host)

    const frame = (now: number) => {
      const dt = Math.min(0.05, (now - last) / 1000)
      last = now

      if (!reduce && !drag.active && now >= nextFlick) {
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
      const idle = reduce ? 0 : 1
      root.position.y = idle * Math.sin(s * 1.25) * 0.06 + hop
      root.rotation.x = -0.2 + idle * Math.sin(s * 0.7) * 0.08
      root.rotation.y = 0.24 + idle * Math.sin(s * 0.55 + 1) * 0.12
      root.rotation.z = idle * Math.sin(s * 0.9) * 0.03
      pivot.rotation.z = swing
      spin.quaternion.setFromAxisAngle(AXIS, roll)
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
        canvas.style.opacity = "1"
        canvas.style.transform = "none"
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
      cancelAnimationFrame(raf)
      visible = false
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

  if (failed)
    return (
      <div style={{ ...hostStyle, display: "grid", placeItems: "center", cursor: "default" }}>
        <img src="/app-icon.png" alt="Scrawl" width={76} height={76} />
      </div>
    )

  return (
    <div ref={hostRef} style={hostStyle} role="img" aria-label="Scrawl">
      <div ref={glowRef} style={glowStyle} />
    </div>
  )
}
