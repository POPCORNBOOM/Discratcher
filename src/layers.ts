// Pictures placed on the disc as layers: what the carve page and the hologram's flat positions
// both do with them. Lists are bottom first.
import { MAX_RING_TURNS } from './ringMap'
import type { ArtworkAnalysis, Layer } from './types'

export const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p

/** The size that lets a picture fill the disc, as one picture usually should. */
export const coverSize = (l: Pick<Layer, 'width' | 'height'>) => Math.max(l.width, l.height) / Math.max(1, Math.min(l.width, l.height))

/** What a layer from an older file lacks. */
const NEW_EFFECTS = { pixelated: false, repeat: false, repeatDensity: 100, repeatAngle: 0, repeatStagger: 0, stretchX: 1, stretchY: 1, ring: false, ringTurns: 1 }

/** A picture's width and height on the disc, as shares of the diameter (`Composition::new`). */
export function layerExtent(l: Pick<Layer, 'width' | 'height' | 'size' | 'stretchX' | 'stretchY'>): [number, number] {
  const long = Math.max(l.width, l.height, 1)
  const s = (v: number) => (Number.isFinite(v) && v > 0 ? v : 1)
  return [(l.size * s(l.stretchX) * l.width) / long, (l.size * s(l.stretchY) * l.height) / long]
}

/** A new layer for `file`. The first picture fills the disc; later ones arrive smaller, on top. */
export function makeLayer(file: string, a: ArtworkAnalysis, first: boolean): Layer {
  const l: Layer = {
    id: crypto.randomUUID(), path: file, name: fileName(file), width: a.width, height: a.height,
    x: 0, y: 0, size: 0, rotationDeg: 0, invert: a.suggestInvert, contrast: 0, brightness: 0,
    ...NEW_EFFECTS,
  }
  l.size = first ? coverSize(l) : 0.5
  return l
}

/** A copy of `l`, nudged aside so it can be told apart. */
export function copyLayer<T extends Layer>(l: T): T {
  return { ...l, id: crypto.randomUUID(), x: l.x + 0.03, y: l.y + 0.03 }
}

/** `list` with `id` moved one place: `by` 1 brings it forward, -1 sends it back; null if it cannot. */
export function restacked<T extends { id: string }>(list: readonly T[], id: string, by: 1 | -1): T[] | null {
  const i = list.findIndex((l) => l.id === id)
  const j = i + by
  if (i < 0 || j < 0 || j >= list.length) return null
  const next = [...list]
  ;[next[i], next[j]] = [next[j], next[i]]
  return next
}

/** A layer read back from a file, with what older files lack filled in; null if it is not one. */
export function readLayer(raw: unknown): Layer | null {
  if (!raw || typeof raw !== 'object') return null
  const l = raw as Partial<Layer>
  if (typeof l.path !== 'string' || !l.path) return null
  const num = (v: unknown, d: number) => (typeof v === 'number' && Number.isFinite(v) ? v : d)
  return {
    ...NEW_EFFECTS,
    ...(l as Layer),
    id: typeof l.id === 'string' && l.id ? l.id : crypto.randomUUID(),
    name: typeof l.name === 'string' && l.name ? l.name : fileName(l.path),
    width: num(l.width, 1), height: num(l.height, 1),
    x: num(l.x, 0), y: num(l.y, 0), size: num(l.size, 1), rotationDeg: num(l.rotationDeg, 0),
    invert: !!l.invert, contrast: num(l.contrast, 0), brightness: num(l.brightness, 0),
    stretchX: num(l.stretchX, 1), stretchY: num(l.stretchY, 1),
    ring: !!l.ring, ringTurns: Math.min(MAX_RING_TURNS, Math.max(1, Math.round(num(l.ringTurns, 1)))),
  }
}
