<template>
  <div class="page carve">
    <aside class="side">
      <div class="flow fields">
        <h1>{{ t('carve.title') }}</h1>
        <PresetList />
        <section class="section">
          <div class="title row">
            <span class="grow ellipsis">{{ title }}</span>
            <span v-if="comp.dirty" class="small muted">{{ t('carve.unsaved') }}</span>
          </div>
          <button v-if="!comp.layers.length" class="drop" type="button" @click="choose">
            <span class="muted"><UiIcon :path="mdiImagePlusOutline" :size="20" />{{ t('carve.choose') }}</span>
          </button>
          <div v-else class="layers" role="listbox" :aria-label="t('carve.image')">
            <div v-for="(l, i) in stack" :key="l.id" class="layer" role="option" :aria-selected="l.id === comp.selectedId" :class="{ on: l.id === comp.selectedId }" @click="comp.selectedId = l.id">
              <img v-if="comp.pictures[l.path]" :src="comp.pictures[l.path].thumbnail" alt="" />
              <span v-else class="thumb" />
              <span class="grow ellipsis" :data-tooltip="l.name" data-tooltip-overflow>{{ l.name }}</span>
              <span class="tools">
                <UiButton icon-only variant="ghost" :icon="mdiArrowUp" :aria-label="t('carve.raise')" :disabled="i === 0" @click.stop="comp.restack(l.id, 1)" />
                <UiButton icon-only variant="ghost" :icon="mdiArrowDown" :aria-label="t('carve.lower')" :disabled="i === stack.length - 1" @click.stop="comp.restack(l.id, -1)" />
                <UiButton icon-only variant="ghost" :icon="mdiClose" :aria-label="t('carve.remove')" @click.stop="comp.remove(l.id)" />
              </span>
            </div>
          </div>
          <UiButton v-if="comp.layers.length" variant="ghost" :icon="mdiPlus" :disabled="comp.full" :data-tooltip="comp.full ? t('carve.full') : undefined" @click="choose">{{ t('carve.add') }}</UiButton>
          <div v-if="sel && comp.pictures[sel.path]?.suggestInvert && !sel.invert" class="notice warn">
            <UiIcon :path="mdiAlertOutline" />
            <div class="grow">
              {{ t('carve.darkWarn') }}
              <a role="button" tabindex="0" @click="sel.invert = true" @keydown.enter="sel.invert = true">{{ t('carve.invertWhite') }}</a>
            </div>
          </div>
        </section>

        <UiAdvanced>
          <section v-if="sel" class="section">
            <div class="title row">
              <span class="grow">{{ t('carve.position') }}</span>
              <a class="action-link" role="button" tabindex="0" @click="resetLayer" @keydown.enter="resetLayer">{{ t('carve.reset') }}</a>
            </div>
            <UiSlider v-model="percent.x" :label="t('carve.x')" :min="-50" :max="50" :step="0.5" :format="formatOffset" />
            <UiSlider v-model="percent.y" :label="t('carve.y')" :min="-50" :max="50" :step="0.5" :format="formatOffset" />
            <UiSlider v-model="percent.size" :label="t('carve.size')" :min="5" :max="300" :step="1" :format="(v) => `${Math.round(v)}%`" />
            <UiSlider v-model="sel.rotationDeg" :label="t('carve.rotate')" :min="-180" :max="180" :step="0.5" :format="(v) => `${v.toFixed(1)}°`" />
          </section>

          <section v-if="sel" class="section">
            <div class="title">{{ t('carve.tone') }}</div>
            <UiSwitch v-model="sel.invert">{{ t('carve.invert') }}</UiSwitch>
            <UiSlider v-model="sel.contrast" :label="t('carve.contrast')" :min="-100" :max="100" :step="1" />
          </section>

          <UiSwitch v-model="c.servoSafeDither">{{ t('carve.servoSafe') }}</UiSwitch>
          <UiSlider v-model="c.maxDarkDutyCycle" :label="t('carve.maxDuty')" :min="0.5" :max="0.85" :step="0.01" :disabled="!c.servoSafeDither" :format="(v) => `${Math.round(v * 100)}%`" />
          <div v-if="c.servoSafeDither && c.maxDarkDutyCycle > MAX_SECTOR_DARK" class="notice danger">
            <UiIcon :path="mdiAlertOctagonOutline" />
            <div>{{ t('carve.dutyWarn') }}</div>
          </div>
          <div v-if="!c.servoSafeDither" class="notice danger">
            <UiIcon :path="mdiAlertOctagonOutline" />
            <div>{{ t('carve.servoWarn') }}</div>
          </div>
        </UiAdvanced>
      </div>

      <div class="actions">
        <UiButton variant="solid" size="lg" :icon="mdiFire" :disabled="!s.carveReady" @click="burn">{{ t('carve.burn') }}</UiButton>
        <UiButton variant="ghost" size="lg" :disabled="!s.carveReady" @click="exportIso">{{ t('carve.export') }}</UiButton>
      </div>
    </aside>

    <div ref="viewport" class="viewport" :class="{ grabbing: !!drag }" :style="{ cursor }" data-disc-area @wheel.prevent="wheel" @pointerdown="pointerDown" @pointermove="pointerMove" @pointerup="pointerUp" @pointercancel="pointerUp" @dblclick="resetView">
      <img v-if="previewUrl && !s.discRendered" class="disc" :src="previewUrl" :alt="t('carve.previewAlt')" draggable="false" :style="{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})` }" />
      <svg v-if="box && s.discRendered" class="overlay" :width="box.w" :height="box.h" aria-hidden="true">
        <template v-if="outline">
          <polygon class="frame" :points="outline.points" />
          <line class="frame" :x1="outline.top[0]" :y1="outline.top[1]" :x2="outline.knob[0]" :y2="outline.knob[1]" />
          <rect v-for="(p, i) in outline.corners" :key="i" class="handle" data-handle="scale" :x="p[0] - 5" :y="p[1] - 5" width="10" height="10" rx="2" :style="{ cursor: i % 2 ? 'nesw-resize' : 'nwse-resize' }" />
          <circle class="handle" data-handle="rotate" :cx="outline.knob[0]" :cy="outline.knob[1]" r="6" />
        </template>
      </svg>
      <div v-if="!comp.layers.length" class="hint">{{ t('carve.hint') }}</div>
      <div v-if="comp.loading" class="spin small muted">{{ t('carve.updating') }}</div>
      <div class="float mono muted">{{ Math.round(zoom * 100) }}%</div>
      <div v-if="sel" class="float left mono muted">{{ t('carve.center', { x: formatOffset(sel.x * 100), y: formatOffset(sel.y * 100) }) }} · {{ Math.round(sel.size * 100) }}% · {{ sel.rotationDeg.toFixed(1) }}°</div>
    </div>

    <UiDialog v-if="pending" :title="t('carve.discardTitle')" @close="pending = null">
      <p>{{ t('carve.discardBody') }}</p>
      <template #actions>
        <UiButton variant="ghost" @click="pending = null">{{ t('common.cancel') }}</UiButton>
        <UiButton variant="danger" @click="discard">{{ t('carve.discard') }}</UiButton>
      </template>
    </UiDialog>
  </div>
</template>

<script setup lang="ts">
import { mdiAlertOctagonOutline, mdiAlertOutline, mdiArrowDown, mdiArrowUp, mdiClose, mdiFire, mdiImagePlusOutline, mdiPlus } from '@mdi/js'
import { computed, onBeforeUnmount, onMounted, reactive, ref, toRef, watch } from 'vue'
import { call, pickComposition, pickCompositionSave, pickImage, pickSavePath } from '../api'
import PresetList from '../components/PresetList.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiButton from '../components/UiButton.vue'
import UiDialog from '../components/UiDialog.vue'
import UiIcon from '../components/UiIcon.vue'
import UiSlider from '../components/UiSlider.vue'
import UiSwitch from '../components/UiSwitch.vue'
import { CARVE_FIT } from '../discLook'
import { COMPOSE_SECONDS } from '../etaModel'
import { t } from '../i18n'
import { issueOf } from '../catalog'
import { notifyError, notifySuccess } from '../notices'
import { coverSize, fileName, useComposition } from '../stores/composition'
import { useProcess, type StepContext } from '../stores/process'
import { useStudio } from '../stores/studio'
import type { BuiltIso, DiscGeometryPreset, Layer } from '../types'

const s = useStudio()
const p = useProcess()
const comp = useComposition()
const c = s.config
/** The servo safety line the build measures against; going past it is allowed but warned. */
const MAX_SECTOR_DARK = 0.7
const stem = (f: string) => fileName(f).replace(/\.[^.]+$/, '')
const formatOffset = (v: number) => (v > 0 ? `+${v.toFixed(1)}%` : `${v.toFixed(1)}%`)

/** Top first, as layer lists read. */
const stack = computed(() => [...comp.layers].reverse())
const sel = computed(() => comp.selected)
const title = computed(() => (comp.path ? stem(comp.path) : t('carve.image')))
/** The selected picture's place in percent of the diameter, for the precise sliders. */
const percent = reactive({
  x: computed({ get: () => (sel.value?.x ?? 0) * 100, set: (v) => sel.value && (sel.value.x = v / 100) }),
  y: computed({ get: () => (sel.value?.y ?? 0) * 100, set: (v) => sel.value && (sel.value.y = v / 100) }),
  size: computed({ get: () => (sel.value?.size ?? 1) * 100, set: (v) => sel.value && (sel.value.size = v / 100) }),
})
function resetLayer() {
  const l = sel.value
  if (!l) return
  Object.assign(l, { x: 0, y: 0, rotationDeg: 0, size: comp.layers[0] === l ? coverSize(l) : 0.5 })
}

async function choose() {
  const f = await pickImage()
  if (f) await comp.add(f)
}

// ---- New, open, save ----
const pending = ref<null | (() => void)>(null)
/** Runs `then` at once, or after the user agrees to drop unsaved changes. */
function guard(then: () => void) {
  if (comp.dirty) pending.value = then
  else then()
}
function discard() {
  const then = pending.value
  pending.value = null
  then?.()
}
function newComposition() {
  guard(() => comp.clear())
}
function openComposition() {
  guard(async () => {
    const f = await pickComposition()
    if (!f) return
    try {
      await comp.open(f)
    } catch (e) {
      notifyError(String(e))
    }
  })
}
async function saveComposition(as = false) {
  if (!comp.layers.length) return
  const f = !as && comp.path ? comp.path : await pickCompositionSave(`${comp.path ? stem(comp.path) : stem(comp.layers[comp.layers.length - 1].name)}.discratch`)
  if (!f) return
  try {
    await comp.save(f)
    notifySuccess(t('carve.saved', { name: fileName(f) }))
  } catch (e) {
    notifyError(String(e))
  }
}

// ---- Fallback without WebGL: a picture the native side renders ----
const previewUrl = ref('')
let ticket = 0
let timer: ReturnType<typeof setTimeout> | undefined
watch([() => JSON.stringify(comp.layers), () => ({ ...c }), () => s.preset], () => {
  if (s.discRendered) return
  clearTimeout(timer)
  timer = setTimeout(refreshFallback, 120)
})
async function refreshFallback() {
  if (!comp.layers.length || !s.preset) return
  const mine = ++ticket
  try {
    const url = await call<string>('simulate_disc_preview', { config: { ...c, layers: comp.layers, outputIsoPath: '', preset: s.preset }, previewSize: 1024 })
    if (mine === ticket) previewUrl.value = url
  } catch (e) {
    notifyError(String(e))
  }
}

// ---- The disc on screen, and the handles drawn over it ----
const zoom = toRef(s, 'discZoom')
const pan = s.discPan
const viewport = ref<HTMLElement | null>(null)
const box = ref<{ w: number; h: number } | null>(null)
let observer: ResizeObserver | null = null

/** Screen pixels per unit of disc position (one diameter), and where the centre is. */
const frame = computed(() => {
  if (!box.value) return null
  const radiusPx = (Math.min(box.value.w, box.value.h) / 2) * CARVE_FIT * zoom.value
  // The renderer's disc spans 60 mm to its edge; positions are shares of the diameter.
  const k = (radiusPx / 60) * 2 * (s.preset?.outerRadiusMm ?? 58)
  return { cx: box.value.w / 2 + pan.x, cy: box.value.h / 2 + pan.y, k }
})
const toScreen = (x: number, y: number): [number, number] => [frame.value!.cx + x * frame.value!.k, frame.value!.cy + y * frame.value!.k]
const toDisc = (e: PointerEvent) => {
  const r = viewport.value!.getBoundingClientRect()
  return { x: (e.clientX - r.left - frame.value!.cx) / frame.value!.k, y: (e.clientY - r.top - frame.value!.cy) / frame.value!.k }
}
function halfExtent(l: Layer) {
  const long = Math.max(l.width, l.height, 1)
  return [(l.size * l.width) / long / 2, (l.size * l.height) / long / 2]
}
/** A point of the picture, given relative to its centre, on the disc. Turns clockwise as seen. */
function corner(l: Layer, u: number, v: number) {
  const a = (l.rotationDeg * Math.PI) / 180
  return { x: l.x + u * Math.cos(a) - v * Math.sin(a), y: l.y + u * Math.sin(a) + v * Math.cos(a) }
}
function contains(l: Layer, x: number, y: number) {
  const a = (l.rotationDeg * Math.PI) / 180
  const [dx, dy] = [x - l.x, y - l.y]
  const [hw, hh] = halfExtent(l)
  return Math.abs(Math.cos(a) * dx + Math.sin(a) * dy) <= hw && Math.abs(-Math.sin(a) * dx + Math.cos(a) * dy) <= hh
}
const KNOB_PX = 26
const outline = computed(() => {
  const l = sel.value
  if (!l || !frame.value) return null
  const [hw, hh] = halfExtent(l)
  const corners = [[-hw, -hh], [hw, -hh], [hw, hh], [-hw, hh]].map(([u, v]) => { const q = corner(l, u, v); return toScreen(q.x, q.y) })
  const t0 = corner(l, 0, -hh)
  const top = toScreen(t0.x, t0.y)
  const a = (l.rotationDeg * Math.PI) / 180
  const knob: [number, number] = [top[0] + Math.sin(a) * KNOB_PX, top[1] - Math.cos(a) * KNOB_PX]
  return { corners, top, knob, points: corners.map((p) => p.join(',')).join(' ') }
})

type Drag =
  | { kind: 'pan'; x: number; y: number; moved: boolean }
  | { kind: 'move'; id: string; from: { x: number; y: number }; at: { x: number; y: number } }
  | { kind: 'scale'; id: string; from: number; size: number }
  | { kind: 'rotate'; id: string; from: number; deg: number }
const drag = ref<Drag | null>(null)
const hovering = ref(false)
const cursor = computed(() => (drag.value?.kind === 'move' ? 'grabbing' : drag.value?.kind === 'pan' ? 'grabbing' : hovering.value ? 'move' : 'default'))
const angleTo = (l: Layer, q: { x: number; y: number }) => (Math.atan2(q.y - l.y, q.x - l.x) * 180) / Math.PI

function pointerDown(e: PointerEvent) {
  if (e.button !== 0 || (e.target as HTMLElement).closest('.float')) return
  viewport.value?.setPointerCapture(e.pointerId)
  const handle = (e.target as Element).closest('[data-handle]')?.getAttribute('data-handle')
  const q = frame.value ? toDisc(e) : null
  const l = sel.value
  if (handle && l && q) {
    drag.value = handle === 'scale'
      ? { kind: 'scale', id: l.id, from: Math.hypot(q.x - l.x, q.y - l.y), size: l.size }
      : { kind: 'rotate', id: l.id, from: angleTo(l, q), deg: l.rotationDeg }
    return
  }
  const hit = q && s.discRendered ? stack.value.find((x) => contains(x, q.x, q.y)) : undefined
  if (hit && q) {
    comp.selectedId = hit.id
    drag.value = { kind: 'move', id: hit.id, from: q, at: { x: hit.x, y: hit.y } }
    return
  }
  drag.value = { kind: 'pan', x: e.clientX - pan.x, y: e.clientY - pan.y, moved: false }
}
function pointerMove(e: PointerEvent) {
  const d = drag.value
  const q = frame.value ? toDisc(e) : null
  if (!d) {
    hovering.value = !!q && s.discRendered && comp.layers.some((l) => contains(l, q.x, q.y))
    return
  }
  if (d.kind === 'pan') {
    Object.assign(pan, { x: e.clientX - d.x, y: e.clientY - d.y })
    d.moved = true
    return
  }
  const l = comp.layers.find((x) => x.id === d.id)
  if (!l || !q) return
  if (d.kind === 'move') {
    l.x = d.at.x + q.x - d.from.x
    l.y = d.at.y + q.y - d.from.y
  } else if (d.kind === 'scale') {
    l.size = Math.min(4, Math.max(0.02, (d.size * Math.hypot(q.x - l.x, q.y - l.y)) / Math.max(d.from, 1e-6)))
  } else {
    let deg = d.deg + angleTo(l, q) - d.from
    if (e.shiftKey) deg = Math.round(deg / 15) * 15
    l.rotationDeg = ((((deg + 180) % 360) + 360) % 360) - 180
  }
}
function pointerUp() {
  // A click on empty space lets go of the picture.
  if (drag.value?.kind === 'pan' && !drag.value.moved) comp.selectedId = ''
  drag.value = null
}
function wheel(e: WheelEvent) {
  zoom.value = Math.min(16, Math.max(0.5, zoom.value * Math.exp(-e.deltaY * 0.002)))
}
function resetView() {
  zoom.value = 1
  Object.assign(pan, { x: 0, y: 0 })
}

function keydown(e: KeyboardEvent) {
  const el = e.target instanceof Element ? e.target : null
  if (s.view !== 'carve' || el?.closest('input, textarea, [contenteditable], [role="dialog"], [role="menu"]')) return
  const mod = e.metaKey || e.ctrlKey
  if (mod && e.key.toLowerCase() === 's') {
    e.preventDefault()
    void saveComposition(e.shiftKey)
    return
  }
  const l = sel.value
  if (!l || mod) return
  const step = e.shiftKey ? 0.05 : 0.005
  const moves: Record<string, [number, number]> = { ArrowLeft: [-step, 0], ArrowRight: [step, 0], ArrowUp: [0, -step], ArrowDown: [0, step] }
  if (moves[e.key]) {
    e.preventDefault()
    l.x += moves[e.key][0]
    l.y += moves[e.key][1]
  } else if (e.key === 'Delete' || e.key === 'Backspace') {
    e.preventDefault()
    comp.remove(l.id)
  } else if (e.key === 'Escape') {
    comp.selectedId = ''
  }
}

// ---- Build and burn ----
const jobName = () => (comp.path ? stem(comp.path) : stem(comp.layers[comp.layers.length - 1]?.name ?? 'carved'))
async function compose(path: string, ctx?: StepContext) {
  const r = await call<BuiltIso>('build_carved_iso', { config: { ...c, layers: comp.layers, outputIsoPath: path, preset: s.preset } })
  // A foreign byte means the image is not what was asked for: never write that to a disc.
  if (r.audit.foreignBytes) throw t('carve.selfCheckFail', { n: r.audit.foreignBytes, lba: r.audit.worstLba })
  // An over-dark image is a risk the user is allowed to take: warn, then obey.
  if (!r.summary.limitOk) {
    const question = t('carve.riskBody')
    if (!(await ctx?.ask(question, t('carve.riskContinue')))) throw t('carve.riskDeclined')
  }
}
/** A real burn finished with these parameters: only now may they be shared. */
async function markBurned(preset: DiscGeometryPreset) {
  try {
    await call('save_preset', { preset: { ...preset, burnedAt: new Date().toISOString() } })
    await s.refreshPresets()
  } catch (e) {
    notifyError(String(e))
  }
}
function burn() {
  const preset = s.preset!
  p.openBurn({ title: t('carve.burnTitle'), note: t('carve.burnNote', { name: jobName() }), fileName: 'carved.iso', prepareLabel: t('carve.compose'), prepare: compose,
    rateIssue: issueOf(preset.id) ?? undefined,
    shareable: preset.id.startsWith('user-') ? preset : undefined,
    onBurned: () => markBurned(preset),
    reveal: true,
  })
}
async function exportIso() {
  const out = await pickSavePath(`${jobName()}.iso`)
  if (!out) return
  p.open({ title: t('carve.exportTitle'), note: fileName(out), writes: false, doneMessage: t('carve.exported', { name: fileName(out) }), steps: [{ label: t('carve.compose'), estimate: () => COMPOSE_SECONDS, run: (ctx) => compose(out, ctx) }] })
}

onMounted(() => {
  s.carve = { choose, exportIso, burn, newComposition, openComposition, save: () => saveComposition(false), saveAs: () => saveComposition(true) }
  observer = new ResizeObserver(([e]) => (box.value = { w: e.contentRect.width, h: e.contentRect.height }))
  observer.observe(viewport.value!)
  window.addEventListener('keydown', keydown)
})
onBeforeUnmount(() => {
  s.carve = null
  observer?.disconnect()
  window.removeEventListener('keydown', keydown)
})
</script>

<style scoped>
.drop { position: relative; width: 100%; height: 148px; padding: 0; border: 1px dashed var(--oc-border-strong); border-radius: var(--oc-radius-control); background: var(--oc-bg-base); display: grid; place-items: center; overflow: hidden; }
.drop:hover { background-image: linear-gradient(var(--oc-bg-hover), var(--oc-bg-hover)); }
.drop span { display: flex; flex-direction: column; align-items: center; gap: var(--oc-space-2xs); }
.layers { display: flex; flex-direction: column; gap: 2px; }
.layer { display: flex; align-items: center; gap: var(--oc-space-sm); height: 40px; padding: 0 var(--oc-space-2xs) 0 var(--oc-space-xs); border-radius: var(--oc-radius-control); cursor: pointer; user-select: none; }
.layer:hover { background: var(--oc-bg-hover); }
.layer.on { background: var(--oc-bg-selected, var(--oc-bg-hover)); box-shadow: inset 0 0 0 1px var(--oc-accent); }
.layer img, .layer .thumb { width: 30px; height: 30px; flex: none; object-fit: contain; border-radius: 4px; background: var(--oc-bg-base); }
.layer .tools { display: flex; opacity: 0; }
.layer:hover .tools, .layer.on .tools, .layer:focus-within .tools { opacity: 1; }
a { color: var(--oc-fg-accent); cursor: pointer; }
.action-link { font-size: var(--oc-text-xs); color: var(--oc-fg-muted); text-decoration: none; cursor: pointer; user-select: none; }
.action-link:hover { color: var(--oc-fg-default); }
.side { width: calc(var(--oc-page-gutter) + 348px); flex: none; min-height: 0; display: flex; flex-direction: column; gap: var(--oc-space-md); padding: 64px 0 var(--oc-space-lg) var(--oc-page-gutter); }
.fields { width: auto; max-width: none; min-height: 0; overflow-y: auto; scrollbar-gutter: stable; }
.fields .section + .section { margin-top: var(--oc-space-sm); }
.actions { flex: none; display: flex; gap: var(--oc-space-sm); }
.viewport { position: relative; flex: 1; min-width: 0; min-height: 0; overflow: hidden; display: grid; place-items: center; touch-action: none; }
.disc { position: absolute; left: var(--oc-space-md); top: var(--oc-space-md); width: calc(100% - 24px); height: calc(100% - 72px); object-fit: contain; transform-origin: center; will-change: transform; filter: drop-shadow(0 4px 12px rgba(0, 0, 0, 0.28)); }
.overlay { position: absolute; inset: 0; pointer-events: none; overflow: visible; }
.overlay .frame { fill: none; stroke: var(--oc-accent); stroke-width: 1.5; }
.overlay .handle { fill: #fff; stroke: var(--oc-accent); stroke-width: 1.5; pointer-events: all; }
.overlay circle.handle { cursor: grab; }
.hint { padding: var(--oc-space-2xs) var(--oc-space-md); border-radius: var(--oc-radius-pill); background: var(--oc-bg-glass); backdrop-filter: blur(18px); color: var(--oc-fg-default); }
.spin { position: absolute; top: var(--oc-space-sm); right: var(--oc-space-md); }
.float { position: absolute; right: var(--oc-space-md); bottom: var(--oc-space-sm); pointer-events: none; }
.float.left { right: auto; left: var(--oc-space-md); }
</style>
