<template>
  <div class="page page-flow">
    <div class="flow">
      <h1>{{ t('cal.title') }}</h1>
      <p class="small muted">{{ t('cal.tip') }} <a role="button" tabindex="0" @click="s.view = 'models'" @keydown.enter="s.view = 'models'">{{ t('cal.goSearch') }}</a></p>
      <p class="small muted">{{ s.pendingCode ? t('cal.forCode', { code: s.pendingCode }) : t('cal.forUnlisted') }} {{ t('cal.times', { n: minutes }) }}</p>
      <DrivePicker />
      <div class="step">
        <span class="grow small">{{ t(!hasDisc ? 'cal.hintNoDisc' : blank ? 'cal.hintBlank' : 'cal.hintWritten') }}</span>
        <UiButton variant="solid" :disabled="!hasDisc || !!s.job" @click="begin">{{ t('cal.start') }}</UiButton>
      </div>
      <div class="small muted">{{ t('cal.autosave') }}</div>
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

      <section class="records">
        <div class="title">{{ t('cal.records') }}</div>
        <div v-if="!log.length" class="small muted">{{ t('cal.noRecords') }}</div>
        <div class="list">
          <div v-for="r in log" :key="r.id" class="record" :class="{ open: openId === r.id }">
            <button type="button" class="summary" :aria-expanded="openId === r.id" @click="openId = openId === r.id ? '' : r.id">
              <UiIcon :class="state(r)" :path="stateIcon[state(r)]" />
              <span class="grow text">
                <span class="ellipsis">{{ r.stamperCode || r.mediaId || t('cal.unknownDisc') }}</span>
                <span class="small muted ellipsis">{{ when(r.startedAt) }} · {{ stateLabel[state(r)] }}</span>
              </span>
              <span class="mono muted">{{ r.rmsDeg != null ? `${r.rmsDeg.toFixed(3)}°` : '—' }}</span>
            </button>
            <div v-if="openId === r.id" class="detail">
              <dl>
                <div><dt>{{ t('cal.disc') }}</dt><dd>{{ r.mediaId || '—' }}</dd></div>
                <div><dt>{{ t('cal.drive') }}</dt><dd>{{ r.driveModel || '—' }}</dd></div>
                <div><dt>{{ t('cal.rungsDone') }}</dt><dd class="mono">{{ r.rungsMeasured }} / {{ r.rungs }}</dd></div>
                <div><dt>{{ t('cal.readsPer') }}</dt><dd class="mono">{{ t('cal.readsUnit', { n: r.reads }) }}</dd></div>
                <div><dt>{{ t('cal.range') }}</dt><dd class="mono">{{ r.rangePct.map((v) => `${Math.round(v * 100)}%`).join('–') }}</dd></div>
                <div><dt>{{ t('cal.took') }}</dt><dd class="mono">{{ duration(r.durationSeconds) }}</dd></div>
                <div v-if="r.preset"><dt>{{ t('cal.innerRadius') }}</dt><dd class="mono">{{ r.preset.innerRadiusMm.toFixed(6) }} mm</dd></div>
                <div v-if="r.preset"><dt>{{ t('cal.pitch') }}</dt><dd class="mono">{{ r.preset.basePitchUm.toFixed(6) }} µm</dd></div>
              </dl>
              <p v-if="r.message" class="small muted selectable">{{ r.message }}</p>
              <div class="row">
                <UiButton v-if="r.status === 'success' && r.preset" :disabled="!usable(r)" @click="useRecord(r)">{{ t('cal.useCarve') }}</UiButton>
                <UiButton v-if="sharedAs(r)" variant="ghost" @click="viewContribution(r)">{{ t('cal.viewShared') }}</UiButton>
                <UiButton v-else-if="r.status === 'success' && r.preset" variant="ghost" :disabled="!burned(r)" :data-tooltip="burned(r) ? undefined : t('cal.shareTip')" @click="share(r)">{{ t('cal.share') }}</UiButton>
                <UiButton variant="ghost" @click="deleteId = r.id">{{ t('common.delete') }}</UiButton>
              </div>
            </div>
          </div>
        </div>
      </section>
    </div>

    <ContributeDialog v-if="shareTarget" :preset="shareTarget" @close="shareTarget = null" />
    <UiDialog v-if="deleteId" :title="t('cal.deleteTitle')" @close="deleteId = ''">
      <p>{{ t('cal.deleteBody') }}</p>
      <template #actions>
        <UiButton variant="ghost" @click="deleteId = ''">{{ t('common.cancel') }}</UiButton>
        <UiButton variant="danger" @click="removeRecord">{{ t('common.delete') }}</UiButton>
      </template>
    </UiDialog>
  </div>
</template>

<script setup lang="ts">
import { mdiAlertOutline, mdiCheckCircleOutline, mdiCloseCircleOutline, mdiInformationOutline, mdiMinusCircleOutline } from '@mdi/js'
import { computed, onMounted, ref, shallowRef, watch } from 'vue'
import { call, CANCELLED, isMock, loadCatalog, mockDev, on, openInBrowser } from '../api'
import { issueUrl } from '../catalog'
import { calibrationSeconds, CalibrationPlan, REFERENCE_SHARE } from '../etaModel'
import { t } from '../i18n'
import ContributeDialog from '../components/ContributeDialog.vue'
import UiDialog from '../components/UiDialog.vue'
import DrivePicker from '../components/DrivePicker.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiButton from '../components/UiButton.vue'
import UiIcon from '../components/UiIcon.vue'
import UiSlider from '../components/UiSlider.vue'
import { notifyError, notifySuccess, notifyWarning } from '../notices'
import { useProcess, type ProcessStep, type StepContext } from '../stores/process'
import { useStudio } from '../stores/studio'
import type { CalibRungEvent, CalibrationRecord, CalibrationResult, CalibrationRungRecord, CalibrationStatus, DiscCheck, DiscGeometryPreset, DiscKind } from '../types'

/** Every calibration image is DVD sized, whatever the drive said a moment ago (issue #8). */
const CALIBRATION_SECTORS = 2295104

const s = useStudio()
const p = useProcess()
const rungs = ref(56)
const reads = ref(40)
const lo = ref(2)
const hi = ref(96)
/** The calibration being shared; sharing opens only for one that has really been burned. */
const shareTarget = ref<DiscGeometryPreset | null>(null)
const deleteId = ref('')
const log = shallowRef<CalibrationRecord[]>([])
const openId = ref('')
/** A successful calibration is told apart by how far it has been taken: measured, burned, shared. */
type LogState = Exclude<CalibrationStatus, 'success'> | 'unverified' | 'tested' | 'contributed'
const stateLabel = computed<Record<LogState, string>>(() => ({ unverified: t('cal.state.unverified'), tested: t('cal.state.tested'), contributed: t('cal.state.contributed'), suspect: t('cal.state.suspect'), failed: t('cal.state.failed'), cancelled: t('cal.state.cancelled') }))
const stateIcon: Record<LogState, string> = { unverified: mdiAlertOutline, tested: mdiInformationOutline, contributed: mdiCheckCircleOutline, suspect: mdiAlertOutline, failed: mdiCloseCircleOutline, cancelled: mdiMinusCircleOutline }
/** Geometry already in the community catalog; stays empty when the catalog is unreachable. */
const shared = shallowRef<DiscGeometryPreset[]>([])
const same = (a: DiscGeometryPreset, b: DiscGeometryPreset) =>
  [a.basePitchUm - b.basePitchUm, a.pitchLinear - b.pitchLinear, a.pitchQuadratic - b.pitchQuadratic, a.pitchCubic - b.pitchCubic].every((d) => Math.abs(d) < 1e-9)

const sharedAs = (r: CalibrationRecord) => (r.preset ? shared.value.find((p) => same(p, r.preset!)) : undefined)

function state(r: CalibrationRecord): LogState {
  if (r.status !== 'success') return r.status
  const mine = r.preset && s.presets.find((p) => p.id === r.preset!.id)
  if (sharedAs(r)) return 'contributed'
  return mine?.burnedAt ? 'tested' : 'unverified'
}
const when = (iso: string) => new Date(iso).toLocaleString('zh-CN', { year: 'numeric', month: '2-digit', day: '2-digit', hour: '2-digit', minute: '2-digit' })
const duration = (s: number) => (s >= 60 ? t('time.minSec', { m: Math.floor(s / 60), s: Math.round(s % 60) }) : t('time.sec', { s: Math.round(s) }))
const usable = (r: CalibrationRecord) => s.presets.some((p) => p.id === r.preset?.id)

function useRecord(r: CalibrationRecord) {
  s.presetId = r.preset!.id
  s.view = 'carve'
}

const current = (r: CalibrationRecord) => s.presets.find((p) => p.id === r.preset?.id)
const burned = (r: CalibrationRecord) => !!current(r)?.burnedAt

function viewContribution(r: CalibrationRecord) {
  openInBrowser(issueUrl(sharedAs(r)!)).catch((e) => notifyError(String(e)))
}

function share(r: CalibrationRecord) {
  shareTarget.value = current(r) ?? null
}

async function removeRecord() {
  const id = deleteId.value
  deleteId.value = ''
  try {
    await call('delete_calibration_record', { id })
    await loadLog()
  } catch (e) {
    notifyError(String(e))
  }
}

async function loadLog() {
  try {
    log.value = await call<CalibrationRecord[]>('get_calibration_log')
  } catch (e) {
    notifyError(String(e))
  }
}

async function record(entry: Omit<CalibrationRecord, 'id' | 'durationSeconds' | 'startedAt' | 'rungsMeasured' | 'points' | 'rungs' | 'reads' | 'rangePct'>, began: Date) {
  const entryId = `calibration-${began.getTime()}`
  const full: CalibrationRecord = {
    ...entry,
    id: entryId,
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
    await loadLog()
    openId.value = entryId
  } catch (e) {
    notifyError(t('cal.logFail', { e: String(e) }))
  }
}
let ctx: StepContext | null = null
let plan: CalibrationPlan | null = null
let referenceLba = 0
let points: CalibrationRungRecord[] = []
let rungClock = 0

// A CD or Blu-ray disc counts as no disc: neither burning nor measuring means anything on it.
const hasDisc = computed(() => !!s.drive?.mediaPresent && !!s.drive.supported)
const blank = computed(() => hasDisc.value && !!s.drive?.isBlank)
const discSectors = computed(() => s.drive?.totalSectors || CALIBRATION_SECTORS)
const range = computed<[number, number]>(() => [lo.value / 100, hi.value / 100])
const seconds = computed(() => calibrationSeconds(rungs.value, reads.value, range.value, discSectors.value))
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
      const c = await call<DiscCheck>('check_calibration_disc', { driveId: s.drive!.id })
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
  let kind: DiscKind
  try {
    kind = await call<DiscKind>('inspect_disc', { driveId: d.id })
  } catch (e) {
    notifyError(String(e))
    return
  }
  if (kind === 'blank') {
    p.openBurn({
      title: t('cal.title'),
      note: t('cal.burnNote'),
      fileName: 'calibration.iso',
      prepareLabel: t('cal.prepare'),
      prepare: (outputPath) => call('generate_calibration_iso', { outputPath, totalSectors: CALIBRATION_SECTORS }),
      autoClose: true,
      doneMessage: t('cal.measureDone'),
      after: [{ label: t('cal.waitStep'), estimate: () => 10, run: waitForDisc }, checkStep, measureStep],
    })
  } else if (kind === 'calibration') {
    p.open({ title: t('cal.title'), note: t('cal.measureNote'), writes: false, autoClose: true, doneMessage: t('cal.measureDone'), steps: [checkStep, measureStep] })
  } else {
    notifyError(t(kind === 'other' ? 'cal.notCalDisc' : kind === 'unsupported' ? 'cal.unsupported' : 'drive.noDisc'))
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
    r = await call<CalibrationResult>('start_calibration', { driveId: d.id, maxTargets: rungs.value, reps: reads.value, rangePct: range.value })
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
  const preset = {
    ...r.preset,
    id: `user-${Date.now()}`,
    name: s.pendingCode || `${d.mediaId || d.product} · ${stamp}`,
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
  loadLog()
  refreshShared()
})

function refreshShared() {
  loadCatalog().then((all) => (shared.value = all.map((e) => e.preset))).catch(() => {})
}
// A contribution lands on GitHub after the form closes, so look again whenever the page is shown.
watch(() => s.view, (v) => v === 'calibrate' && refreshShared())
</script>

<style scoped>
.records { display: flex; flex-direction: column; gap: var(--oc-space-sm); padding-top: var(--oc-space-md); border-top: 1px solid var(--oc-border-muted); }
a { color: var(--oc-fg-accent); cursor: pointer; }
.list { display: flex; flex-direction: column; gap: 2px; }
.record { border-radius: var(--oc-radius-control); background: var(--oc-bg-glass); backdrop-filter: blur(18px); }
.summary { width: 100%; display: flex; align-items: center; gap: var(--oc-space-sm); padding: var(--oc-space-xs) var(--oc-space-sm); border: 0; border-radius: var(--oc-radius-control); background: transparent; text-align: left; }
.summary:hover { background-color: var(--oc-bg-hover); }
.summary svg { flex: none; }
.text { display: flex; flex-direction: column; min-width: 0; }
.contributed { color: var(--oc-fg-success); }
.tested { color: var(--oc-fg-accent); }
.unverified, .suspect { color: var(--oc-fg-warning); }
.failed { color: var(--oc-fg-danger); }
.cancelled { color: var(--oc-fg-muted); }
.detail { display: flex; flex-direction: column; gap: var(--oc-space-sm); padding: var(--oc-space-2xs) var(--oc-space-sm) var(--oc-space-sm) 32px; }
dl { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: var(--oc-space-xs) var(--oc-space-md); margin: 0; }
dt { color: var(--oc-fg-muted); font-size: var(--oc-text-xs); }
dd { margin: 0; overflow-wrap: anywhere; }
.step { display: flex; align-items: center; gap: var(--oc-space-sm); }
.dev { padding-top: var(--oc-space-sm); border-top: 1px dashed var(--oc-border-muted); }
</style>
