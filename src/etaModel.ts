// Remaining-time models. Pure, so they can be replayed against recorded runs.

/** Share of progress after which measured speed fully replaces the prior estimate. */
const PRIOR_FADE_PERCENT = 12
/** The recent rate looks back over at least this much progress. */
const RECENT_MIN_PERCENT = 6
const RECENT_SHARE_OF_ELAPSED = 0.3

/**
 * Remaining seconds of one step. The prior carries the first moments, when measured speed
 * is mostly start-up noise. After that the recent rate wins over the average, because
 * drives speed up towards the outer edge and an average would stay too pessimistic.
 */
export class StepEstimator {
  private samples: { t: number; p: number }[] = []

  constructor(private priorSeconds: number) {}

  add(t: number, percent: number) {
    const last = this.samples[this.samples.length - 1]
    if (!last || percent > last.p) this.samples.push({ t, p: percent })
  }

  private rate(t: number) {
    const first = this.samples[0]
    const last = this.samples[this.samples.length - 1]
    if (!first || last.p <= first.p || last.t <= first.t) return 0
    const since = t - (t - first.t) * RECENT_SHARE_OF_ELAPSED
    const start = [...this.samples].reverse().find((x) => x.t <= since && last.p - x.p >= RECENT_MIN_PERCENT) ?? first
    return (last.p - start.p) / (last.t - start.t)
  }

  /** `t` and the samples are in seconds. */
  remaining(t: number) {
    const first = this.samples[0]
    if (!first) return this.priorSeconds
    const last = this.samples[this.samples.length - 1]
    const left = 100 - last.p
    const prior = (this.priorSeconds * left) / 100
    const rate = this.rate(t)
    if (!rate) return Math.max(prior - (t - last.t), 0)
    const trust = Math.min(1, (last.p - first.p) / PRIOR_FADE_PERCENT)
    return Math.max(prior * (1 - trust) + (left / rate) * trust - (t - last.t), 0)
  }
}

// Calibration. On the GUD1N a rung of 40 pairs takes 5.47 + 17.61·√d seconds, where d is
// its distance from the reference as a share of the disc (rms 1.1 s over 56 rungs).
const SECONDS_PER_PAIR = 5.47 / 40
const SEEK_RATIO = 17.61 / 5.47
/** The ladder's first rungs sit a few turns from the reference and barely seek. */
const NEAR_RUNGS = 10
export const REFERENCE_SHARE = 0.49
const SETTLE_SECONDS = 10

const rungCost = (distanceShare: number) => 1 + SEEK_RATIO * Math.sqrt(distanceShare)

/** Mean cost of a rung spread evenly over the measured range. */
function farRungCost(lo: number, hi: number) {
  const steps = 200
  let sum = 0
  for (let i = 0; i < steps; i++) sum += rungCost(Math.abs(lo + ((hi - lo) * (i + 0.5)) / steps - REFERENCE_SHARE))
  return sum / steps
}

export class CalibrationPlan {
  private far: number
  private done = 0
  private count = 0

  constructor(private rungs: number, private reads: number, private range: [number, number], private totalSectors: number) {
    this.far = farRungCost(range[0], range[1])
  }

  private left() {
    const near = Math.max(Math.min(NEAR_RUNGS, this.rungs) - this.count, 0)
    return near + (this.rungs - this.count - near) * this.far
  }

  get seconds() {
    return SETTLE_SECONDS + this.left() * this.reads * SECONDS_PER_PAIR
  }

  /** Records a finished rung and returns progress as a share of the work, not of rungs. */
  rungDone(lba: number, referenceLba: number) {
    this.done += this.count < NEAR_RUNGS ? 1 : rungCost(Math.abs(lba - referenceLba) / this.totalSectors)
    this.count++
    return (this.done / (this.done + this.left())) * 100
  }
}

export function calibrationSeconds(rungs: number, reads: number, range: [number, number], totalSectors: number) {
  return new CalibrationPlan(rungs, reads, range, totalSectors).seconds
}

// Burning. A 4.7 GB DVD at 1x, 1385 kB/s; an 80 minute CD at 1x plays for 80 minutes.
const ONE_X_SECONDS = { dvd: 3394, cd: 4800 }

export function burnSeconds(speed: number, verify: boolean, media: 'dvd' | 'cd' = 'dvd') {
  return (ONE_X_SECONDS[media] / speed) * (verify ? 2 : 1)
}

/** A CD is measured at 8x, where a turn takes 48 ms against a DVD's 25 ms at 4x. */
export const CD_MEASURE_FACTOR = 1.9

export const isCd = (d: { mediaType: string } | null | undefined) => !!d?.mediaType.toUpperCase().startsWith('CD')

/** Measured: 4.7 GB in 22 s at 212 MB/s. */
export const COMPOSE_SECONDS = 22
