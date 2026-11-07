<template>
  <canvas ref="canvas" class="disc-stage" aria-hidden="true" />
  <!-- The magnifier that aims at the stamped code while the code field is in use. -->
  <div ref="lens" class="lens" aria-hidden="true"><canvas ref="lensCanvas" /></div>
</template>

<script setup lang="ts">
// The one disc in the app. It sits under every page; each page only says where it wants
// the disc to be, and the disc travels there.
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { lookOf } from '../discLook'
import { createDiscRenderer, type DiscState } from '../discRenderer'
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
/** Room left round the disc on the carve page, as a share of the free height. */
const CARVE_FIT = 0.94
/** A blank disc on the carve page keeps a hint of light; a loaded image gets none. */
const CARVE_FX = 0.25

const s = useStudio()
const process = useProcess()
const canvas = ref<HTMLCanvasElement | null>(null)
const look = computed(() => lookOf(s.discPreview ?? s.preset))

const state: DiscState = {
  x: 0, y: 0, scale: 1, dim: 1, fx: 1, tiltX: -0.42, tiltY: -0.52, pointerX: 0.3, pointerY: 0.5, spin: 0, time: 0,
  dye: [...look.value.dye], pitch: look.value.pitch, seed: look.value.seed, etchMix: 0, artMix: 0, artRange: look.value.artRange, codeLight: 0,
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

let artShown = ''

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
    fx: s.discArt ? 0 : CARVE_FX,
    lean: s.discArt ? 0 : 0.05,
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
  const k = 1 - Math.exp(-dt * POSE_RATE)
  const kLook = 1 - Math.exp(-dt * LOOK_RATE)
  const kArt = 1 - Math.exp(-dt * ART_RATE)
  const kSpin = 1 - Math.exp(-dt * SPIN_RATE)

  const dx = (pose.x - state.x) * k
  const dy = (pose.y - state.y) * k
  state.x += dx
  state.y += dy
  state.scale += (pose.scale - state.scale) * k
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
  state.artRange = l.artRange
  state.artMix += ((s.discArt && s.view === 'carve' ? 1 : 0) - state.artMix) * kArt
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

watch(() => s.discArt, (url) => {
  if (!url) return
  artShown = url
  const image = new Image()
  image.onload = () => {
    // A newer preview may have finished decoding first.
    if (artShown !== url) return
    renderer?.setArt(image)
    snap()
  }
  image.src = url
})

watch(() => [s.view, s.codeFocus, s.discPreview, s.discZoom, s.discPan.x, s.discPan.y, look.value], snap)

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
