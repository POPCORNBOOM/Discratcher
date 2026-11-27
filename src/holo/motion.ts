import { compileExpr, type CompiledExpr } from './expr'

export const MOTION_SAMPLES = 25

export type MotionPresetId =
  | 'none'
  | 'spinY90'
  | 'spinY180'
  | 'spinY360'
  | 'tiltX90'
  | 'slideX'
  | 'grow'
  | 'custom'

export const MOTION_PRESET_IDS: readonly MotionPresetId[] = [
  'none',
  'spinY90',
  'spinY180',
  'spinY360',
  'tiltX90',
  'slideX',
  'grow',
  'custom',
]

export interface HoloMotion {
  preset: string
  /** 16 row-major expression strings (scene axes, mm, angles in degrees, t in [0, 1]). */
  matrix: string[]
  /** 25 sampled row-major 4x4 numeric matrices evenly spaced over t in [0, 1]. */
  samples: number[][]
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

const PRESET_MATRICES: Record<Exclude<MotionPresetId, 'custom'>, readonly string[]> = {
  none: IDENTITY_MATRIX_STRINGS,
  spinY90: [
    'cos(90*(t-0.5))', '0', 'sin(90*(t-0.5))', '0',
    '0', '1', '0', '0',
    '-sin(90*(t-0.5))', '0', 'cos(90*(t-0.5))', '0',
    '0', '0', '0', '1',
  ],
  spinY180: [
    'cos(180*(t-0.5))', '0', 'sin(180*(t-0.5))', '0',
    '0', '1', '0', '0',
    '-sin(180*(t-0.5))', '0', 'cos(180*(t-0.5))', '0',
    '0', '0', '0', '1',
  ],
  spinY360: [
    'cos(360*t)', '0', 'sin(360*t)', '0',
    '0', '1', '0', '0',
    '-sin(360*t)', '0', 'cos(360*t)', '0',
    '0', '0', '0', '1',
  ],
  tiltX90: [
    '1', '0', '0', '0',
    '0', 'cos(90*(t-0.5))', '-sin(90*(t-0.5))', '0',
    '0', 'sin(90*(t-0.5))', 'cos(90*(t-0.5))', '0',
    '0', '0', '0', '1',
  ],
  slideX: [
    '1', '0', '0', '40*(t-0.5)',
    '0', '1', '0', '0',
    '0', '0', '1', '0',
    '0', '0', '0', '1',
  ],
  grow: [
    '0.5+0.5*t', '0', '0', '0',
    '0', '0.5+0.5*t', '0', '0',
    '0', '0', '0.5+0.5*t', '0',
    '0', '0', '0', '1',
  ],
}

export function isPresetId(preset: string): preset is MotionPresetId {
  return (MOTION_PRESET_IDS as readonly string[]).includes(preset)
}

export function presetMatrixStrings(preset: string): string[] | null {
  if (preset in PRESET_MATRICES) {
    return [...PRESET_MATRICES[preset as Exclude<MotionPresetId, 'custom'>]]
  }
  return null
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
    matrix: [...IDENTITY_MATRIX_STRINGS],
    samples: DEFAULT_IDENTITY_SAMPLES.map((s) => [...s]),
  }
}

export function cloneMotion(m: HoloMotion): HoloMotion {
  return {
    preset: m.preset,
    matrix: [...m.matrix],
    samples: m.samples.map((s) => [...s]),
  }
}

export function makePresetMotion(preset: Exclude<MotionPresetId, 'custom'>): HoloMotion {
  const matrix = [...PRESET_MATRICES[preset]]
  const res = evaluateMatrixSamples(matrix, MOTION_SAMPLES)
  return {
    preset,
    matrix,
    samples: res.ok ? res.samples : DEFAULT_IDENTITY_SAMPLES.map((s) => [...s]),
  }
}
