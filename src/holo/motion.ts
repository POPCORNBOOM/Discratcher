import { compileExpr, type CompiledExpr } from './expr'

export const MOTION_SAMPLES = 25

export type MotionPresetId = 'none' | 'spinY' | 'tiltX' | 'slideX' | 'grow' | 'custom'

export const MOTION_PRESET_IDS: readonly MotionPresetId[] = ['none', 'spinY', 'tiltX', 'slideX', 'grow', 'custom']

export interface HoloMotion {
  preset: string
  /** The preset's one number: degrees to turn, mm to slide, or % to start from. */
  amount: number
  /** 16 row-major expression strings (scene axes, mm, angles in degrees, t in [0, 1]). */
  matrix: string[]
  /** 25 sampled row-major 4x4 numeric matrices evenly spaced over t in [0, 1]. */
  samples: number[][]
}

/** The number a preset takes: its label, range and unit. */
export interface PresetAmount {
  label: string
  min: number
  max: number
  step: number
  unit: string
  initial: number
}

export const PRESET_AMOUNTS: Partial<Record<MotionPresetId, PresetAmount>> = {
  spinY: { label: 'holo.motionAngle', min: 0, max: 720, step: 5, unit: '°', initial: 90 },
  tiltX: { label: 'holo.motionAngle', min: 0, max: 360, step: 5, unit: '°', initial: 90 },
  slideX: { label: 'holo.motionDistance', min: 0, max: 80, step: 1, unit: ' mm', initial: 40 },
  grow: { label: 'holo.motionStartSize', min: 5, max: 100, step: 5, unit: '%', initial: 50 },
}

/** Presets from before they took a number. */
const LEGACY: Record<string, [MotionPresetId, number]> = {
  spinY90: ['spinY', 90],
  spinY180: ['spinY', 180],
  spinY360: ['spinY', 360],
  tiltX90: ['tiltX', 90],
}

export const IDENTITY_MATRIX_STRINGS: readonly string[] = [
  '1', '0', '0', '0',
  '0', '1', '0', '0',
  '0', '0', '1', '0',
  '0', '0', '0', '1',
]

export const IDENTITY_MATRIX_NUMS: readonly number[] = [
  1, 0, 0, 0,
  0, 1, 0, 0,
  0, 0, 1, 0,
  0, 0, 0, 1,
]

const num = (v: number) => String(Math.round(v * 1000) / 1000)

/** A preset's matrix with its number written in; the motion is centred, so the disc held flat shows the model as placed. */
export function presetMatrixStrings(preset: string, amount = PRESET_AMOUNTS[preset as MotionPresetId]?.initial ?? 0): string[] | null {
  const a = `${num(amount)}*(t-0.5)`
  switch (preset) {
    case 'none':
      return [...IDENTITY_MATRIX_STRINGS]
    case 'spinY':
      return [`cos(${a})`, '0', `sin(${a})`, '0', '0', '1', '0', '0', `-sin(${a})`, '0', `cos(${a})`, '0', '0', '0', '0', '1']
    case 'tiltX':
      return ['1', '0', '0', '0', '0', `cos(${a})`, `-sin(${a})`, '0', '0', `sin(${a})`, `cos(${a})`, '0', '0', '0', '0', '1']
    case 'slideX':
      return ['1', '0', '0', a, '0', '1', '0', '0', '0', '0', '1', '0', '0', '0', '0', '1']
    case 'grow': {
      const s0 = num(amount / 100)
      const s = `${s0}+(1-${s0})*t`
      return [s, '0', '0', '0', '0', s, '0', '0', '0', '0', s, '0', '0', '0', '0', '1']
    }
    default:
      return null
  }
}

export function isPresetId(preset: string): preset is MotionPresetId {
  return (MOTION_PRESET_IDS as readonly string[]).includes(preset)
}

/** A saved preset and its number, reading presets from before they took one. */
export function readPreset(preset: string, amount: unknown): [MotionPresetId, number] | null {
  if (preset in LEGACY) return LEGACY[preset]
  if (!isPresetId(preset)) return null
  const range = PRESET_AMOUNTS[preset]
  if (!range) return [preset, 0]
  const n = typeof amount === 'number' && Number.isFinite(amount) ? amount : range.initial
  return [preset, Math.min(range.max, Math.max(range.min, n))]
}

export type MatrixEvalResult =
  | { ok: true; samples: number[][] }
  | { ok: false; error: string; cell: number }

export function evaluateMatrixSamples(matrix: readonly string[], count = MOTION_SAMPLES): MatrixEvalResult {
  if (matrix.length !== 16) {
    return { ok: false, error: 'Matrix must have 16 cells', cell: 0 }
  }
  const fns: CompiledExpr[] = []
  for (let c = 0; c < 16; c++) {
    const res = compileExpr(matrix[c])
    if (!res.ok) {
      return { ok: false, error: res.error, cell: c }
    }
    fns.push(res.fn)
  }
  const n = Math.max(2, count)
  const samples: number[][] = []
  for (let i = 0; i < n; i++) {
    const t = i / (n - 1)
    const row = new Array<number>(16)
    for (let c = 0; c < 16; c++) {
      const val = fns[c](t)
      if (!Number.isFinite(val)) {
        return { ok: false, error: `Non-finite result at t=${t.toFixed(2)}`, cell: c }
      }
      row[c] = val
    }
    samples.push(row)
  }
  return { ok: true, samples }
}

/** Linearly interpolate `T(t)` from sampled row-major 4x4 matrices (mirrors Rust `Motion::sample`). */
export function sampleMotion(samples: readonly (readonly number[])[], t: number): number[] {
  const n = samples.length
  if (n === 0) return [...IDENTITY_MATRIX_NUMS]
  if (n === 1) return [...samples[0]]
  const tc = Number.isFinite(t) ? Math.min(1, Math.max(0, t)) : 0
  const pos = tc * (n - 1)
  const i0 = Math.min(n - 2, Math.floor(pos))
  const frac = pos - i0
  const a = samples[i0]
  const b = samples[i0 + 1]
  const out = new Array<number>(16)
  for (let k = 0; k < 16; k++) {
    out[k] = a[k] + (b[k] - a[k]) * frac
  }
  return out
}

const DEFAULT_IDENTITY_SAMPLES: readonly (readonly number[])[] = (() => {
  const res = evaluateMatrixSamples(IDENTITY_MATRIX_STRINGS, MOTION_SAMPLES)
  return res.ok ? res.samples : []
})()

export function defaultMotion(): HoloMotion {
  return {
    preset: 'none',
    amount: 0,
    matrix: [...IDENTITY_MATRIX_STRINGS],
    samples: DEFAULT_IDENTITY_SAMPLES.map((s) => [...s]),
  }
}

export function cloneMotion(m: HoloMotion): HoloMotion {
  return {
    preset: m.preset,
    amount: m.amount,
    matrix: [...m.matrix],
    samples: m.samples.map((s) => [...s]),
  }
}

export function makePresetMotion(preset: Exclude<MotionPresetId, 'custom'>, amount = PRESET_AMOUNTS[preset]?.initial ?? 0): HoloMotion {
  const matrix = presetMatrixStrings(preset, amount) ?? [...IDENTITY_MATRIX_STRINGS]
  const res = evaluateMatrixSamples(matrix, MOTION_SAMPLES)
  return {
    preset,
    amount,
    matrix,
    samples: res.ok ? res.samples : DEFAULT_IDENTITY_SAMPLES.map((s) => [...s]),
  }
}
