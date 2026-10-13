import { useEffect, useRef, useState } from "react"
import * as THREE from "three"
import { SVGLoader } from "three/examples/jsm/loaders/SVGLoader.js"
import { toCreasedNormals } from "three/examples/jsm/utils/BufferGeometryUtils.js"
import { BRISTLES_PATH, HANDLE_PATH } from "./brush-paths"

const TAU = Math.PI * 2
const AXIS = new THREE.Vector3(1, 1, 0).normalize()
const SPIN_EVERY = 3800
const STIFFNESS = 46
const DAMPING = 2 * Math.sqrt(STIFFNESS) * 0.58

function extrude(d: string, depth: number) {
  const { paths } = new SVGLoader().parse(`<svg xmlns="http://www.w3.org/2000/svg"><path d="${d}"/></svg>`)
  const shapes = paths.flatMap((p) => p.toShapes())
  const geometry = new THREE.ExtrudeGeometry(shapes, {
    depth,
    curveSegments: 6,
    bevelEnabled: true,
    bevelThickness: 90,
    bevelSize: 60,
    bevelOffset: 0,
    bevelSegments: 8,
  })
  geometry.translate(0, 0, -depth / 2)
  // Smooth across the bevel segments while keeping the cap edges crisp.
  const creased = toCreasedNormals(geometry, Math.PI / 5)
  geometry.dispose()
  return creased
}

// A graded studio dome with softbox strips gives the metal reflections from every roll angle.
function studio() {
  const scene = new THREE.Scene()
  const dome = new THREE.SphereGeometry(20, 48, 24)
  const pos = dome.getAttribute("position")
  const colors: number[] = []
  const top = new THREE.Color(0xf2f4f8).multiplyScalar(1.1)
  const horizon = new THREE.Color(0x8a8f99).multiplyScalar(0.3)
  const floor = new THREE.Color(0x2a1a1c).multiplyScalar(0.5)
  const c = new THREE.Color()
  for (let i = 0; i < pos.count; i++) {
    const y = pos.getY(i) / 20
    if (y >= 0) c.lerpColors(horizon, top, Math.pow(y, 0.7))
    else c.lerpColors(horizon, floor, Math.pow(-y, 0.5))
    colors.push(c.r, c.g, c.b)
  }
  dome.setAttribute("color", new THREE.Float32BufferAttribute(colors, 3))
  const domeMaterial = new THREE.MeshBasicMaterial({ vertexColors: true, side: THREE.BackSide })
  scene.add(new THREE.Mesh(dome, domeMaterial))

  const box = new THREE.BoxGeometry(1, 1, 1)
  const materials: THREE.Material[] = [domeMaterial]
  const panel = (color: number, intensity: number, position: [number, number, number], size: [number, number, number]) => {
    const material = new THREE.MeshBasicMaterial({ color: new THREE.Color(color).multiplyScalar(intensity) })
    materials.push(material)
    const mesh = new THREE.Mesh(box, material)
    mesh.position.set(...position)
    mesh.scale.set(...size)
    scene.add(mesh)
  }
  panel(0xffffff, 4, [0, 9, 1], [12, 0.2, 5])
  panel(0xffffff, 5, [-8, 1, 3], [0.2, 9, 1.4])
  panel(0xdce6ff, 3.5, [8, 2, -1], [0.2, 7, 1.2])
  panel(0xffffff, 3, [-5, 6, 7], [2.2, 0.5, 0.2])
  panel(0xff4a3a, 1.4, [0, -8, 3], [10, 0.2, 5])
  const dispose = () => {
    dome.dispose()
    box.dispose()
    materials.forEach((m) => m.dispose())
  }
  return { scene, dispose }
}

function buildBrush() {
  const handle = extrude(HANDLE_PATH, 460)
  const bristles = extrude(BRISTLES_PATH, 560)

  // Center on the handle's diagonal centerline so a rotation about AXIS twirls the brush in place.
  const perp = new THREE.Vector3(-1, 1, 0).normalize()
  let uMin = Infinity
  let uMax = -Infinity
  let vMin = Infinity
  let vMax = -Infinity
  const v = new THREE.Vector3()
  for (const g of [handle, bristles]) {
    const pos = g.getAttribute("position")
    for (let i = 0; i < pos.count; i++) {
      v.fromBufferAttribute(pos, i)
      const u = v.dot(AXIS)
      const w = v.dot(perp)
      uMin = Math.min(uMin, u)
      uMax = Math.max(uMax, u)
      vMin = Math.min(vMin, w)
      vMax = Math.max(vMax, w)
    }
  }
  const center = AXIS.clone()
    .multiplyScalar((uMin + uMax) / 2)
    .addScaledVector(perp, (vMin + vMax) / 2)
  const halfLength = (uMax - uMin) / 2
  const scale = 2.15 / halfLength
  for (const g of [handle, bristles]) {
    g.translate(-center.x, -center.y, 0)
    g.scale(scale, scale, scale)
  }
  return { handle, bristles }
}

export function Brush3D() {
  const hostRef = useRef<HTMLDivElement>(null)
  const [failed, setFailed] = useState(false)
  const [ready, setReady] = useState(false)

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
    renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2))
    renderer.toneMapping = THREE.NeutralToneMapping
    renderer.toneMappingExposure = 1
    renderer.setClearColor(0x000000, 0)
    const canvas = renderer.domElement
    host.appendChild(canvas)

    const scene = new THREE.Scene()
    const pmrem = new THREE.PMREMGenerator(renderer)
    const env = studio()
    const envMap = pmrem.fromScene(env.scene, 0.02).texture
    env.dispose()
    scene.environment = envMap

    const camera = new THREE.PerspectiveCamera(30, 1, 0.1, 100)
    camera.position.set(0, 0, 10)

    const key = new THREE.DirectionalLight(0xffffff, 1.8)
    key.position.set(-4, 5, 6)
    const rim = new THREE.DirectionalLight(0xff8f86, 1.4)
    rim.position.set(5, -3, -4)
    scene.add(key, rim)

    const { handle, bristles } = buildBrush()
    const metal = new THREE.MeshPhysicalMaterial({
      color: 0xd9dde4,
      metalness: 1,
      roughness: 0.26,
      clearcoat: 1,
      clearcoatRoughness: 0.04,
    })
    const paint = new THREE.MeshPhysicalMaterial({
      color: 0xff3b30,
      metalness: 0,
      roughness: 0.42,
      clearcoat: 0.55,
      clearcoatRoughness: 0.14,
      sheen: 0.3,
      sheenColor: new THREE.Color(0xff6a5a),
      sheenRoughness: 0.5,
      envMapIntensity: 0.4,
    })

    const root = new THREE.Group()
    const spin = new THREE.Group()
    spin.add(new THREE.Mesh(handle, metal), new THREE.Mesh(bristles, paint))
    root.add(spin)
    scene.add(root)

    const reduce = window.matchMedia("(prefers-reduced-motion: reduce)").matches
    let angle = 0
    let vel = 0
    let target = 0
    let nextAuto = performance.now() + 1400
    let visible = true
    let raf = 0
    let last = performance.now()
    let firstFrame = true

    const drag = { active: false, id: -1, x: 0, y: 0, moved: 0, last: 0 }

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

      if (!drag.active) {
        if (!reduce && now >= nextAuto) {
          target = Math.round(target / TAU) * TAU + TAU
          nextAuto = now + SPIN_EVERY
        }
        // Semi-implicit spring integration in small steps keeps it stable at low frame rates.
        const steps = Math.ceil(dt / (1 / 240))
        const h = dt / steps
        for (let i = 0; i < steps; i++) {
          vel += (STIFFNESS * (target - angle) - DAMPING * vel) * h
          angle += vel * h
        }
        if (Math.abs(target - angle) < 1e-4 && Math.abs(vel) < 1e-4) {
          const wraps = Math.round(target / TAU) * TAU
          angle -= wraps
          target -= wraps
          vel = 0
        }
      }

      const s = now / 1000
      const idle = reduce ? 0 : 1
      root.position.y = idle * Math.sin(s * 1.25) * 0.08
      root.rotation.x = -0.18 + idle * Math.sin(s * 0.7) * 0.09
      root.rotation.y = 0.22 + idle * Math.sin(s * 0.55 + 1) * 0.14
      root.rotation.z = idle * Math.sin(s * 0.9) * 0.03 - vel * 0.006
      spin.quaternion.setFromAxisAngle(AXIS, angle)

      renderer.render(scene, camera)
      if (firstFrame) {
        firstFrame = false
        setReady(true)
      }
      if (visible) raf = requestAnimationFrame(frame)
    }
    raf = requestAnimationFrame(frame)

    const io = new IntersectionObserver(([entry]) => {
      const was = visible
      visible = entry.isIntersecting && !document.hidden
      if (visible && !was) {
        last = performance.now()
        nextAuto = Math.max(nextAuto, last + 600)
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
      vel = 0
      canvas.setPointerCapture(e.pointerId)
      host.classList.add("grabbing")
    }
    const onMove = (e: PointerEvent) => {
      if (!drag.active || e.pointerId !== drag.id) return
      const dx = e.clientX - drag.x
      const dy = e.clientY - drag.y
      drag.x = e.clientX
      drag.y = e.clientY
      drag.moved += Math.hypot(dx, dy)
      // A drag across the handle (screen ↘) rolls the front face the same way.
      const delta = ((dx + dy) / Math.SQRT2) * 0.016
      const now = performance.now()
      const dts = Math.max(0.008, (now - drag.last) / 1000)
      drag.last = now
      angle += delta
      target = angle
      vel = vel * 0.6 + (delta / dts) * 0.4
    }
    const onUp = (e: PointerEvent) => {
      if (!drag.active || e.pointerId !== drag.id) return
      drag.active = false
      host.classList.remove("grabbing")
      if (drag.moved < 5) {
        vel += 4
        target = (Math.round(angle / TAU) + 1) * TAU
      } else {
        if (performance.now() - drag.last > 90) vel = 0
        target = Math.round((angle + vel * 0.3) / TAU) * TAU
      }
      last = performance.now()
      nextAuto = last + SPIN_EVERY
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
      envMap.dispose()
      pmrem.dispose()
      renderer.dispose()
      canvas.remove()
    }
  }, [])

  if (failed)
    return (
      <div className="hero-brush">
        <img className="hero-icon" src="/app-icon.png" alt="Scrawl" />
      </div>
    )

  return <div ref={hostRef} className={`hero-brush${ready ? " ready" : ""}`} role="img" aria-label="Scrawl" />
}
