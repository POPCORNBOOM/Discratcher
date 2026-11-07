import type { DiscGeometryPreset } from './types'

export interface DiscLook {
  dye: [number, number, number]
  pitch: number
  seed: number
  /** Etched round the hub, the way the real code is. */
  code: string
  /** Half width of the preview image, in millimetres. */
  artRange: number
}

/** Recording dyes as they read on real discs. */
const DYES: [number, number, number][] = [
  [0.7, 0.62, 0.8], // violet
  [0.52, 0.66, 0.86], // azo blue
  [0.5, 0.78, 0.72], // cyan-green
  [0.84, 0.74, 0.5], // gold
  [0.84, 0.56, 0.72], // magenta
  [0.72, 0.74, 0.78], // silver
]
const DEFAULT_CODE = 'DSCRTCHR-DVDR47-0001 L1'

function hash(text: string) {
  let h = 2166136261
  for (const c of text) h = Math.imul(h ^ c.codePointAt(0)!, 16777619)
  return h >>> 0
}

/** The same disc always looks the same, and a different disc looks different. */
export function lookOf(preset: DiscGeometryPreset | null): DiscLook {
  if (!preset) return { dye: DYES[0], pitch: 0.74, seed: 0.3, code: DEFAULT_CODE, artRange: 58.5 }
  const h = hash(preset.mediaId || preset.stamperCode || preset.name)
  return {
    dye: DYES[h % DYES.length],
    pitch: preset.basePitchUm,
    seed: ((h >>> 16) % 1000) / 1000,
    code: preset.stamperCode || preset.name,
    artRange: preset.outerRadiusMm + 0.6,
  }
}
