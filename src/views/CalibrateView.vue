<template>
  <div class="page page-flow">
    <div class="flow">
      <h1>{{ s.pendingCode ? t('cal.titleFor', { code: s.pendingCode }) : t('cal.title') }}</h1>
      <DrivePicker bare />
      <div v-if="discMid" class="row small">
        <span class="grow muted">{{ t('cal.discMid') }}</span>
        <span class="mono selectable">{{ discMid }}</span>
      </div>
      <div class="step">
        <span class="grow">{{ line }}</span>
        <UiButton variant="solid" :disabled="!ready || !!s.job" @click="begin">{{ t('cal.start') }}</UiButton>
      </div>
      <div v-if="isMock" class="dev row small">
        <span class="grow muted">{{ t('cal.devNote') }}</span>
        <UiButton variant="ghost" @click="simulate">{{ t('cal.devRun') }}</UiButton>
      </div>
      <UiAdvanced>
        <UiSlider v-model="rungs" :label="t('cal.rungs')" :min="16" :max="72" :step="4" />
        <UiSlider v-model="reads" :label="t('cal.reads')" :min="16" :max="60" :step="4" />
        <UiSlider v-model="lo" :label="t('cal.lo')" :min="1" :max="10" :step="1" :format="(v) => `${v}%`" />
        <UiSlider v-model="hi" :label="t('cal.hi')" :min="80" :max="98" :step="1" :format="(v) => `${v}%`" />
      </UiAdvanced>

    </div>

  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { call, CANCELLED, isMock, mockDev, on } from '../api'
import { calibrationSeconds, CalibrationPlan, CD_MEASURE_FACTOR, isCd, isDvdRam, REFERENCE_SHARE } from '../etaModel'
import { twistOf } from '../twist'
import { t, unsupportedText } from '../i18n'
import DrivePicker from '../components/DrivePicker.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiButton from '../components/UiButton.vue'
import UiSlider from '../components/UiSlider.vue'
import { notifyError, notifySuccess, notifyWarning } from '../notices'
import { useProcess, type ProcessStep, type StepContext } from '../stores/process'
import { useStudio } from '../stores/studio'
import type { CalibRungEvent, CalibrationRecord, CalibrationResult, CalibrationRungRecord, DiscCheck, DiscGeometryPreset, DiscKind } from '../types'

/** Every DVD calibration image is DVD sized, whatever the drive said a moment ago (issue #8). */
const DVD_SECTORS = 2295104
/** CD-Rs come in several lengths, so a CD image follows the disc, less a margin for the burner's run-out blocks. */
const CD_NOMINAL_SECTORS = 359849
const CD_MARGIN_SECTORS = 64

const s = useStudio()
const p = useProcess()
const rungs = ref(56)
const reads = ref(40)
const lo = ref(2)
const hi = ref(96)
async function record(entry: Omit<CalibrationRecord, 'id' | 'durationSeconds' | 'startedAt' | 'rungsMeasured' | 'points' | 'rungs' | 'reads' | 'rangePct'>, began: Date) {
  const full: CalibrationRecord = {
    ...entry,
    id: `calibration-${began.getTime()}`,
    startedAt: began.toISOString(),
    durationSeconds: (Date.now() - began.getTime()) / 1000,
    rungs: rungs.value,
    reads: reads.value,
    rangePct: range.value,
    rungsMeasured: points.length,
    points,
  }
  try {
    await call('add_calibration_record', { record: full })
  } catch (e) {
    notifyError(t('cal.logFail', { e: String(e) }))
  }
}
let ctx: StepContext | null = null
let plan: CalibrationPlan | null = null
let referenceLba = 0
let points: CalibrationRungRecord[] = []
let rungClock = 0

/** The disc in the drive names itself, and calibrating uses up a blank disc of
 *  this model: the code says which one that is before anything is burned. */
const discMid = computed(() => s.drive?.mediaId ?? '')
/** What the disc is, found out before the button is pressed. `unknown` when the read failed: pressing the button says why. */
const kind = ref<DiscKind | 'checking' | 'unknown'>('none')
watch(
  () => (s.drive ? [s.keyOf(s.drive), s.drive.mediaPresent, s.drive.supported, s.drive.isBlank, s.drive.reusable].join() : ''),
  async (key, _, stale) => {
    const d = s.drive
    // The listing settles everything but a written disc, so only that one is read.
    if (!d?.mediaPresent) return (kind.value = 'none')
    if (!d.supported) return (kind.value = 'unsupported')
    if (d.isBlank || d.reusable) return (kind.value = 'blank')
    let current = true
    stale(() => (current = false))
    kind.value = 'checking'
    const found = await call<DiscKind>('inspect_disc', { driveId: d.id }).catch(() => 'unknown' as const)
    if (current) kind.value = found
  },
  { immediate: true },
)
const ready = computed(() => kind.value === 'blank' || kind.value === 'calibration' || kind.value === 'unknown')
const line = computed(() => {
  const d = s.drive
  if (!d) return t(s.driveStuck ? 'drive.stuck' : 'drive.none')
  // A DVD-RAM can be recorded, but its spindle turns at a different speed in
  // every zone, so the ladder cannot measure it yet.
  if (isDvdRam(d)) return t('drive.unsupportedDvdRam')
  switch (kind.value) {
    case 'checking': return t('cal.checking')
    case 'none': return t('cal.hintNoDisc')
    case 'unsupported': return unsupportedText(d)
    case 'blank': return t(d.isBlank ? 'cal.willBurn' : 'cal.willErase', { type: d.mediaType, n: minutes.value + (cd.value ? 8 : 15) })
    case 'calibration': return t('cal.willMeasure', { n: minutes.value })
    case 'other': return t('cal.notCalDisc')
    default: return t('drive.written')
  }
})
const cd = computed(() => isCd(s.drive))
/** Held from the last time the drive showed its disc: it lists none for a moment after each session. */
const mediaType = ref('DVD-R')
watch(() => s.drive, (d) => d?.mediaPresent && d.supported && (mediaType.value = d.mediaType), { immediate: true })
const imageSectors = computed(() => (cd.value ? (s.drive?.totalSectors || CD_NOMINAL_SECTORS) - CD_MARGIN_SECTORS : DVD_SECTORS))
const discSectors = computed(() => s.drive?.totalSectors || imageSectors.value)
// disc-probe's CD ladder: fewer rungs cover a track a quarter as long.
watch(cd, (yes) => ([rungs.value, reads.value] = yes ? [36, 32] : [56, 40]), { immediate: true })
const range = computed<[number, number]>(() => [lo.value / 100, hi.value / 100])
const seconds = computed(() => calibrationSeconds(rungs.value, reads.value, range.value, discSectors.value) * (cd.value ? CD_MEASURE_FACTOR : 1))
const minutes = computed(() => Math.ceil(seconds.value / 60))

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

/** After a burn the drive takes a while to read the disc again, and some eject it. */
async function waitForDisc(step: StepContext) {
  for (;;) {
    if (p.stopping) throw CANCELLED
    await s.refreshDrives().catch(() => {})
    const kind = s.drive ? await call<DiscKind>('inspect_disc', { driveId: s.drive.id }).catch((): DiscKind => 'none') : 'none'
    if (kind === 'calibration') return
    if (kind === 'other' || kind === 'unsupported') throw t('cal.notCalDisc')
    step.report(null, t(kind === 'none' ? 'cal.reinsert' : 'cal.waitDisc'))
    await sleep(2000)
  }
}

let checkCtx: StepContext | null = null
const checkStep: ProcessStep = {
  label: t('cal.checkStep'),
  estimate: () => 20,
  run: async (step) => {
    checkCtx = step
    try {
      const c = await call<DiscCheck>('check_calibration_disc', { driveId: s.drive!.id, mediaType: mediaType.value })
      if (c.foreign) notifyWarning(t('cal.foreign'))
    } finally {
      checkCtx = null
    }
  },
}
const measureStep: ProcessStep = { label: t('cal.measureStep'), estimate: () => seconds.value, run: measure }

/**
 * One button. A blank DVD is burned, read back and measured in one job; a calibration
 * disc burned earlier is read back and measured; anything else is refused with the
 * reason. Every stage checks its input before the next one trusts it.
 */
async function begin() {
  const d = s.drive
  if (!d) return
  if (isDvdRam(d)) {
    notifyError(t('drive.unsupportedDvdRam'))
    return
  }
  let kind: DiscKind
  try {
    kind = await call<DiscKind>('inspect_disc', { driveId: d.id })
  } catch (e) {
    notifyError(String(e))
    return
  }
  // A used rewritable disc is erased by the burn itself, so it needs a calibration image
  // just as a blank one does.
  if (kind === 'blank' || (kind === 'other' && d.reusable)) {
    p.openBurn({
      title: t('cal.title'),
      note: t('cal.burnNote'),
      fileName: 'calibration.iso',
      prepareLabel: t('cal.prepare'),
      prepare: (outputPath) => call('generate_calibration_iso', { outputPath, totalSectors: imageSectors.value }),
      autoClose: true,
      doneMessage: t('cal.measureDone'),
      after: [{ label: t('cal.waitStep'), estimate: () => 10, run: waitForDisc }, checkStep, measureStep],
    })
  } else if (kind === 'calibration') {
    p.open({ title: t('cal.title'), note: t('cal.measureNote'), writes: false, autoClose: true, doneMessage: t('cal.measureDone'), steps: [checkStep, measureStep] })
  } else if (kind === 'unsupported') {
    // The drive says why this disc cannot be recorded; the generic line would not.
    notifyError(unsupportedText(d))
  } else {
    notifyError(t(kind === 'other' ? 'cal.notCalDisc' : 'drive.noDisc'))
  }
}

async function measure(step: StepContext) {
  const d = s.drive!
  const began = new Date()
  const about = { driveModel: `${d.vendor} ${d.product} ${d.revision}`.trim(), mediaId: d.mediaId, stamperCode: s.pendingCode }
  ctx = step
  plan = new CalibrationPlan(rungs.value, reads.value, range.value, discSectors.value)
  referenceLba = Math.floor((discSectors.value * REFERENCE_SHARE) / 16) * 16
  points = []
  rungClock = performance.now()
  let r: CalibrationResult
  try {
    r = await call<CalibrationResult>('start_calibration', { driveId: d.id, mediaType: mediaType.value, maxTargets: rungs.value, reps: reads.value, rangePct: range.value })
  } catch (e) {
    const cancelled = e === CANCELLED
    await record({ ...about, status: cancelled ? 'cancelled' : 'failed', message: cancelled ? '' : String(e), rmsDeg: null, preset: null }, began)
    throw e
  }
  if (r.suspect) {
    await record({ ...about, status: 'suspect', message: t('cal.suspectMsg'), rmsDeg: r.cubicRmsDeg, preset: null }, began)
    throw t('cal.suspectErr')
  }
  if (r.specProblem) {
    await record({ ...about, status: 'suspect', message: r.specProblem, rmsDeg: r.cubicRmsDeg, preset: null }, began)
    throw r.specProblem
  }
  const stamp = new Date().toLocaleString('zh-CN', { month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
  const preset: DiscGeometryPreset = {
    ...r.preset,
    // What this drive's twist chart showed last time holds for every CD it reads.
    ...(r.preset.media === 'cd' ? { alongTrackFrames: twistOf(about.driveModel) } : {}),
    id: `user-${Date.now()}`,
    mediaId: d.mediaId,
    stamperCode: s.pendingCode,
    driveModel: about.driveModel,
    createdAt: new Date().toISOString(),
  }
  await call('save_preset', { preset })
  await s.refreshPresets()
  s.presetId = preset.id
  await record({ ...about, status: 'success', message: '', rmsDeg: r.cubicRmsDeg, preset }, began)
  s.pendingCode = ''
  // Nothing is offered for sharing yet: the parameters must first survive a real burn.
  s.view = 'carve'
  notifySuccess(t('cal.saved'))
}


/** Browser-only: put a blank disc in the mock drive, prefill a code, run the real flow. */
function simulate() {
  if (!isMock) return
  mockDev?.setMockDisc('blank')
  s.pendingCode = 'MOCK-0001 A'
  s.refreshDrives().then(begin)
}

onMounted(() => {
  on<CalibRungEvent>('calib://rung-progress', (e) => {
    const clock = performance.now()
    points.push({ lba: e.lba, radiusMm: e.radiusMm, phaseDeg: e.phaseDeg, sigmaDeg: e.sigmaDeg, residualDeg: e.constPitchResidualDeg, seconds: (clock - rungClock) / 1000 })
    rungClock = clock
    if (plan) ctx?.report(plan.rungDone(e.lba, referenceLba))
  })
  on<string>('calib://status', (m) => ctx?.report(null, m))
  on<number>('calib://check', (percent) => checkCtx?.report(percent))
})

</script>

<style scoped>
.step { display: flex; align-items: center; gap: var(--oc-space-sm); }
.dev { padding-top: var(--oc-space-sm); border-top: 1px dashed var(--oc-border-muted); }
</style>
