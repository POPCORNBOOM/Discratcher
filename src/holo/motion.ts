import { compileExpr, type CompiledExpr } from './expr'

export const MOTION_SAMPLES = 25

export type MotionPresetId = 'none' | 'spinY' | 'tiltX' | 'slideX' | 'grow' | 'custom'

export const MOTION_PRESET_IDS: readonly MotionPresetId[] = ['none', 'spinY', 'tiltX', 'slideX', 'grow', 'custom']

/** A named number the matrix uses as `{{name}}`. */
export interface MotionVar {
  name: string
  value: number
}

export interface HoloMotion {
  preset: string
  /** 16 row-major expression strings (scene axes, mm, angles in degrees, t in [0, 1]); `{{name}}` is a variable. */
  matrix: string[]
  vars: MotionVar[]
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

/**
 * Presets: their matrix and their variables. Turns and slides are centred, so the disc held flat
 * shows the model as placed.
 */
const PRESETS: Record<Exclude<MotionPresetId, 'custom'>, { matrix: readonly string[]; vars: readonly MotionVar[] }> = {
  none: { matrix: IDENTITY_MATRIX_STRINGS, vars: [] },
  spinY: {
    matrix: [
      'cos({{angle}}*(t-0.5))', '0', 'sin({{angle}}*(t-0.5))', '0',
      '0', '1', '0', '0',
      '-sin({{angle}}*(t-0.5))', '0', 'cos({{angle}}*(t-0.5))', '0',
      '0', '0', '0', '1',
    ],
    vars: [{ name: 'angle', value: 90 }],
  },
  tiltX: {
    matrix: [
      '1', '0', '0', '0',
      '0', 'cos({{angle}}*(t-0.5))', '-sin({{angle}}*(t-0.5))', '0',
      '0', 'sin({{angle}}*(t-0.5))', 'cos({{angle}}*(t-0.5))', '0',
      '0', '0', '0', '1',
    ],
    vars: [{ name: 'angle', value: 90 }],
  },
  slideX: {
    matrix: [
      '1', '0', '0', '{{distance}}*(t-0.5)',
      '0', '1', '0', '0',
      '0', '0', '1', '0',
      '0', '0', '0', '1',
    ],
    vars: [{ name: 'distance', value: 40 }],
  },
  grow: {
    matrix: [
      '{{start}}+(1-{{start}})*t', '0', '0', '0',
      '0', '{{start}}+(1-{{start}})*t', '0', '0',
      '0', '0', '{{start}}+(1-{{start}})*t', '0',
      '0', '0', '0', '1',
    ],
    vars: [{ name: 'start', value: 0.5 }],
  },
}

/** Presets from before they had variables, and the value they stood for. */
const LEGACY: Record<string, [Exclude<MotionPresetId, 'custom'>, number]> = {
  spinY90: ['spinY', 90],
  spinY180: ['spinY', 180],
  spinY360: ['spinY', 360],
  tiltX90: ['tiltX', 90],
}

export const VAR_NAME = /^[A-Za-z_][A-Za-z0-9_]*$/

export function isPresetId(preset: string): preset is MotionPresetId {
  return (MOTION_PRESET_IDS as readonly string[]).includes(preset)
}

/** Write each `{{name}}` as its value. */
export function substituteVars(expr: string, vars: readonly MotionVar[]): { ok: true; expr: string } | { ok: false; error: string } {
  let missing = ''
  const out = expr.replace(/\{\{\s*([^}]*?)\s*\}\}/g, (_, name: string) => {
    const v = vars.find((x) => x.name === name)
    if (!v) {
      missing ||= name
      return '0'
    }
    return `(${v.value})`
  })
  return missing ? { ok: false, error: `Unknown variable "${missing}"` } : { ok: true, expr: out }
}

export type MatrixEvalResult =
  | { ok: true; samples: number[][] }
  | { ok: false; error: string; cell: number }

export function evaluateMatrixSamples(matrix: readonly string[], vars: readonly MotionVar[] = [], count = MOTION_SAMPLES): MatrixEvalResult {
  if (matrix.length !== 16) {
    return { ok: false, error: 'Matrix must have 16 cells', cell: 0 }
  }
  const fns: CompiledExpr[] = []
  for (let c = 0; c < 16; c++) {
    const sub = substituteVars(matrix[c], vars)
    if (!sub.ok) return { ok: false, error: sub.error, cell: c }
    const res = compileExpr(sub.expr)
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
  const res = evaluateMatrixSamples(IDENTITY_MATRIX_STRINGS)
  return res.ok ? res.samples : []
})()

export function defaultMotion(): HoloMotion {
  return {
    preset: 'none',
    matrix: [...IDENTITY_MATRIX_STRINGS],
    vars: [],
    samples: DEFAULT_IDENTITY_SAMPLES.map((s) => [...s]),
  }
}

export function cloneMotion(m: HoloMotion): HoloMotion {
  return {
    preset: m.preset,
    matrix: [...m.matrix],
    vars: m.vars.map((v) => ({ ...v })),
    samples: m.samples.map((s) => [...s]),
  }
}

/**
 * A motion from a matrix and its variables, or null when they do not evaluate. A preset keeps its
 * own matrix and variable names; only the values given are taken.
 */
export function makeMotion(preset: string, matrix: readonly string[], vars: readonly MotionVar[]): HoloMotion | null {
  let m = [...matrix]
  let v = vars.map((x) => ({ ...x }))
  if (preset !== 'custom') {
    const p = PRESETS[preset as Exclude<MotionPresetId, 'custom'>]
    if (!p) return null
    m = [...p.matrix]
    v = p.vars.map((x) => ({ name: x.name, value: vars.find((y) => y.name === x.name)?.value ?? x.value }))
  }
  const res = evaluateMatrixSamples(m, v)
  return res.ok ? { preset, matrix: m, vars: v, samples: res.samples } : null
}

export function makePresetMotion(preset: Exclude<MotionPresetId, 'custom'>): HoloMotion {
  return makeMotion(preset, [], []) ?? defaultMotion()
}

/** A saved motion read back, including presets from before they had variables. */
export function readMotion(preset: string, matrix: readonly string[] | null, vars: readonly MotionVar[]): HoloMotion | null {
  const legacy = LEGACY[preset]
  if (legacy) return makeMotion(legacy[0], [], [{ name: PRESETS[legacy[0]].vars[0].name, value: legacy[1] }])
  if (!isPresetId(preset)) return null
  return makeMotion(preset, matrix ?? IDENTITY_MATRIX_STRINGS, vars)
}
