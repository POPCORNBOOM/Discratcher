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
        <UiButton variant="solid" :disabled="!ready || !!s.job || deciding" @click="begin()">{{ t('cal.start') }}</UiButton>
      </div>
      <div v-if="isMock" class="dev row small">
        <span class="grow muted">{{ t('cal.devNote') }}</span>
        <UiButton variant="ghost" @click="simulate">{{ t('cal.devRun') }}</UiButton>
      </div>
      <UiAdvanced>
        <UiSlider v-model="rungs" :label="t('cal.rungs')" :min="16" :max="72" :step="4" :ends="[t('cal.faster'), t('cal.finer')]" />
        <UiSlider v-model="reads" :label="t('cal.reads')" :min="16" :max="60" :step="4" :ends="[t('cal.faster'), t('cal.finer')]" />
        <UiSlider v-model="lo" :label="t('cal.lo')" :min="1" :max="10" :step="1" :format="(v) => `${v}%`" />
        <UiSlider v-model="hi" :label="t('cal.hi')" :min="80" :max="98" :step="1" :format="(v) => `${v}%`" />
      </UiAdvanced>

    </div>

  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { call, CANCELLED, isMock, mockDev, on } from '../api'
import { calibrationSeconds, CalibrationPlan, CD_MEASURE_FACTOR, isCd, REFERENCE_SHARE } from '../etaModel'
import { twistOf } from '../twist'
import { t } from '../i18n'
import DrivePicker from '../components/DrivePicker.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiButton from '../components/UiButton.vue'
import UiSlider from '../components/UiSlider.vue'
import { notifyError } from '../notices'
import { useProcess, type ProcessStep, type StepContext } from '../stores/process'
import { useStudio } from '../stores/studio'
import type { CalibRungEvent, CalibrationRecord, CalibrationResult, CalibrationRungRecord, DiscCheck, DiscGeometryPreset, DiscKind, OpticalDriveInfo } from '../types'

/** Every DVD calibration image is DVD sized, whatever the drive said a moment ago (issue #8). */
const DVD_SECTORS = 2295104
/** CD-Rs come in several lengths, so a CD image follows the disc, less a margin for the burner's run-out blocks. */
const CD_NOMINAL_SECTORS = 359849
const CD_MARGIN_SECTORS = 64
/** Two seconds apart: a minute for a drive that dropped out to come back. */
const DRIVE_WAIT_TRIES = 30

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
    // The log only feeds the time estimate; the calibration itself is saved either way.
    console.warn(t('cal.logFail', { e: String(e) }))
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
/**
 * What pressing 开始 does, from the one judgement of the disc (`disc`, made in disc-core)
 * and, for a written disc, what its first sectors say. The line above the button and the
 * button itself both come from here, so they cannot disagree.
 */
type Action = 'none' | 'probing' | 'unsupported' | 'checking' | 'burn' | 'erase' | 'measure' | 'notCal' | 'unreadable'
function actionFor(d: OpticalDriveInfo | null, found: DiscKind | 'checking' | null): Action {
  if (!d) return 'none'
  if (d.probing) return 'probing'
  if (d.disc === 'none') return 'none'
  if (d.disc === 'unsupported') return 'unsupported'
  if (d.disc === 'blank') return 'burn'
  if (found === 'checking') return 'checking'
  if (found === 'calibration') return 'measure'
  // A rewritable disc is burned again whatever it holds, even when it does not read.
  if (d.disc === 'erasable') return 'erase'
  return found === 'unreadable' || found === null ? 'unreadable' : 'notCal'
}

/** What a written disc carries; only a written one needs reading, once per disc. */
const found = ref<DiscKind | 'checking' | null>(null)
watch(
  () => (s.drive ? `${s.keyOf(s.drive)}|${s.drive.disc}` : ''),
  async (_, __, stale) => {
    const d = s.drive
    found.value = null
    if (d?.disc !== 'erasable' && d?.disc !== 'written') return
    let current = true
    stale(() => (current = false))
    found.value = 'checking'
    const k = await call<DiscKind>('inspect_disc', { driveId: d.id }).catch(() => null)
    if (current) found.value = k
  },
  { immediate: true },
)
const action = computed(() => actionFor(s.drive, found.value))
const ready = computed(() => ['burn', 'erase', 'measure'].includes(action.value))
function say(a: Action, d: OpticalDriveInfo | null): string {
  const type = d?.mediaType ?? ''
  switch (a) {
    case 'none': return t(!d ? (s.driveStuck ? 'drive.stuck' : s.driveBusy ? 'drive.busy' : 'drive.none') : 'cal.hintNoDisc')
    case 'probing': return t('drive.probing')
    case 'unsupported': return t('drive.unsupported', { type })
    case 'checking': return t('cal.checking')
    case 'burn': return t('cal.willBurn', { type, n: minutes.value + (cd.value ? 8 : 15) })
    case 'erase': return t('cal.willErase', { type, n: minutes.value + (cd.value ? 8 : 15) })
    case 'measure': return t('cal.willMeasure', { n: minutes.value })
    case 'notCal': return t('cal.notCalDisc')
    case 'unreadable': return t('cal.unreadable')
  }
}
const line = computed(() => say(action.value, s.drive))
const cd = computed(() => isCd(s.drive))
/** Held from the last time the drive showed its disc: it lists none for a moment after each session. */
const mediaType = ref('DVD-R')
watch(() => s.drive, (d) => d && d.disc !== 'none' && d.disc !== 'unsupported' && (mediaType.value = d.mediaType), { immediate: true })
const imageSectors = computed(() => (cd.value ? (s.drive?.totalSectors || CD_NOMINAL_SECTORS) - CD_MARGIN_SECTORS : DVD_SECTORS))
const discSectors = computed(() => s.drive?.totalSectors || imageSectors.value)
// disc-probe's CD ladder: fewer rungs cover a track a quarter as long.
watch(cd, (yes) => ([rungs.value, reads.value] = yes ? [36, 32] : [56, 40]), { immediate: true })
const range = computed<[number, number]>(() => [lo.value / 100, hi.value / 100])
const seconds = computed(() => calibrationSeconds(rungs.value, reads.value, range.value, discSectors.value) * (cd.value ? CD_MEASURE_FACTOR : 1))
const minutes = computed(() => Math.ceil(seconds.value / 60))

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms))

/** After a burn the drive takes a while to read the disc again, and some eject it. */
/** Right after a burn the drive lists no disc, or one it cannot read yet, for a while. */
const SETTLE_MS = 20_000
/** A disc that still does not read after this was not burned right. */
const UNREADABLE_MS = 90_000

/** After a burn the drive takes a while to read the disc again, and some eject it. */
async function waitForDisc(step: StepContext) {
  const began = Date.now()
  for (;;) {
    if (p.stopping) throw CANCELLED
    await s.refreshDrives(true).catch(() => {})
    if (s.drive?.probing) {
      step.report(null, t('drive.probing'))
      await sleep(2000)
      continue
    }
    const kind = s.drive ? await call<DiscKind>('inspect_disc', { driveId: s.drive.id }).catch((): DiscKind => 'none') : 'none'
    if (kind === 'calibration') return
    if (kind === 'other' || kind === 'unsupported') throw t('cal.notCalDisc')
    const waited = Date.now() - began
    if (kind === 'unreadable' && waited > UNREADABLE_MS) throw t('cal.burnedUnreadable')
    // Only a disc gone for longer than the drive's own settling was really ejected.
    step.report(null, t(kind === 'none' && waited > SETTLE_MS ? 'cal.reinsert' : 'cal.waitDisc'))
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
      // A foreign calibration disc or a few spots of dust are handled here; nothing to say.
      await call<DiscCheck>('check_calibration_disc', { driveId: s.drive!.id, mediaType: mediaType.value })
    } finally {
      checkCtx = null
    }
  },
}
const measureStep: ProcessStep = { label: t('cal.measureStep'), estimate: () => seconds.value, run: measure }

/** Burns a calibration disc, reads it back and measures it, in one job. */
function burnCalibration() {
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
}

/** Reads back a calibration disc burned earlier and measures it. If it does not check out
 *  and the disc can still be burned, the failure offers burning a new one over it. */
function measureCalibration() {
  const steps = [checkStep, measureStep]
  // A disc that does not check out is burned again; one that checked out but measured
  // unsteadily is only measured again.
  const redo = (failedStep: number) =>
    steps[failedStep] === measureStep
      ? { label: t('cal.remeasure'), note: t('cal.remeasureNote'), run: measureCalibration }
      : { label: t('cal.reburn'), note: t('cal.reburnNote'), writes: true, run: burnCalibration }
  p.open({ title: t('cal.title'), note: t('cal.measureNote'), writes: false, autoClose: true, doneMessage: t('cal.measureDone'), redo, steps })
}

/** The one button: decided the same way as the line above it, from the disc as it is now. */
/** Set while 开始 is deciding, so a second click cannot start the job twice. */
const deciding = ref(false)
async function begin() {
  if (deciding.value) return
  deciding.value = true
  try {
    await decide()
  } finally {
    deciding.value = false
  }
}
async function decide() {
  let d = s.drive
  // The drive drops out of the list while a disc is swapped or a long read lets go of it:
  // wait for it quietly, and speak only if it stays gone.
  for (let i = 0; !d && i < DRIVE_WAIT_TRIES; i++) {
    if (i) await sleep(2000)
    await s.refreshDrives(true).catch(() => {})
    d = s.drive
  }
  const k = d?.disc === 'erasable' || d?.disc === 'written' ? await call<DiscKind>('inspect_disc', { driveId: d.id }).catch(() => null) : null
  const a = actionFor(d, k)
  if (a === 'burn' || a === 'erase') burnCalibration()
  else if (a === 'measure') measureCalibration()
  else notifyError(say(a, d))
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
}


/** Browser-only: put a blank disc in the mock drive, prefill a code, run the real flow. */
function simulate() {
  if (!isMock) return
  mockDev?.setMockDisc('blank')
  s.pendingCode = 'MOCK-0001 A'
  s.refreshDrives(true).then(begin)
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
