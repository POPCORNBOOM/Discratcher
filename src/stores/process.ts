import { defineStore } from 'pinia'
import { computed, reactive, ref, shallowRef } from 'vue'
import { call, CANCELLED, on } from '../api'
import { t } from '../i18n'
import { burnSeconds, COMPOSE_SECONDS, isCd } from '../etaModel'
import { notifySuccess } from '../notices'
import { removeTask, setTask } from '../tasks'
import type { BurnProgress, CarveProgress, DiscGeometryPreset } from '../types'
import type { HoloMode } from '../holo/scene'
import { canBurn } from '../disc'
import { useStudio } from './studio'

const TASK_KEY = 'process'
/** A quick erase of a written DVD+RW, start to finish; diskutil reports no percentage. */
const ERASE_SECONDS = 60

export interface StepContext {
  report: (percent: number | null, message?: string) => void
  /** Stops the job until the user answers; the answer decides whether the step goes on. */
  ask: (question: string, accept: string) => Promise<boolean>
}

export interface ProcessStep {
  label: string
  /** Expected seconds, used until the step has shown its real speed. */
  estimate: () => number
  /** Stopping here costs the user a disc, so stopping has to be confirmed. */
  dangerous?: () => boolean
  run: (ctx: StepContext) => Promise<unknown>
}

export interface ProcessRedo {
  label: string
  /** Why this one click is enough, said right where the failure is. */
  note: string
  /** Burns the disc again: offered only while the disc in the drive is rewritable and burnable. */
  writes?: boolean
  run: () => void
}

export interface ProcessRequest {
  title: string
  note: string
  /** Touches the disc: waits on the page until the user presses 开始刻录. */
  writes: boolean
  /** What to do instead of walking away when this fails, when that can still help: one
   *  offer for the whole job, or one per step that failed. */
  redo?: ProcessRedo | ((failedStep: number) => ProcessRedo | undefined)
  /** Asked when 开始刻录 is pressed: the steps for the disc as it is then, not when the page opened. */
  restep?: () => Promise<Pick<ProcessRequest, 'steps' | 'revealStep'>>
  steps: ProcessStep[]
  /** Leave the page by itself when finished, for work with a next step of its own. */
  autoClose?: boolean
  doneMessage?: string
  /** The downloaded calibration this job used; once done, the user is asked how accurate it was. */
  rateIssue?: string
  /** A calibration of the user's own that this job used; offered for sharing after a real burn. */
  shareable?: DiscGeometryPreset
  /** Called once a real burn (not a test burn, not the virtual drive) has finished. */
  onBurned?: () => void
  cleanup?: () => Promise<unknown>
  onDone?: () => void
  /** The step that writes the composition: as it runs, the picture appears on the disc. */
  revealStep?: number
  /** Show the hologram phone-light viewing diagram on the burn screen. */
  viewingGuide?: boolean | HoloMode
  /** How far to tilt the disc either way, for a transform-mode guide, degrees. */
  viewingTiltDeg?: number
}

export interface BurnPlan {
  title: string
  note: string
  fileName: string
  prepareLabel: string
  prepare: (path: string, ctx: StepContext) => Promise<unknown>
  autoClose?: boolean
  rateIssue?: string
  shareable?: DiscGeometryPreset
  onBurned?: () => void
  onDone?: () => void
  /** Show the composition growing on the disc while it burns. */
  reveal?: boolean
  /** Show the hologram phone-light viewing diagram on the burn screen. */
  viewingGuide?: boolean | HoloMode
  /** How far to tilt the disc either way, for a transform-mode guide, degrees. */
  viewingTiltDeg?: number
  /** Work on the burned disc that belongs to the same job, such as measuring it. */
  after?: ProcessStep[]
  doneMessage?: string
}

/** The one screen every long job runs on, so it looks and stops the same everywhere. */
export const useProcess = defineStore('process', () => {
  const s = useStudio()
  const request = shallowRef<ProcessRequest | null>(null)
  const status = ref<'idle' | 'confirm' | 'running' | 'done' | 'failed'>('idle')
  const stepIndex = ref(0)
  const percent = ref<number | null>(null)
  const message = ref('')
  const failure = ref('')
  const stopping = ref(false)
  /** The finished job really wrote a disc: a test burn or the virtual drive does not count. */
  const burned = ref(false)
  /** Share of the disc written so far, 0 to 1; never goes back, like the dye. */
  const written = ref(0)
  const burnSettings = reactive({ speed: 4, verify: false })
  /** A question a running step cannot answer itself; the page renders it as a dialog. */
  const pendingAsk = ref<{ question: string; accept: string } | null>(null)
  let askResolve: ((yes: boolean) => void) | null = null
  /** Ends the erase step once the burn reports its first write progress. */
  let eraseDone: (() => void) | null = null

  function ask(question: string, accept: string): Promise<boolean> {
    return new Promise((resolve) => {
      askResolve = resolve
      pendingAsk.value = { question, accept }
    })
  }

  function answerAsk(yes: boolean) {
    const resolve = askResolve
    askResolve = null
    pendingAsk.value = null
    resolve?.(yes)
  }
  const step = computed(() => request.value?.steps[stepIndex.value] ?? null)
  const dangerous = computed(() => status.value === 'running' && !!step.value?.dangerous?.())
  const totalSeconds = computed(() => (request.value?.steps ?? []).reduce((sum, x) => sum + x.estimate(), 0))
  const stepSeconds = () => step.value?.estimate() ?? 0
  const laterSeconds = () => (request.value?.steps ?? []).slice(stepIndex.value + 1).reduce((sum, x) => sum + x.estimate(), 0)

  function publish() {
    const req = request.value
    if (!req) return
    const done = stepIndex.value + (percent.value ?? 0) / 100
    setTask({ key: TASK_KEY, title: req.title, detail: message.value, progress: done / req.steps.length })
  }

  function report(p: number | null, m?: string) {
    if (status.value !== 'running') return
    if (p != null) percent.value = p
    if (p != null && stepIndex.value === request.value?.revealStep) written.value = Math.max(written.value, Math.min(1, p / 100))
    if (m) message.value = m
    publish()
  }

  /** The failed job's image is kept, because redo writes it again; leaving the page drops it. */
  const keepImage = ref(false)

  function open(req: ProcessRequest) {
    if (status.value === 'running') {
      s.view = 'process'
      return
    }
    request.value = req
    status.value = 'confirm'
    stepIndex.value = 0
    keepImage.value = !!req.writes && !!req.redo
    percent.value = null
    failure.value = ''
    pendingAsk.value = null
    burned.value = false
    written.value = 0
    s.view = 'process'
    if (!req.writes) void start()
  }

  function openBurn(plan: BurnPlan) {
    let path = ''
    // One backend call does both: it erases a written rewritable disc, then writes. The
    // page shows them as two steps, and the call's own phase says which one is running.
    let burning: Promise<unknown> | null = null
    // One call erases and writes: the erase step starts it and the write step waits on the
    // same one. A call that failed is forgotten, so 重新刻录 can make a new one.
    const burn = () => {
      if (!burning) {
        const p = call('burn_iso_to_drive', { driveId: s.drive!.id, isoPath: path, speed: burnSettings.speed, verify: burnSettings.verify })
        burning = p.catch((e) => {
          burning = null
          throw e
        })
      }
      return burning
    }
    const eraseStep: ProcessStep = {
      label: t('process.eraseDisc'),
      estimate: () => ERASE_SECONDS,
      // Stopping it costs nothing: the disc is rewritable and the write has not begun.
      run: () =>
        new Promise((resolve, reject) => {
          eraseDone = () => resolve(undefined)
          burn().then(resolve, reject)
        }).finally(() => (eraseDone = null)),
    }
    const writeStep: ProcessStep = {
      label: t('process.writeDisc'),
      estimate: () => burnSeconds(burnSettings.speed, burnSettings.verify, isCd(s.drive) ? 'cd' : 'dvd'),
      dangerous: () => true,
      run: burn,
    }
    const prepareStep: ProcessStep = {
      label: plan.prepareLabel,
      estimate: () => COMPOSE_SECONDS,
      run: async (ctx) => {
        path = await call<string>('cache_iso_path', { name: plan.fileName })
        await plan.prepare(path, ctx)
      },
    }
    const stepsFor = () => [prepareStep, ...(s.drive?.disc === 'erasable' ? [eraseStep] : []), writeStep, ...(plan.after ?? [])]
    const steps = stepsFor()
    // A rewritable disc that stopped partway is worth another write rather than a new disc:
    // the erase and write steps run again, and the image is still on disk for them.
    // The steps for the disc as it is now: a disc blank when the page opened is written once
    // a burn on it failed, and then needs the erase.
    const restep = async () => {
      await s.refreshDrives(true)
      const now = stepsFor()
      return { steps: now, revealStep: plan.reveal ? now.indexOf(writeStep) : undefined }
    }
    // Starts again from the erase (or the write when there is none), with the image kept.
    const redo: ProcessRedo = {
      label: t('burn.retry'),
      note: t('burn.retryNote'),
      writes: true,
      run: async () => {
        burned.value = false
        written.value = 0
        request.value = { ...request.value!, ...(await restep()) }
        const at = request.value.steps.findIndex((x) => x === eraseStep || x === writeStep)
        void start(Math.max(at, 0))
      },
    }
    open({
      redo,
      title: plan.title,
      note: plan.note,
      writes: true,
      autoClose: plan.autoClose,
      doneMessage: plan.doneMessage ?? t('process.burnDone'),
      onDone: plan.onDone,
      rateIssue: plan.rateIssue,
      shareable: plan.shareable,
      onBurned: plan.onBurned,
      revealStep: plan.reveal ? steps.indexOf(writeStep) : undefined,
      viewingGuide: plan.viewingGuide,
      viewingTiltDeg: plan.viewingTiltDeg,
      restep,
      cleanup: () => (path ? call('remove_cached_iso', { path }) : Promise.resolve()),
      steps,
    })
  }

  async function start(from = 0) {
    status.value = 'running'
    stopping.value = false
    failure.value = ''
    if (from === 0 && request.value?.restep) request.value = { ...request.value, ...(await request.value.restep()) }
    const req = request.value!
    let wrote = false
    let failed = false
    const ok = await s.run(req.title, async () => {
      try {
        for (let i = from; i < req.steps.length; i++) {
          // A stop landing as one step ends must not start the next: it may be the laser.
          if (stopping.value) throw CANCELLED
          stepIndex.value = i
          percent.value = null
          message.value = req.steps[i].label
          wrote ||= !!req.steps[i].dangerous?.()
          publish()
          await req.steps[i].run({ report, ask })
        }
        return true
      } catch (e) {
        failed = true
        throw e
      } finally {
        // The image of a failed burn on a rewritable disc is what redo writes again.
        if (!failed || !keepImage.value) await req.cleanup?.().catch(() => {})
      }
    })
    pendingAsk.value = null
    removeTask(TASK_KEY)
    await s.refreshDrives(true)
    if (ok) {
      notifySuccess(req.doneMessage ?? t('process.doneSuffix', { title: req.title }))
      status.value = 'done'
      percent.value = 100
      if (req.revealStep != null) written.value = 1
      message.value = req.doneMessage ?? t('process.finished')
      if (req.autoClose || s.view !== 'process') close()
      burned.value = wrote
      if (burned.value) req.onBurned?.()
      req.onDone?.()
    } else if (stopping.value && !req.steps[stepIndex.value]?.dangerous?.()) {
      // Stopped while nothing was being written: a cancel, whatever ran before it.
      close()
    } else {
      failedStep.value = stepIndex.value
      failure.value = stopping.value ? t(s.drive?.rewritable ? 'process.terminatedRw' : 'process.terminatedBad') : s.error
      status.value = 'failed'
    }
    stopping.value = false
  }

  /** The step the job failed at, for an offer that depends on it. */
  const failedStep = ref(0)

  /** The job failed where one click still fixes it: the page offers that instead of 返回. */
  // Offered only while the disc in the drive can still be burned: the drive was asked
  // again as the job ended, so this is the disc as it is now.
  const redo = computed(() => {
    const r = request.value?.redo
    const offer = status.value === 'failed' ? (typeof r === 'function' ? r(failedStep.value) : r) : undefined
    if (!offer) return null
    return !offer.writes || (canBurn(s.drive) && s.drive?.rewritable) ? offer : null
  })

  function stop() {
    stopping.value = true
    call('cancel_job')
  }

  function close() {
    // The image of a failed burn was kept for redo; leaving the page drops it.
    if (keepImage.value) {
      keepImage.value = false
      void request.value?.cleanup?.().catch(() => {})
    }
    status.value = 'idle'
    request.value = null
    if (s.view === 'process') s.back()
  }

  function listen() {
    on<CarveProgress>('carve://progress', (p) => report(p.percent))
    on<BurnProgress>('burn://progress', (p) => {
      // The first word from the write ends the erase step; the picture only follows the
      // write step, so nothing the erase reports can reveal it.
      if (p.phase === 'write') eraseDone?.()
      // The step's own label says what is happening; the tools' English lines do not.
      report(p.percent)
    })
  }

  return { request, burned, written, status, stepIndex, percent, message, failure, stopping, burnSettings, dangerous, totalSeconds, stepSeconds, laterSeconds, open, openBurn, start, redo, stop, close, listen, pendingAsk, answerAsk }
})