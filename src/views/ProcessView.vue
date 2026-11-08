<template>
  <section class="process">
    <div v-if="r" class="flow">
      <h1>{{ r.title }}</h1>
      <p class="lede">{{ r.note }}</p>

      <template v-if="p.status === 'confirm'">
        <DrivePicker />
        <div v-if="warning" class="notice warn">
          <UiIcon :path="mdiAlertOutline" />
          <div>{{ warning }}</div>
        </div>
        <p class="small muted">{{ t('burn.estimate', { n: minutes }) }}</p>
        <UiAdvanced>
          <div class="field">
            <div class="field-head">
              <span class="small muted">{{ t('burn.speed') }}</span>
              <span v-if="s.drive?.maxWriteSpeed" class="mono muted small">{{ t('burn.speedMax', { n: s.drive.maxWriteSpeed }) }}</span>
            </div>
            <UiSelect
              v-model="speedString"
              :label="t('burn.speed')"
              :options="speedOptions"
              :disabled="!speedOptions.length || !!s.job"
              :placeholder="t('burn.speedNone')"
            />
          </div>
          <UiSwitch v-model="b.verify">{{ t('burn.verify') }}</UiSwitch>
          <UiSwitch v-model="b.testOnly">{{ t('burn.testOnly') }}</UiSwitch>
        </UiAdvanced>
        <div class="actions"><UiButton variant="solid" size="lg" :icon="mdiFire" :disabled="!usable" @click="p.start()">{{ t(b.testOnly ? 'burn.startTest' : 'burn.start') }}</UiButton></div>
      </template>

      <template v-else>
        <ol class="steps">
          <li v-for="(x, i) in r.steps" :key="i" :class="stepState(i)">
            <UiIcon :path="stepIcon(i)" /><span>{{ x.label }}</span>
          </li>
        </ol>
        <div class="figure mono">{{ p.percent != null ? `${p.percent.toFixed(0)}%` : '—' }}</div>
        <div class="row small">
          <span class="grow ellipsis">{{ p.message }}</span>
          <span v-if="eta" class="muted">{{ eta }}</span>
        </div>
        <div class="bar"><i :style="{ width: `${p.percent ?? 0}%` }" /></div>

        <div v-if="p.status === 'failed'" class="notice danger">
          <UiIcon :path="mdiAlertOctagonOutline" />
          <div class="selectable">{{ p.failure }}</div>
        </div>
        <div v-if="confirmStop" class="notice warn">
          <UiIcon :path="mdiAlertOutline" />
          <div>{{ t('burn.stopWarn') }}</div>
        </div>

        <div v-if="rating" class="rate">
          <span class="small muted">{{ t('burn.rateIntro') }}</span>
          <div class="stars">
            <button v-for="n in 5" :key="n" type="button" class="star" :class="{ on: n <= hover }" :aria-label="t('burn.starLabel', { n })" :data-tooltip="scores[n - 1]" @pointerenter="hover = n" @pointerleave="hover = 0" @click="rate(n)">
              <UiIcon :path="n <= hover ? mdiStar : mdiStarOutline" />
            </button>
          </div>
          <span class="small muted">{{ t('burn.rateNote') }}</span>
        </div>

        <div v-if="shareable" class="rate">
          <span class="small muted">{{ t('burn.shareIntro') }}</span>
          <div class="actions-inline"><UiButton variant="soft" @click="sharing = true">{{ t('burn.share') }}</UiButton></div>
        </div>

        <div class="actions">
          <UiButton v-if="p.status === 'done' || p.status === 'failed'" :variant="p.status === 'done' ? 'solid' : 'soft'" size="lg" @click="p.close()">{{ t(p.status === 'done' ? 'common.done' : 'common.back') }}</UiButton>
          <UiButton v-else-if="p.stopping" variant="danger" size="lg" disabled>{{ t('burn.stopping') }}</UiButton>
          <template v-else-if="confirmStop">
            <UiButton variant="ghost" size="lg" @click="confirmStop = false">{{ t('burn.continue') }}</UiButton>
            <UiButton variant="danger" size="lg" @click="stop">{{ t('burn.confirmStop') }}</UiButton>
          </template>
          <UiButton v-else-if="p.dangerous" variant="danger" size="lg" @click="confirmStop = true">{{ t('burn.stop') }}</UiButton>
          <UiButton v-else size="lg" @click="stop">{{ t('common.cancel') }}</UiButton>
        </div>
      </template>
    </div>
  <UiDialog v-if="p.pendingAsk" :title="t('burn.riskTitle')" @close="p.answerAsk(false)">
    <p class="selectable">{{ p.pendingAsk.question }}</p>
    <template #actions>
      <UiButton variant="ghost" size="lg" @click="p.answerAsk(false)">{{ t('common.cancel') }}</UiButton>
      <UiButton variant="danger" size="lg" @click="p.answerAsk(true)">{{ p.pendingAsk.accept }}</UiButton>
    </template>
  </UiDialog>
  <ContributeDialog v-if="sharing && shareable" :preset="shareable" @close="sharing = false" />
  </section>
</template>

<script setup lang="ts">
import { mdiAlertOctagonOutline, mdiAlertOutline, mdiCheck, mdiCircleOutline, mdiClose, mdiFire, mdiProgressClock, mdiStar, mdiStarOutline } from '@mdi/js'
import { computed, ref, watch } from 'vue'
import ContributeDialog from '../components/ContributeDialog.vue'
import DrivePicker from '../components/DrivePicker.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiButton from '../components/UiButton.vue'
import UiDialog from '../components/UiDialog.vue'
import UiIcon from '../components/UiIcon.vue'
import UiSelect from '../components/UiSelect.vue'
import UiSwitch from '../components/UiSwitch.vue'
import { openInBrowser } from '../api'
import { ratingUrl } from '../catalog'
import { notifyError } from '../notices'
import { t } from '../i18n'
import { useEta } from '../eta'
import { useProcess } from '../stores/process'
import { useStudio } from '../stores/studio'


const s = useStudio()
const p = useProcess()
const b = p.burnSettings
const r = computed(() => p.request)
const confirmStop = ref(false)
const hover = ref(0)
const scores = computed(() => [1, 2, 3, 4, 5].map((n) => t(`burn.score${n}`)))
/** A real burn that used a downloaded calibration: only then is there something to rate. */
const rating = computed(() => (p.status === 'done' && p.burned ? r.value?.rateIssue : undefined))
const shareable = computed(() => (p.status === 'done' && p.burned ? r.value?.shareable : undefined))
const sharing = ref(false)

async function rate(score: number) {
  try {
    await openInBrowser(ratingUrl(rating.value!, score))
  } catch (e) {
    notifyError(String(e))
  }
}
const eta = useEta({ running: () => p.status === 'running', step: () => p.stepIndex, percent: () => p.percent, stepSeconds: p.stepSeconds, laterSeconds: p.laterSeconds })
const minutes = computed(() => Math.ceil(p.totalSeconds / 60))

const usable = computed(() => !!s.drive?.mediaPresent && !!s.drive.supported && (s.drive.isBlank || s.drive.simulated || b.testOnly) && speedOptions.value.length > 0)
const warning = computed(() => {
  if (!s.drive) return t('burn.warnNoDrive')
  if (s.drive.simulated) return t('burn.warnSim')
  if (!s.drive.mediaPresent) return t('burn.warnNoDisc')
  if (!s.drive.supported) return t('drive.unsupported', { type: s.drive.mediaType })
  if (!s.drive.isBlank) return t('burn.warnNotBlank')
  if (!speedOptions.value.length) return t('burn.warnNoSpeed')
  return ''
})

const speedOptions = computed(() => {
  const d = s.drive
  if (!d || !d.mediaPresent) return []
  let speeds: number[] = []
  if (d.writeSpeeds && d.writeSpeeds.length > 0) {
    speeds = [...d.writeSpeeds]
  } else if (d.maxWriteSpeed && d.maxWriteSpeed > 0) {
    const isCd = d.mediaType.toUpperCase().includes('CD')
    const candidates = isCd ? [4, 8, 10, 16, 24, 32, 40, 48] : [2, 4, 6, 8, 12, 16]
    speeds = candidates.filter((x) => x <= d.maxWriteSpeed!)
  } else if (d.isBlank) {
    speeds = [2, 4, 8, 16]
  }
  speeds = Array.from(new Set(speeds)).sort((a, b) => a - b)
  return speeds.map((speed) => {
    let label = `${speed}x`
    if (speed === 4) label += t('burn.recommended')
    else if (speed === speeds[speeds.length - 1] && speeds.length > 1) label += t('burn.fastest')
    return { value: String(speed), label }
  })
})

const speedString = computed({
  get: () => String(b.speed),
  set: (val: string) => {
    const num = Number(val)
    if (!Number.isNaN(num) && num > 0) {
      b.speed = num
    }
  },
})

watch(
  speedOptions,
  (options) => {
    if (!options.length) return
    const hasCurrent = options.some((o) => o.value === String(b.speed))
    if (!hasCurrent) {
      const has4x = options.some((o) => o.value === '4')
      if (has4x) {
        b.speed = 4
      } else {
        const nums = options.map((o) => Number(o.value))
        nums.sort((x, y) => Math.abs(x - 4) - Math.abs(y - 4))
        b.speed = nums[0]
      }
    }
  },
  { immediate: true },
)

watch(() => p.status, () => (confirmStop.value = false))
watch(() => p.dangerous, (d) => d || (confirmStop.value = false))

function stop() {
  confirmStop.value = false
  p.stop()
}
function stepState(i: number) {
  if (i < p.stepIndex || p.status === 'done') return 'past'
  return i === p.stepIndex ? (p.status === 'failed' ? 'failed' : 'current') : ''
}
function stepIcon(i: number) {
  return { past: mdiCheck, current: mdiProgressClock, failed: mdiClose, '': mdiCircleOutline }[stepState(i)]!
}
</script>

<style scoped>
.process { position: relative; flex: 1; min-width: 0; overflow: hidden; display: flex; align-items: center; padding-left: var(--oc-page-gutter); }
.lede { color: var(--oc-fg-default); }
.steps { list-style: none; margin: 0; padding: 0; display: flex; flex-direction: column; gap: var(--oc-space-xs); color: var(--oc-fg-subtle); }
.steps li { display: flex; align-items: center; gap: var(--oc-space-xs); }
.steps .past { color: var(--oc-fg-muted); }
.steps .past svg { color: var(--oc-fg-success); }
.steps .current { color: var(--oc-fg-default); }
.steps .current svg { color: var(--oc-fg-accent); }
.steps .failed { color: var(--oc-fg-default); }
.steps .failed svg { color: var(--oc-fg-danger); }
.figure { font-size: 64px; line-height: 1; font-weight: 500; letter-spacing: -0.03em; color: #f5f5f7; }
.actions { display: flex; gap: var(--oc-space-sm); margin-top: var(--oc-space-sm); }
.rate { display: flex; flex-direction: column; gap: var(--oc-space-xs); padding: var(--oc-space-sm) var(--oc-space-md); border-radius: var(--oc-radius-control); background: var(--oc-bg-glass); backdrop-filter: blur(18px); }
.actions-inline { display: flex; }
.stars { display: flex; gap: 2px; }
.star { width: var(--oc-size-md); height: var(--oc-size-md); display: grid; place-items: center; border: 0; border-radius: var(--oc-radius-control); background: transparent; color: var(--oc-fg-muted); }
.star.on { color: var(--oc-fg-accent); }
.field { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
.field-head { display: flex; justify-content: space-between; align-items: baseline; }
</style>