<template>
  <div ref="host" class="preview" :class="{ dragging }" @pointerdown="down" @pointermove="move" @pointerup="up" @pointercancel="up" @pointerleave="leave" @wheel.prevent="wheel">
    <canvas ref="canvas" />
    <div v-if="error" class="notice warn fallback">{{ error }}</div>
  </div>
</template>

<script setup lang="ts">
// The solved disc, drawn on the graphics card: every pixel adds up what its cell's sequences
// look like from where the eye and the lamp are, the same sum as design::render.
import { Camera, DataArrayTexture, DataTexture, DataUtils, GLSL3, Group, HalfFloatType, LinearFilter, Mesh, NearestFilter, PerspectiveCamera, RawShaderMaterial, RGBAFormat, RingGeometry, Scene, Vector3, WebGLRenderer } from 'three'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { DEFAULT_TRANSFORM_TILT_DEG, transformTiltDeg, type Vec3 } from '../holo/constants'
import type { HoloMode } from '../holo/scene'
import { t } from '../i18n'

const props = withDefaults(defineProps<{
  active: boolean
  weights: ArrayBuffer | null
  tables: ArrayBuffer | null
  innerRadiusMm: number
  outerRadiusMm: number
  lampMm: Vec3
  distanceMm: number
  mode?: HoloMode
  /** How far the disc tilts either way in transform mode, degrees. */
  tiltDeg?: number
}>(), {
  mode: 'real',
  tiltDeg: DEFAULT_TRANSFORM_TILT_DEG,
})
/** The angle of the disc at the top, degrees. */
const turn = defineModel<number>('turn', { required: true })
/** Shared animation parameter t in [0, 1] for transform mode: the tilt from -tiltDeg to +tiltDeg. */
const previewT = defineModel<number>('previewT', { default: 0.5 })

const MAX_TILT = 5
/** Share of what is left to go, per second: how quickly the disc follows. */
const FOLLOW = 14
const host = ref<HTMLElement | null>(null)
const canvas = ref<HTMLCanvasElement | null>(null)
const dragging = ref(false)
const error = ref('')

let renderer: WebGLRenderer | null = null
let scene: Scene | null = null
let camera: PerspectiveCamera | null = null
let disc: Group | null = null
let material: RawShaderMaterial | null = null
let observer: ResizeObserver | null = null
let raf = 0
let last = 0
let tilt = 0
let wantTilt = 0
let shownTurn = turn.value
let dirty = true


const VERTEX = `
precision highp float;
in vec3 position;
uniform mat4 modelViewMatrix, projectionMatrix;
out vec2 at;
void main() { at = position.xy; gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0); }
`
const FRAGMENT = `
precision highp float;
precision highp sampler2DArray;
in vec2 at;
out vec4 color;
uniform sampler2DArray weights, looks;
uniform sampler2D groove;
uniform vec3 eye, lamp;
uniform float outer, gMax, spread, plain, carveBase, carveLevels;
uniform vec2 bins;
void main() {
  vec3 p = vec3(at, 0.0);
  vec3 g = normalize(lamp - p) + normalize(eye - p);
  vec2 r = normalize(at);
  float gt = abs(-g.x * r.y + g.y * r.x), gr = abs(dot(g.xy, r));
  vec2 cell = vec2(at.x / (2.0 * outer) + 0.5, 0.5 - at.y / (2.0 * outer));
  // Bin centres, as Responses::at has them.
  vec2 look = (min(vec2(gr, gt) / gMax, 1.0) * (bins - 1.0) + 0.5) / bins;
  vec3 c = vec3(0.0);
  for (int l = 0; l < LAYERS; l++) {
    vec4 w = textureLod(weights, vec3(cell, float(l)), 0.0);
    for (int i = 0; i < 4; i++) if (w[i] > 0.0) c += w[i] * textureLod(looks, vec3(look, float(l * 4 + i)), 0.0).rgb;
  }
  // What the drive writes of its own accord, which nobody chooses.
  c = mix(c, textureLod(looks, vec3(look, plain), 0.0).rgb, 0.153);
  // A flat position's picture: the carve page's dithered bytes at this spot's share of light
  // bytes, as simulated through the same optics (design::carve_looks).
  vec4 picture = textureLod(weights, vec3(cell, float(LAYERS)), 0.0);
  if (picture.g > 0.5 && carveLevels > 1.0) {
    float at = picture.r * (carveLevels - 1.0);
    float i0 = min(floor(at), carveLevels - 2.0);
    c = mix(textureLod(looks, vec3(look, carveBase + i0), 0.0).rgb, textureLod(looks, vec3(look, carveBase + i0 + 1.0), 0.0).rgb, at - i0);
  }
  c += textureLod(groove, vec2(min(gr / gMax, 1.0), 0.5), 0.0).rgb * exp(-gt * gt / (2.0 * spread * spread));
  c = clamp(c, 0.0, 1.0);
  color = vec4(mix(c * 12.92, 1.055 * pow(c, vec3(1.0 / 2.4)) - 0.055, step(0.0031308, c)), 1.0);
}
`

function halves(src: Float32Array, texels: number): Uint16Array {
  const out = new Uint16Array(texels * 4)
  for (let i = 0; i < texels; i++) {
    for (let c = 0; c < 3; c++) out[i * 4 + c] = DataUtils.toHalfFloat(Math.min(src[i * 3 + c], 65000))
    out[i * 4 + 3] = 0x3c00
  }
  return out
}

function dropTexture(name: string) {
  ;(material?.uniforms[name]?.value as DataArrayTexture | null)?.dispose?.()
}

function loadWeights() {
  if (!material || !props.weights) return
  const [grid, options] = new Uint32Array(props.weights, 0, 2)
  const layers = Math.ceil(options / 4)
  // One more layer after the options: the flat positions' greys (`turntable::weight_layers`).
  const tex = new DataArrayTexture(new Uint8Array(props.weights, 8), grid, grid, layers + 1)
  tex.format = RGBAFormat
  tex.minFilter = tex.magFilter = NearestFilter
  tex.unpackAlignment = 1
  tex.needsUpdate = true
  dropTexture('weights')
  material.uniforms.weights.value = tex
  if (material.defines.LAYERS !== layers) {
    material.defines.LAYERS = layers
    material.needsUpdate = true
  }
  dirty = true
}

function loadTables() {
  if (!material || !props.tables) return
  // `turntable::response_tables`: the options, then the carve page's bytes at each level.
  const [options, gtBins, grBins, grooves, carveLevels] = new Uint32Array(props.tables, 0, 5)
  const [gMax, spread] = new Float32Array(props.tables, 20, 2)
  const layers = options + carveLevels
  const texels = layers * gtBins * grBins
  const looks = new DataArrayTexture(halves(new Float32Array(props.tables, 28, texels * 3), texels), grBins, gtBins, layers)
  const groove = new DataTexture(halves(new Float32Array(props.tables, 28 + texels * 12, grooves * 3), grooves), grooves, 1)
  for (const t of [looks, groove]) {
    t.format = RGBAFormat
    t.type = HalfFloatType
    t.minFilter = t.magFilter = LinearFilter
    t.needsUpdate = true
  }
  dropTexture('looks')
  dropTexture('groove')
  Object.assign(material.uniforms.looks, { value: looks })
  Object.assign(material.uniforms.groove, { value: groove })
  material.uniforms.gMax.value = gMax
  material.uniforms.spread.value = spread
  material.uniforms.plain.value = options - 1
  material.uniforms.carveBase.value = options
  material.uniforms.carveLevels.value = carveLevels
  material.uniforms.bins.value = [grBins, gtBins]
  dirty = true
}

function buildDisc() {
  if (!disc || !material) return
  for (const m of [...disc.children]) {
    ;(m as Mesh).geometry.dispose()
    m.removeFromParent()
  }
  disc.add(new Mesh(new RingGeometry(props.innerRadiusMm, props.outerRadiusMm, 192), material))
  material.uniforms.outer.value = props.outerRadiusMm
  dirty = true
}

const shortWay = (d: number) => ((((d + 180) % 360) + 360) % 360) - 180

function tick(now: number) {
  raf = requestAnimationFrame(tick)
  const dt = Math.min((now - last) / 1000, 0.05)
  const k = 1 - Math.exp(-dt * FOLLOW)
  last = now
  const isTransform = props.mode === 'transform'
  if (isTransform) {
    const tiltLeft = transformTiltDeg(props.tiltDeg, previewT.value) - tilt
    if (Math.abs(tiltLeft) > 0.004) {
      tilt += tiltLeft * k
      dirty = true
    }
  } else {
    // Turning happens flat, so that the axis stays put; the tilt comes back afterwards.
    const tiltLeft = (dragging.value ? 0 : wantTilt) - tilt
    if (Math.abs(tiltLeft) > 0.004) {
      tilt += tiltLeft * k
      dirty = true
    }
  }
  const turnLeft = shortWay(turn.value - shownTurn)
  if (Math.abs(turnLeft) > 0.01) {
    shownTurn += dragging.value ? turnLeft : turnLeft * k
    dirty = true
  }
  if (!dirty || !renderer || !scene || !camera || !disc || !material || !props.weights || !props.tables) return
  dirty = false
  disc.rotation.set(0, (tilt * Math.PI) / 180, (shownTurn * Math.PI) / 180, 'YXZ')
  disc.updateMatrixWorld()
  disc.worldToLocal((material.uniforms.eye.value as Vector3).set(0, 0, props.distanceMm))
  const lamp = props.lampMm
  disc.worldToLocal((material.uniforms.lamp.value as Vector3).set(...lamp))
  renderer.render(scene, camera as Camera)
}

function resize() {
  if (!host.value || !renderer || !camera) return
  const [w, h] = [Math.max(1, host.value.clientWidth), Math.max(1, host.value.clientHeight)]
  renderer.setPixelRatio(Math.min(devicePixelRatio, 2))
  renderer.setSize(w, h, false)
  // The whole disc, with room for the bars at the top and bottom.
  const need = (props.outerRadiusMm * 1.24) / Math.min(1, w / h)
  camera.aspect = w / h
  camera.position.z = props.distanceMm
  camera.fov = (2 * Math.atan(need / props.distanceMm) * 180) / Math.PI
  camera.updateProjectionMatrix()
  dirty = true
}

/** Where the pointer is about the middle of the disc: a share of the stage, and an angle. */
function about(e: PointerEvent) {
  const box = host.value!.getBoundingClientRect()
  const [x, y] = [e.clientX - box.left - box.width / 2, e.clientY - box.top - box.height / 2]
  const r = Math.min(box.width, box.height) / 2 || 1
  return { x: x / r, r: Math.hypot(x, y) / r, deg: (Math.atan2(y, x) * 180) / Math.PI }
}

let lastDeg = 0
function down(e: PointerEvent) {
  if (e.button !== 0) return
  dragging.value = true
  lastDeg = about(e).deg
  host.value?.setPointerCapture(e.pointerId)
}
/** Sway with the pointer: in transform mode across the animation's tilt, t from 0 to 1. */
function sway(x: number) {
  const at = Math.max(-1, Math.min(1, x))
  if (props.mode === 'transform') {
    previewT.value = Math.round(((at + 1) / 2) * 1000) / 1000
    dirty = true
  } else {
    wantTilt = at * MAX_TILT
  }
}
function move(e: PointerEvent) {
  const at = about(e)
  if (!dragging.value) {
    sway(at.x)
    return
  }
  // The disc follows the hand round its centre, where a direction means nothing close in.
  const by = shortWay(at.deg - lastDeg)
  lastDeg = at.deg
  if (at.r > 0.08) turn.value = (turn.value - by + 360) % 360
}
function up(e: PointerEvent) {
  dragging.value = false
  sway(about(e).x)
}
function leave() {
  if (props.mode !== 'transform' && !dragging.value) wantTilt = 0
}
function wheel(e: WheelEvent) {
  turn.value = (turn.value + (Math.abs(e.deltaX) > Math.abs(e.deltaY) ? e.deltaX : e.deltaY) * 0.15 + 360) % 360
}

onMounted(() => {
  try {
    renderer = new WebGLRenderer({ canvas: canvas.value!, antialias: true, alpha: true, preserveDrawingBuffer: false })
  } catch (e) {
    error.value = String(e)
    return
  }
  scene = new Scene()
  camera = new PerspectiveCamera(20, 1, 1, 4000)
  disc = new Group()
  scene.add(disc)
  material = new RawShaderMaterial({
    glslVersion: GLSL3,
    vertexShader: VERTEX,
    fragmentShader: FRAGMENT,
    defines: { LAYERS: 7 },
    uniforms: {
      weights: { value: null }, looks: { value: null }, groove: { value: null },
      eye: { value: new Vector3() }, lamp: { value: new Vector3() },
      outer: { value: 1 }, gMax: { value: 2 }, spread: { value: 0.01 }, plain: { value: 0 }, carveBase: { value: 0 }, carveLevels: { value: 0 }, bins: { value: [1, 1] },
    },
  })
  buildDisc()
  loadTables()
  loadWeights()
  if (props.mode === 'transform') tilt = transformTiltDeg(props.tiltDeg, previewT.value)
  observer = new ResizeObserver(resize)
  observer.observe(host.value!)
  resize()
  start()
})

function start() {
  cancelAnimationFrame(raf)
  if (props.active && renderer) {
    last = performance.now()
    dirty = true
    raf = requestAnimationFrame(tick)
  }
}
watch(() => props.active, start)
watch(() => props.mode, (m) => {
  if (m === 'transform') tilt = transformTiltDeg(props.tiltDeg, previewT.value)
  else wantTilt = 0
  dirty = true
})
watch(previewT, () => {
  if (props.mode === 'transform') dirty = true
})
watch(() => props.weights, loadWeights)
watch(() => props.tables, loadTables)
watch(() => [props.innerRadiusMm, props.outerRadiusMm], buildDisc)
watch(() => [props.lampMm, props.distanceMm], resize, { deep: true })
onBeforeUnmount(() => {
  cancelAnimationFrame(raf)
  observer?.disconnect()
  for (const n of ['weights', 'looks', 'groove']) dropTexture(n)
  disc?.children.forEach((m) => (m as Mesh).geometry.dispose())
  material?.dispose()
  renderer?.dispose()
  // dispose() keeps the WebGL context; WebKit allows only a few, and every hot reload or remount
  // would leak one until the page stalls.
  renderer?.forceContextLoss()
})
</script>

<style scoped>
.preview { position: absolute; inset: 0; cursor: grab; touch-action: none; }
.preview.dragging { cursor: grabbing; }
canvas { display: block; width: 100%; height: 100%; }
.fallback { position: absolute; inset: auto var(--oc-space-md) var(--oc-space-md); }
</style>
