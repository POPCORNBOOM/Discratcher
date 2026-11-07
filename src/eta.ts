import { onBeforeUnmount, ref, watch, type Ref } from 'vue'
import { t } from './i18n'
import { StepEstimator } from './etaModel'

/** Sources for the time left in a whole job, not only the step it is on. */
export interface EtaSource {
  running: () => boolean
  /** Changes when the job moves to its next step. */
  step: () => number
  percent: () => number | null
  stepSeconds: () => number
  laterSeconds: () => number
}

/** Small swings in the estimate are noise; showing them only makes it look unsure. */
const HYSTERESIS = 0.06
const TICK_MS = 1000

export function formatRemaining(seconds: number) {
  if (seconds >= 90) return t('eta.minutes', { n: Math.ceil(seconds / 60) })
  return t('eta.seconds', { n: Math.max(5, Math.ceil(seconds / 5) * 5) })
}

export function useEta(source: EtaSource): Ref<string> {
  const text = ref('')
  const now = () => performance.now() / 1000
  let estimator: StepEstimator | null = null
  let shown = 0
  let timer = 0

  function refresh() {
    if (!estimator) return
    const left = estimator.remaining(now()) + source.laterSeconds()
    if (!shown || Math.abs(left - shown) > shown * HYSTERESIS) shown = left
    else shown = Math.max(shown - TICK_MS / 1000, 0)
    text.value = shown > 0 ? formatRemaining(shown) : ''
  }

  function begin() {
    estimator = new StepEstimator(source.stepSeconds())
    refresh()
  }

  function end() {
    clearInterval(timer)
    estimator = null
    shown = 0
    text.value = ''
  }

  // Synchronous: a step's end and the next one's first sample can land in the same tick.
  watch(source.running, (on) => {
    end()
    if (!on) return
    begin()
    timer = window.setInterval(refresh, TICK_MS)
  }, { flush: 'sync', immediate: true })
  watch(source.step, () => source.running() && begin(), { flush: 'sync' })
  watch(source.percent, (p) => {
    if (p == null || !estimator) return
    estimator.add(now(), p)
    refresh()
  }, { flush: 'sync' })

  onBeforeUnmount(end)
  return text
}
