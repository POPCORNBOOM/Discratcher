<template>
  <div class="page carve">
    <aside class="side">
      <div class="flow fields">
        <h1>{{ t('carve.title') }}</h1>
        <PresetList />
        <section class="section">
          <div class="title">{{ t('carve.image') }}</div>
          <button class="drop" type="button" @click="choose">
            <img v-if="analysis" :src="analysis.thumbnail" alt="" />
            <span v-else class="muted"><UiIcon :path="mdiImagePlusOutline" :size="20" />{{ t('carve.choose') }}</span>
          </button>
          <div v-if="analysis" class="row small muted">
            <span class="grow ellipsis" :data-tooltip="c.sourceImagePath" data-tooltip-overflow>{{ fileName(c.sourceImagePath) }}</span>
            <span class="mono">{{ analysis.width }}×{{ analysis.height }}</span>
          </div>
          <div v-if="analysis?.suggestInvert && !c.invertImage" class="notice warn">
            <UiIcon :path="mdiAlertOutline" />
            <div class="grow">
              {{ t('carve.darkWarn') }}
              <a role="button" tabindex="0" @click="c.invertImage = true" @keydown.enter="c.invertImage = true">{{ t('carve.invertWhite') }}</a>
            </div>
          </div>
          <UiSegmented v-model="c.cropMode" :options="crops" />
          <UiSlider v-if="c.cropMode === 'custom'" v-model="c.scale" :label="t('carve.scale')" :min="0.2" :max="3" :step="0.01" :format="(v) => `${Math.round(v * 100)}%`" />
        </section>

        <UiAdvanced>
          <section class="section">
            <div class="title row">
              <span class="grow">{{ t('carve.position') }}</span>
              <a v-if="hasTransform" class="action-link" role="button" tabindex="0" @click="resetTransform" @keydown.enter="resetTransform">{{ t('carve.reset') }}</a>
            </div>
            <UiSlider v-model="c.offsetX" :label="t('carve.x')" :min="-50" :max="50" :step="0.5" :format="formatOffset" />
            <UiSlider v-model="c.offsetY" :label="t('carve.y')" :min="-50" :max="50" :step="0.5" :format="formatOffset" />
            <UiSlider v-model="c.rotationDeg" :label="t('carve.rotate')" :min="-180" :max="180" :step="0.5" :format="(v) => `${v.toFixed(1)}°`" />
          </section>

          <section class="section">
            <div class="title">{{ t('carve.tone') }}</div>
            <UiSwitch v-model="c.invertImage">{{ t('carve.invert') }}</UiSwitch>
            <UiSlider v-model="c.contrast" :label="t('carve.contrast')" :min="-100" :max="100" :step="1" />
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

    <div ref="viewport" class="viewport" data-disc-area @wheel.prevent="wheel" @pointerdown="dragStart" @pointermove="dragMove" @pointerup="drag = null" @dblclick="resetView">
      <img v-if="previewUrl && !s.discRendered" class="disc" :src="previewUrl" :alt="t('carve.previewAlt')" draggable="false" :style="{ transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})` }" />
      <div v-if="!previewUrl" class="hint">{{ t('carve.hint') }}</div>
      <div v-if="previewing" class="spin small muted">{{ t('carve.updating') }}</div>
      <div class="float mono muted">{{ Math.round(zoom * 100) }}%</div>
      <div v-if="hasTransform" class="float left mono muted">{{ t('carve.center', { x: formatOffset(c.offsetX), y: formatOffset(c.offsetY) }) }}<template v-if="c.cropMode === 'custom'"> · {{ Math.round(c.scale * 100) }}%</template></div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { mdiAlertOctagonOutline, mdiAlertOutline, mdiFire, mdiImagePlusOutline } from '@mdi/js'
import { computed, onBeforeUnmount, onMounted, ref, toRef, watch } from 'vue'
import { call, pickImage, pickSavePath } from '../api'
import PresetList from '../components/PresetList.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiButton from '../components/UiButton.vue'
import UiIcon from '../components/UiIcon.vue'
import UiSegmented from '../components/UiSegmented.vue'
import UiSlider from '../components/UiSlider.vue'
import UiSwitch from '../components/UiSwitch.vue'
import { COMPOSE_SECONDS } from '../etaModel'
import { t } from '../i18n'
import { issueOf } from '../catalog'
import { notifyError } from '../notices'
import { useProcess, type StepContext } from '../stores/process'
import { useStudio } from '../stores/studio'
import type { ArtworkAnalysis, BuiltIso, DiscGeometryPreset } from '../types'

const s = useStudio()
const p = useProcess()
const c = s.config
const crops = computed(() => [
  { value: 'square-center', label: t('carve.cropCenter') },
  { value: 'fit', label: t('carve.cropFit') },
  { value: 'stretch', label: t('carve.cropStretch') },
  { value: 'custom', label: t('carve.cropCustom') },
])
/** The servo safety line the build measures against; going past it is allowed but warned. */
const MAX_SECTOR_DARK = 0.7
const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p
const formatOffset = (v: number) => (v > 0 ? `+${v.toFixed(1)}%` : `${v.toFixed(1)}%`)
const hasTransform = computed(() => c.offsetX !== 0 || c.offsetY !== 0 || c.rotationDeg !== 0 || (c.cropMode === 'custom' && c.scale !== 1))
function resetTransform() {
  c.offsetX = 0
  c.offsetY = 0
  c.rotationDeg = 0
  c.scale = 1
}

const analysis = ref<ArtworkAnalysis | null>(null)
async function choose() {
  const p = await pickImage()
  if (!p) return
  try {
    analysis.value = await call<ArtworkAnalysis>('analyze_artwork', { imagePath: p })
    c.sourceImagePath = p
    c.invertImage = analysis.value.suggestInvert
  } catch (e) {
    notifyError(String(e))
  }
}

const previewUrl = ref('')
const previewing = ref(false)
let ticket = 0
let timer: ReturnType<typeof setTimeout> | undefined
watch([() => ({ ...c }), () => s.preset], () => {
  clearTimeout(timer)
  timer = setTimeout(refreshPreview, 120)
})
async function refreshPreview() {
  if (!c.sourceImagePath || !s.preset) return
  const mine = ++ticket
  previewing.value = true
  try {
    const url = await call<string>('simulate_disc_preview', {
      config: { ...c, preset: s.preset },
      previewSize: 1024,
    })
    if (mine === ticket) previewUrl.value = s.discArt = url
  } catch (e) {
    notifyError(String(e))
  } finally {
    if (mine === ticket) previewing.value = false
  }
}

const zoom = toRef(s, 'discZoom')
const pan = s.discPan
let drag: { x: number; y: number } | null = null
function wheel(e: WheelEvent) {
  zoom.value = Math.min(16, Math.max(0.5, zoom.value * Math.exp(-e.deltaY * 0.002)))
}
function dragStart(e: PointerEvent) {
  if ((e.target as HTMLElement).closest('.float')) return
  drag = { x: e.clientX - pan.x, y: e.clientY - pan.y }
}
function dragMove(e: PointerEvent) {
  if (drag) Object.assign(pan, { x: e.clientX - drag.x, y: e.clientY - drag.y })
}
function resetView() {
  zoom.value = 1
  Object.assign(pan, { x: 0, y: 0 })
}

async function compose(path: string, ctx?: StepContext) {
  const r = await call<BuiltIso>('build_carved_iso', { config: { ...c, outputIsoPath: path, preset: s.preset } })
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
  p.openBurn({ title: t('carve.burnTitle'), note: t('carve.burnNote', { name: fileName(c.sourceImagePath) }), fileName: 'carved.iso', prepareLabel: t('carve.compose'), prepare: compose,
    rateIssue: issueOf(preset.id) ?? undefined,
    shareable: preset.id.startsWith('user-') ? preset : undefined,
    onBurned: () => markBurned(preset),
  })
}
async function exportIso() {
  const base = fileName(c.sourceImagePath).replace(/\.[^.]+$/, '')
  const out = await pickSavePath(`${base || 'carved'}.iso`)
  if (!out) return
  p.open({ title: t('carve.exportTitle'), note: fileName(out), writes: false, doneMessage: t('carve.exported', { name: fileName(out) }), steps: [{ label: t('carve.compose'), estimate: () => COMPOSE_SECONDS, run: (ctx) => compose(out, ctx) }] })
}

onMounted(() => (s.carve = { choose, exportIso, burn }))
onBeforeUnmount(() => (s.carve = null))
</script>

<style scoped>
.drop { position: relative; width: 100%; height: 148px; padding: 0; border: 1px dashed var(--oc-border-strong); border-radius: var(--oc-radius-control); background: var(--oc-bg-base); display: grid; place-items: center; overflow: hidden; }
.drop:hover { background-image: linear-gradient(var(--oc-bg-hover), var(--oc-bg-hover)); }
.drop img { position: absolute; inset: 0; width: 100%; height: 100%; object-fit: contain; }
.drop span { display: flex; flex-direction: column; align-items: center; gap: var(--oc-space-2xs); }
a { color: var(--oc-fg-accent); cursor: pointer; }
.action-link { font-size: var(--oc-text-xs); color: var(--oc-fg-muted); text-decoration: none; cursor: pointer; user-select: none; }
.action-link:hover { color: var(--oc-fg-default); }
.side { width: calc(var(--oc-page-gutter) + 348px); flex: none; min-height: 0; display: flex; flex-direction: column; gap: var(--oc-space-md); padding: 64px 0 var(--oc-space-lg) var(--oc-page-gutter); }
.fields { width: auto; max-width: none; min-height: 0; overflow-y: auto; scrollbar-gutter: stable; }
.fields .section + .section { margin-top: var(--oc-space-sm); }
.actions { flex: none; display: flex; gap: var(--oc-space-sm); }
.viewport { position: relative; flex: 1; min-width: 0; min-height: 0; overflow: hidden; display: grid; place-items: center; }
.disc { position: absolute; left: var(--oc-space-md); top: var(--oc-space-md); width: calc(100% - 24px); height: calc(100% - 72px); object-fit: contain; transform-origin: center; will-change: transform; filter: drop-shadow(0 4px 12px rgba(0, 0, 0, 0.28)); }
.hint { padding: var(--oc-space-2xs) var(--oc-space-md); border-radius: var(--oc-radius-pill); background: var(--oc-bg-glass); backdrop-filter: blur(18px); color: var(--oc-fg-default); }
.spin { position: absolute; top: var(--oc-space-sm); right: var(--oc-space-md); }
.float { position: absolute; right: var(--oc-space-md); bottom: var(--oc-space-sm); pointer-events: none; }
.float.left { right: auto; left: var(--oc-space-md); }
</style>