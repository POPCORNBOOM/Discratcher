import { defineStore } from 'pinia'
import { computed, reactive, ref, shallowRef } from 'vue'
import { call, CANCELLED, on } from '../api'
import { t } from '../i18n'
import { burnSeconds, COMPOSE_SECONDS, isCd } from '../etaModel'
import { notifySuccess, notifyWarning } from '../notices'
import { removeTask, setTask } from '../tasks'
import type { BurnProgress, CarveProgress, DiscGeometryPreset } from '../types'
import { useStudio } from './studio'

const TASK_KEY = 'process'

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

export interface ProcessRequest {
  title: string
  note: string
  /** Touches the disc: waits on the page until the user presses 开始刻录. */
  writes: boolean
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

  function open(req: ProcessRequest) {
    if (status.value === 'running') {
      s.view = 'process'
      return
    }
    request.value = req
    status.value = 'confirm'
    stepIndex.value = 0
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
    open({
      title: plan.title,
      note: plan.note,
      writes: true,
      autoClose: plan.autoClose,
      doneMessage: plan.doneMessage ?? t('process.burnDone'),
      onDone: plan.onDone,
      rateIssue: plan.rateIssue,
      shareable: plan.shareable,
      onBurned: plan.onBurned,
      revealStep: plan.reveal ? 1 : undefined,
      cleanup: () => (path ? call('remove_cached_iso', { path }) : Promise.resolve()),
      steps: [
        {
          label: plan.prepareLabel,
          estimate: () => COMPOSE_SECONDS,
          run: async (ctx) => {
            path = await call<string>('cache_iso_path', { name: plan.fileName })
            await plan.prepare(path, ctx)
          },
        },
        {
          label: t('process.writeDisc'),
          estimate: () => burnSeconds(burnSettings.speed, burnSettings.verify, isCd(s.drive) ? 'cd' : 'dvd'),
          dangerous: () => true,
          run: () => call('burn_iso_to_drive', { driveId: s.drive!.id, isoPath: path, speed: burnSettings.speed, verify: burnSettings.verify }),
        },
        ...(plan.after ?? []),
      ],
    })
  }

  async function start() {
    const req = request.value!
    let wrote = false
    // The virtual drive plays a burn without writing, so it proves nothing about the parameters.
    const real = !s.drive?.simulated
    status.value = 'running'
    stopping.value = false
    const ok = await s.run(req.title, async () => {
      try {
        for (let i = 0; i < req.steps.length; i++) {
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
      } finally {
        await req.cleanup?.().catch(() => {})
      }
    })
    pendingAsk.value = null
    removeTask(TASK_KEY)
    await s.refreshDrives()
    if (ok) {
      notifySuccess(req.doneMessage ?? t('process.doneSuffix', { title: req.title }))
      status.value = 'done'
      percent.value = 100
      if (req.revealStep != null) written.value = 1
      message.value = req.doneMessage ?? t('process.finished')
      if (req.autoClose || s.view !== 'process') close()
      burned.value = wrote && real
      if (burned.value) req.onBurned?.()
      req.onDone?.()
    } else if (stopping.value && !wrote) {
      notifyWarning(t('process.cancelled'))
      close()
    } else {
      failure.value = stopping.value ? t('process.terminatedBad') : s.error
      if (stopping.value) notifyWarning(t('process.terminated'))
      status.value = 'failed'
    }
  }

  function stop() {
    stopping.value = true
    call('cancel_job')
  }

  function close() {
    status.value = 'idle'
    request.value = null
    if (s.view === 'process') s.back()
  }

  function listen() {
    on<CarveProgress>('carve://progress', (p) => report(p.percent))
    on<BurnProgress>('burn://progress', (p) => report(p.percent, p.message))
  }

  return { request, burned, written, status, stepIndex, percent, message, failure, stopping, burnSettings, dangerous, totalSeconds, stepSeconds, laterSeconds, open, openBurn, start, stop, close, listen, pendingAsk, answerAsk }
})