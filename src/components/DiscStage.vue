<template>
  <canvas ref="canvas" class="disc-stage" aria-hidden="true" />
  <!-- The magnifier that aims at the stamped code while the code field is in use. -->
  <div ref="lens" class="lens" aria-hidden="true"><canvas ref="lensCanvas" /></div>
</template>

<script setup lang="ts">
// The one disc in the app. It sits under every page; each page only says where it wants
// the disc to be, and the disc travels there.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { CARVE_FIT, lookOf } from '../discLook'
import { createDiscRenderer, MAX_LAYERS, NO_ART, type ArtPlacement, type DiscState } from '../discRenderer'
import { useComposition } from '../stores/composition'
import { useProcess } from '../stores/process'
import { useStudio } from '../stores/studio'

interface Pose {
  x: number
  y: number
  scale: number
  tiltX: number
  tiltY: number
  dim: number
  fx: number
  /** How far the pointer may lean the disc, in radians. */
  lean: number
  spin: number
}

const POSE_RATE = 3.6
const LOOK_RATE = 2.6
const ART_RATE = 5
const SPIN_RATE = 2
/** The lens answers a click at once; the disc's own glides stay slow. */
const LENS_RATE = 14
const IDLE_SPIN = 0.09
const BURN_SPIN = 1.6
/** Travelling across the window rolls the disc, like something with mass. */
const ROLL_PER_UNIT = 0.9
/** On the carve page the disc follows pan and zoom at once after arriving, so the
 *  handles drawn over it stay on the picture. */
const CARVE_SETTLE_MS = 700
/** A blank disc on the carve page keeps a hint of light; a loaded image gets none. */
const CARVE_FX = 0.25

const s = useStudio()
const process = useProcess()
const comp = useComposition()
const canvas = ref<HTMLCanvasElement | null>(null)
const look = computed(() => lookOf(s.discPreview ?? s.preset))

const state: DiscState = {
  x: 0, y: 0, scale: 1, dim: 1, fx: 1, tiltX: -0.42, tiltY: -0.52, pointerX: 0.3, pointerY: 0.5, spin: 0, time: 0,
  dye: [...look.value.dye], pitch: look.value.pitch, seed: look.value.seed, etchMix: 0, artMix: 0, art: NO_ART, codeLight: 0,
}
let spinSpeed = IDLE_SPIN
let leanX = 0
let leanY = 0
let target: { x: number; y: number } | null = null
let renderer: ReturnType<typeof createDiscRenderer> = null
let frame = 0
let observer: ResizeObserver | null = null
let last = 0
let reduced = false
let fading = false
let lensMix = 0
const lens = ref<HTMLElement | null>(null)
const lensCanvas = ref<HTMLCanvasElement | null>(null)
let lensRenderer: ReturnType<typeof createDiscRenderer> = null
/** Lens diameter on screen, and how much of the ring's width it frames. */
const LENS_PX = 200
/** Where the stamped text runs, as a share of the disc's drawn radius, and how much the tilt squashes it. */
const RING_TEXT = 0.33
const RING_SQUASH = 0.82
/** Where on the ring the lens aims (radians, from the right, screen y down): towards the form. */
const LENS_AIM = 3.75
/** Half the width of disc the lens frames, as a share of the drawn radius: about 2.5x. */
const LENS_FRAME = 0.12
const lensWanted = () => s.view === 'models' && (s.codeFocus || !!s.discPreview)

/** Re-renders the stamped ring up close in the lens, so the text stays sharp. */
function updateLens() {
  const el = lens.value
  const src = canvas.value
  if (!el || !src) return
  el.style.opacity = String(lensMix)
  el.style.visibility = lensMix > 0.01 ? 'visible' : 'hidden'
  if (lensMix <= 0.01 || !lensRenderer) return
  const box = src.getBoundingClientRect()
  const unit = box.height / 2
  const radius = state.scale * unit
  const cx = box.width / 2 + state.x * unit + Math.cos(LENS_AIM) * RING_TEXT * radius
  const cy = box.height / 2 - state.y * unit + Math.sin(LENS_AIM) * RING_TEXT * radius * RING_SQUASH
  el.style.transform = `translate(${cx - LENS_PX / 2}px, ${cy - LENS_PX / 2}px) scale(${0.85 + 0.15 * lensMix})`
  lensRenderer.draw({ ...state, etchMix: 0, window: [(cx - box.width / 2) / unit, -(cy - box.height / 2) / unit, (LENS_FRAME * radius) / unit] })
}

/** The layers the disc has pictures for, bottom first: the order setArt was given. */
const shownLayers = computed(() => comp.layers.filter((l) => comp.fields.has(l.path)).slice(0, MAX_LAYERS))

/** The carve settings as the carver reads them, so the disc can follow every change at once. */
function placement(): ArtPlacement {
  const p = s.preset
  if (!p) return NO_ART
  // A burn writes from the hub outwards at an even density, so written area tracks progress.
  const burning = revealing()
  const [ri, ro] = [p.innerRadiusMm, p.outerRadiusMm]
  const revealMm = burning ? (process.written > 0 ? Math.sqrt(ri * ri + process.written * (ro * ro - ri * ri)) : 0) : 100
  // Tone::from_duty in carver.rs.
  const duty = Math.min(0.85, Math.max(0.5, s.config.maxDarkDutyCycle))
  return {
    innerMm: p.innerRadiusMm,
    outerMm: p.outerRadiusMm,
    floor: s.config.servoSafeDither ? Math.round(255 * (1 - duty)) / 255 : 0,
    layers: shownLayers.value.map((l) => {
      const long = Math.max(l.width, l.height, 1)
      return { x: l.x, y: l.y, width: (l.size * l.width) / long, height: (l.size * l.height) / long, rotationRad: (l.rotationDeg * Math.PI) / 180, contrast: l.contrast, invert: l.invert }
    }),
    turn: burning ? state.spin - spinAtBurn : 0,
    revealMm,
  }
}

/** The burn page is writing this composition, so it shows on the spinning disc. */
const revealing = () => s.view === 'process' && process.request?.revealStep != null
let spinAtBurn = 0
watch(revealing, (on) => on && (spinAtBurn = state.spin))

let carveSince = 0
watch(() => s.view, (v) => v === 'carve' && (carveSince = performance.now()))
const hasArt = computed(() => shownLayers.value.length > 0)

function carvePose(fallback: Pose): Pose {
  const box = canvas.value!.getBoundingClientRect()
  const area = document.querySelector('[data-disc-area]')?.getBoundingClientRect()
  if (!area?.width) return fallback
  const unit = box.height / 2
  return {
    x: (area.left + area.width / 2 - box.left - box.width / 2) / unit + s.discPan.x / unit,
    y: (box.top + box.height / 2 - area.top - area.height / 2) / unit - s.discPan.y / unit,
    scale: (Math.min(area.width, area.height) / 2 / unit) * CARVE_FIT * s.discZoom,
    tiltX: 0,
    tiltY: 0,
    dim: 1,
    fx: hasArt.value ? 0 : CARVE_FX,
    lean: hasArt.value ? 0 : 0.05,
    spin: 0,
  }
}

function poseFor(aspect: number): Pose {
  const hero: Pose = { x: aspect * 0.62, y: 0.02, scale: 1, tiltX: -0.42, tiltY: -0.52, dim: 1, fx: 1, lean: 0.2, spin: IDLE_SPIN }
  switch (s.view) {
    case 'models':
    case 'calibrate':
      // Typing a code turns the disc to face the reader, moved to the middle and enlarged
      // so the stamped ring is what the field is about.
      // The disc stays put; a magnifier does the pointing (see updateLens).
      return { x: aspect * 0.72, y: -0.05, scale: 1.05, tiltX: -0.6, tiltY: -0.3, dim: 0.62, fx: 1, lean: 0.14, spin: IDLE_SPIN }
    case 'carve':
      return carvePose(hero)
    case 'process':
      return { ...hero, spin: process.status === 'running' ? BURN_SPIN : IDLE_SPIN }
    default:
      return hero
  }
}

function aim(e: PointerEvent) {
  const box = canvas.value!.getBoundingClientRect()
  target = { x: ((e.clientX - box.left) / box.width) * 2 - 1, y: 1 - ((e.clientY - box.top) / box.height) * 2 }
}
const release = () => (target = null)

function advance(now: number, dt: number) {
  const box = canvas.value!.getBoundingClientRect()
  const pose = poseFor(box.width / Math.max(box.height, 1))
  // Frame-rate independent, so the glide feels the same on any display.
  const settled = s.view === 'carve' && now - carveSince > CARVE_SETTLE_MS
  const k = 1 - Math.exp(-dt * POSE_RATE)
  const kPlace = settled ? 1 : k
  const kLook = 1 - Math.exp(-dt * LOOK_RATE)
  const kArt = 1 - Math.exp(-dt * ART_RATE)
  const kSpin = 1 - Math.exp(-dt * SPIN_RATE)

  const dx = (pose.x - state.x) * kPlace
  const dy = (pose.y - state.y) * kPlace
  state.x += dx
  state.y += dy
  state.scale += (pose.scale - state.scale) * kPlace
  state.dim += (pose.dim - state.dim) * k
  state.fx += (pose.fx - state.fx) * k

  const t = target ?? { x: 0.3 + 0.35 * Math.sin(now / 5200), y: 0.5 * Math.cos(now / 4100) }
  leanX += (-t.y * pose.lean - leanX) * k
  leanY += (t.x * pose.lean - leanY) * k
  state.tiltX += (pose.tiltX + leanX - state.tiltX) * k
  state.tiltY += (pose.tiltY + leanY - state.tiltY) * k
  state.pointerX += (t.x - state.pointerX) * k
  state.pointerY += (t.y - state.pointerY) * k

  spinSpeed += (pose.spin - spinSpeed) * kSpin
  state.spin += spinSpeed * dt + Math.hypot(dx, dy) * ROLL_PER_UNIT
  state.time = now / 1000

  const l = look.value
  state.dye = state.dye.map((v, i) => v + (l.dye[i] - v) * kLook) as DiscState['dye']
  state.pitch += (l.pitch - state.pitch) * kLook
  state.seed += (l.seed - state.seed) * kLook
  state.art = placement()
  state.artMix += ((hasArt.value && (s.view === 'carve' || revealing()) ? 1 : 0) - state.artMix) * kArt
  lensMix += ((lensWanted() ? 1 : 0) - lensMix) * (1 - Math.exp(-dt * LENS_RATE))
  if (fading) {
    state.etchMix += (1 - state.etchMix) * kLook
    if (state.etchMix > 0.995) {
      renderer?.swapEtch()
      state.etchMix = 0
      fading = false
    }
  }
}

function step(now: number) {
  const dt = Math.min((now - last) / 1000, 0.05)
  last = now
  advance(now, dt)
  renderer?.draw(state)
  updateLens()
  frame = requestAnimationFrame(step)
}

/** With reduced motion the disc jumps to its place instead of gliding. */
function snap() {
  if (!reduced) return
  advance(performance.now(), 10)
  renderer?.draw(state)
  updateLens()
}

watch(() => look.value.code, (code) => {
  if (fading) {
    renderer?.swapEtch()
    state.etchMix = 0
  }
  renderer?.setEtch(code, false)
  lensRenderer?.setEtch(code, true)
  fading = true
  snap()
})

watch(() => shownLayers.value.map((l) => l.path).join('\n') + comp.fields.size, () => {
  renderer?.setArt(shownLayers.value.map((l) => comp.fields.get(l.path)!))
  snap()
})

watch(() => [s.view, s.codeFocus, s.discPreview, s.discZoom, s.discPan.x, s.discPan.y, look.value, placement()], snap, { deep: true })

onMounted(() => {
  renderer = createDiscRenderer(canvas.value!)
  s.discRendered = !!renderer
  if (!renderer) return
  reduced = matchMedia('(prefers-reduced-motion: reduce)').matches
  renderer.setEtch(look.value.code, true)
  lensRenderer = createDiscRenderer(lensCanvas.value!)
  lensRenderer?.resize()
  lensRenderer?.setEtch(look.value.code, true)
  const host = canvas.value!.parentElement!
  host.addEventListener('pointermove', aim)
  host.addEventListener('pointerleave', release)
  observer = new ResizeObserver(() => {
    renderer?.resize()
    lensRenderer?.resize()
    renderer?.draw(state)
  })
  observer.observe(canvas.value!)
  renderer.resize()
  const box = canvas.value!.getBoundingClientRect()
  Object.assign(state, poseFor(box.width / Math.max(box.height, 1)))
  renderer.draw(state)
  last = performance.now()
  if (reduced) return
  frame = requestAnimationFrame(step)
})
onBeforeUnmount(() => {
  cancelAnimationFrame(frame)
  observer?.disconnect()
  const host = canvas.value?.parentElement
  host?.removeEventListener('pointermove', aim)
  host?.removeEventListener('pointerleave', release)
  renderer?.dispose()
  lensRenderer?.dispose()
})
</script>

<style scoped>
.disc-stage { position: absolute; inset: 0; width: 100%; height: 100%; }
.lens { position: absolute; left: 0; top: 0; width: 200px; height: 200px; opacity: 0; visibility: hidden; pointer-events: none; will-change: transform; }
.lens canvas { position: relative; width: 100%; height: 100%; display: block; border-radius: 50%; border: 3px solid rgba(235, 235, 245, 0.85); box-shadow: 0 10px 34px rgba(0, 0, 0, 0.55), inset 0 0 22px rgba(0, 0, 0, 0.45); background: #0b0b10; box-sizing: border-box; }
/* A glow that runs round the rim: a blurred band of the accent colours, turning slowly. */
.lens::before { content: ''; position: absolute; inset: -9px; border-radius: 50%; background: conic-gradient(rgba(144, 69, 249, 0), #9045f9 15%, #8fb7ff 35%, #8ff0dc 50%, #9045f9 65%, rgba(144, 69, 249, 0) 85%); filter: blur(7px); opacity: 1; animation: lens-flow 3.2s linear infinite; }
@keyframes lens-flow { to { transform: rotate(360deg); } }
@media (prefers-reduced-motion: reduce) { .lens::before { animation: none; } }
/* The handle, running down and to the right from the rim. */
.lens::after { content: ''; position: absolute; left: 50%; top: 50%; width: 76px; height: 11px; margin: -5px 0 0 -2px; border-radius: 6px; background: linear-gradient(#d7d7e0, #7d7d89); transform-origin: 0 50%; transform: rotate(45deg) translateX(100px); box-shadow: 0 6px 14px rgba(0, 0, 0, 0.5); }
</style>
