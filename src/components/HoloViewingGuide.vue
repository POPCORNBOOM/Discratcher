<template>
  <figure class="guide" :aria-label="t('holo.guideTitle')">
    <figcaption class="guide-head">
      <span class="title">{{ t('holo.guideTitle') }}</span>
      <span class="small muted">{{ subText }}</span>
    </figcaption>
    <div ref="box" class="stage" role="img" :aria-label="altText">
      <span
        v-for="l in labels"
        :key="l.key"
        class="tag"
        :class="l.kind"
        :style="{ transform: `translate(${l.x}px, ${l.y}px) translate(-50%, -50%)`, opacity: l.shown ? 1 : 0 }"
      >{{ l.text }}</span>
    </div>
  </figure>
</template>

<script setup lang="ts">
// The viewing set-up in 3D:
// - Real mode: the disc stands facing you, the phone light sits beside your head, turn the disc.
// - Transform mode: the same set-up, one eye or a phone camera, and the disc tilts left and right.
// Units are centimetres; x right, y up, z from the disc toward you.
import {
  BufferGeometry,
  CircleGeometry,
  Color,
  Group,
  Line,
  LineBasicMaterial,
  LineDashedMaterial,
  Mesh,
  MeshBasicMaterial,
  MeshLambertMaterial,
  AmbientLight,
  DirectionalLight,
  PerspectiveCamera,
  RingGeometry,
  Scene,
  SphereGeometry,
  BoxGeometry,
  Vector3,
  WebGLRenderer,
  DoubleSide,
  ConeGeometry,
} from 'three'
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js'
import { computed, onBeforeUnmount, onMounted, reactive, ref } from 'vue'
import { DEFAULT_TRANSFORM_TILT_DEG, LAMP_RIGHT_CM, LAMP_UP_CM } from '../holo/constants'
import type { HoloPositionMode } from '../holo/scene'
import { t } from '../i18n'

const props = withDefaults(defineProps<{ mode?: HoloPositionMode; tiltDeg?: number }>(), { mode: 'real', tiltDeg: DEFAULT_TRANSFORM_TILT_DEG })
const isTransform = computed(() => props.mode === 'transform')
const tiltWords = computed(() => Math.round(props.tiltDeg))

const subText = computed(() =>
  isTransform.value
    ? t('holo.guideTransformSub', { right: LAMP_RIGHT_CM, up: LAMP_UP_CM, n: tiltWords.value })
    : t('holo.guideSub', { right: LAMP_RIGHT_CM, up: LAMP_UP_CM }),
)
const altText = computed(() =>
  isTransform.value
    ? t('holo.guideTransformAlt', { right: LAMP_RIGHT_CM, up: LAMP_UP_CM, n: tiltWords.value })
    : t('holo.guideAlt', { right: LAMP_RIGHT_CM, up: LAMP_UP_CM }),
)

// --oc-* tokens (WebGL needs numbers).
const ACCENT = 0x9045f9
const FG_ACCENT = 0xae79fb
const FG = 0xcccccc
const MUTED = 0x868686
const SUBTLE = 0x606060
const RAISED = 0x323232
const SURFACE = 0x282828

const DISC_R = 6
const HOLE_R = 0.75
const HEAD_Z = 35
const HEAD_R = 8
const LAMP = new Vector3(LAMP_RIGHT_CM, 0, LAMP_UP_CM)
const FLOOR = -13

const box = ref<HTMLDivElement>()
type Label = { key: string; text: string; kind: string; at: Vector3; x: number; y: number; shown: boolean }
const labels = reactive<Label[]>([
        { key: 'disc', text: isTransform.value ? t('holo.guideTiltDisc', { n: tiltWords.value }) : t('holo.guideDisc'), kind: 'strong', at: new Vector3(0, DISC_R + 4, 0), x: 0, y: 0, shown: false },
        { key: 'eye', text: isTransform.value ? t('holo.guideOneEye') : t('holo.guideEye'), kind: 'strong', at: new Vector3(0, HEAD_R + 3, HEAD_Z), x: 0, y: 0, shown: false },
        { key: 'lamp', text: t('holo.guideLamp'), kind: 'strong', at: new Vector3(LAMP.x + 8, -2, LAMP.z + 3), x: 0, y: 0, shown: false },
        { key: 'right', text: t('holo.guideRight', { cm: LAMP_RIGHT_CM }), kind: 'dim', at: new Vector3(LAMP.x / 2, FLOOR - 2.2, LAMP.z + 2), x: 0, y: 0, shown: false },
        { key: 'away', text: `${LAMP_UP_CM} cm`, kind: 'dim', at: new Vector3(LAMP.x + 5, FLOOR, LAMP.z * 0.12), x: 0, y: 0, shown: false },
])

let renderer: WebGLRenderer | null = null
let controls: OrbitControls | null = null
let observer: ResizeObserver | null = null
let frame = 0
const disposables: { dispose(): void }[] = []

function keep<T extends { dispose(): void }>(x: T): T {
  disposables.push(x)
  return x
}

function line(points: Vector3[], color: number, dashed = false, opacity = 1): Line {
  const geo = keep(new BufferGeometry().setFromPoints(points))
  const mat = keep(
    dashed
      ? new LineDashedMaterial({ color, dashSize: 1.2, gapSize: 0.9, transparent: opacity < 1, opacity })
      : new LineBasicMaterial({ color, transparent: opacity < 1, opacity }),
  )
  const l = new Line(geo, mat)
  if (dashed) l.computeLineDistances()
  return l
}

function arc(radius: number, from: number, to: number, z: number, color: number): Line {
  const pts: Vector3[] = []
  for (let i = 0; i <= 48; i++) {
    const a = from + ((to - from) * i) / 48
    pts.push(new Vector3(radius * Math.cos(a), radius * Math.sin(a), z))
  }
  return line(pts, color)
}

function build(scene: Scene, turning: boolean): { spin: Group } {
  // The disc, upright, its face toward you.
  const spin = new Group()
  const face = new Mesh(keep(new RingGeometry(HOLE_R * 2, DISC_R, 96)), keep(new MeshBasicMaterial({ color: RAISED, side: DoubleSide })))
  spin.add(face)
  // A faint sheen and a marker so the turning shows.
  const sheen = new Mesh(
    keep(new RingGeometry(HOLE_R * 2.6, DISC_R * 0.97, 64, 1, -0.35, 0.7)),
    keep(new MeshBasicMaterial({ color: FG_ACCENT, transparent: true, opacity: 0.16, side: DoubleSide })),
  )
  sheen.position.z = 0.02
  spin.add(sheen)
  const mark = new Mesh(keep(new CircleGeometry(0.45, 16)), keep(new MeshBasicMaterial({ color: FG_ACCENT })))
  mark.position.set(0, -DISC_R * 0.82, 0.04)
  spin.add(mark)
  scene.add(spin)
  scene.add(arc(DISC_R, 0, Math.PI * 2, 0.05, ACCENT))
  scene.add(arc(HOLE_R * 2, 0, Math.PI * 2, 0.05, SUBTLE))
  // The turning arrow just outside the rim, clockwise as you see it.
  if (turning) {
    const R = DISC_R + 1.4
    const a0 = Math.PI * 0.95
    const a1 = Math.PI * 0.35
    scene.add(arc(R, a0, a1, 0, FG_ACCENT))
    const head = new Mesh(keep(new ConeGeometry(0.55, 1.4, 12)), keep(new MeshBasicMaterial({ color: FG_ACCENT })))
    head.position.set(R * Math.cos(a1), R * Math.sin(a1), 0)
    head.rotation.z = a1 - Math.PI // point along the clockwise tangent
    scene.add(head)
  }

  // Your head, looking at the disc.
  const skull = new Mesh(keep(new SphereGeometry(HEAD_R, 32, 20)), keep(new MeshLambertMaterial({ color: 0x5a5a5a })))
  skull.position.set(0, 0, HEAD_Z)
  scene.add(skull)
  for (const ex of [-2.6, 2.6]) {
    const eye = new Mesh(keep(new SphereGeometry(0.9, 12, 8)), keep(new MeshBasicMaterial({ color: ACCENT })))
    eye.position.set(ex, 1.2, HEAD_Z - HEAD_R * 0.93)
    scene.add(eye)
  }

  // The phone, torch toward the disc.
  const phone = new Mesh(keep(new BoxGeometry(3.6, 7.4, 0.6)), keep(new MeshBasicMaterial({ color: SURFACE })))
  phone.position.copy(LAMP)
  scene.add(phone)
  const edges = line(
    [
      new Vector3(-1.8, -3.7, -0.31),
      new Vector3(1.8, -3.7, -0.31),
      new Vector3(1.8, 3.7, -0.31),
      new Vector3(-1.8, 3.7, -0.31),
      new Vector3(-1.8, -3.7, -0.31),
    ].map((p) => p.add(LAMP)),
    FG,
  )
  scene.add(edges)
  const torch = new Mesh(keep(new CircleGeometry(0.6, 16)), keep(new MeshBasicMaterial({ color: FG_ACCENT, side: DoubleSide })))
  torch.position.set(LAMP.x - 0.8, LAMP.y + 2.6, LAMP.z - 0.32)
  scene.add(torch)

  // Sight and light.
  scene.add(line([new Vector3(0, 1.2, HEAD_Z - HEAD_R), new Vector3(0, 0, 0.1)], MUTED, true))
  scene.add(line([torch.position.clone(), new Vector3(0, 0, 0.1)], FG_ACCENT, true))

  // Distances on the floor.
  scene.add(line([new Vector3(0, FLOOR, LAMP.z), new Vector3(LAMP.x, FLOOR, LAMP.z)], SUBTLE))
  scene.add(line([new Vector3(LAMP.x, FLOOR, 0), new Vector3(LAMP.x, FLOOR, LAMP.z)], SUBTLE))
  scene.add(line([new Vector3(0, FLOOR, 0), new Vector3(0, FLOOR, HEAD_Z)], SUBTLE, true, 0.6))
  scene.add(line([new Vector3(0, FLOOR, 0), new Vector3(0, -DISC_R, 0)], SUBTLE, true, 0.6))
  scene.add(line([new Vector3(0, FLOOR, HEAD_Z), new Vector3(0, -HEAD_R, HEAD_Z)], SUBTLE, true, 0.6))
  scene.add(line([new Vector3(LAMP.x, FLOOR, LAMP.z), new Vector3(LAMP.x, -3.7, LAMP.z)], SUBTLE, true, 0.6))
  return { spin }
}

onMounted(() => {
  const el = box.value
  if (!el) return
  try {
    renderer = new WebGLRenderer({ antialias: true, alpha: true })
  } catch {
    return
  }
  renderer.setPixelRatio(Math.min(window.devicePixelRatio, 2))
  renderer.setClearColor(new Color(0x000000), 0)
  el.prepend(renderer.domElement)
  const scene = new Scene()
  const camera = new PerspectiveCamera(25, 2, 1, 500)
  // Behind you and up to the right: the disc ahead, the phone light ahead or beside your head.
  camera.position.set(52, 44, 112)
  const target = new Vector3(5, -5, 19)
  scene.add(new AmbientLight(0xffffff, 1.6))
  const sun = new DirectionalLight(0xffffff, 1.8)
  sun.position.set(40, 60, 50)
  scene.add(sun)
  camera.lookAt(target)
  const { spin } = build(scene, !isTransform.value)

  controls = new OrbitControls(camera, renderer.domElement)
  controls.target.copy(target)
  controls.enableZoom = false
  controls.enablePan = false
  controls.enableDamping = true
  controls.minPolarAngle = Math.PI * 0.12
  controls.maxPolarAngle = Math.PI * 0.55
  controls.update()

  const size = () => {
    const w = el.clientWidth
    const h = el.clientHeight
    if (!renderer || !w || !h) return
    renderer.setSize(w, h, false)
    camera.aspect = w / h
    camera.updateProjectionMatrix()
  }
  observer = new ResizeObserver(size)
  observer.observe(el)
  size()

  const v = new Vector3()
  const loop = (now: number) => {
    frame = requestAnimationFrame(loop)
    if (!renderer || document.hidden) return
    // Turned, or in transform mode rocked left and right through the tilt the disc was solved for.
    if (isTransform.value) spin.rotation.y = (props.tiltDeg * Math.sin(now / 600) * Math.PI) / 180
    else spin.rotation.z = -now / 2600
    controls?.update()
    renderer.render(scene, camera)
    const w = el.clientWidth
    const h = el.clientHeight
    for (const l of labels) {
      v.copy(l.at).project(camera)
      l.x = Math.round((v.x * 0.5 + 0.5) * w)
      l.y = Math.round((-v.y * 0.5 + 0.5) * h)
      l.shown = v.z < 1
    }
  }
  frame = requestAnimationFrame(loop)
})

onBeforeUnmount(() => {
  cancelAnimationFrame(frame)
  observer?.disconnect()
  controls?.dispose()
  for (const d of disposables) d.dispose()
  renderer?.dispose()
  renderer?.domElement.remove()
  renderer = null
})

</script>

<style scoped>
.guide {
  margin: 0;
  padding: var(--oc-space-sm) var(--oc-space-md);
  border-radius: var(--oc-radius-control);
  border: 1px solid var(--oc-border-muted);
  background: var(--oc-bg-glass);
  backdrop-filter: blur(16px);
  display: flex;
  flex-direction: column;
  gap: var(--oc-space-xs);
}
.guide-head {
  display: flex;
  justify-content: space-between;
  align-items: baseline;
  gap: var(--oc-space-sm);
}
.stage {
  position: relative;
  width: 100%;
  aspect-ratio: 360 / 170;
  cursor: grab;
  overflow: hidden;
}
.stage:active { cursor: grabbing; }
.stage :deep(canvas) {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  display: block;
}
.tag {
  position: absolute;
  left: 0;
  top: 0;
  pointer-events: none;
  white-space: nowrap;
  transition: opacity 0.15s;
}
.strong { color: var(--oc-fg-default); font-size: 11px; font-weight: 500; }
.dim { color: var(--oc-fg-muted); font-size: 10px; font-family: var(--oc-font-mono); }
</style>
