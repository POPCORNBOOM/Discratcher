import type { DiscGeometryPreset } from '../types'
import {
  DEFAULT_DATA_INNER_MM,
  DEFAULT_DATA_OUTER_MM,
  DEFAULT_OBJECT_DEPTH_MM,
  DEFAULT_OBJECT_SCALE_MM,
  LAMP_HELD_MM,
  type Vec3,
} from './constants'

export const SCENE_VERSION = 1 as const
const LAST_SCENE_STORAGE_KEY = 'discratcher.holo.lastScene.v1'

export interface HoloSceneObject {
  id: string
  path: string
  name: string
  visible: boolean
  positionMm: Vec3
  rotationDeg: Vec3
  scaleMm: number
  positionId: string
}

/** One way the disc is turned to be looked at, with its own objects. */
export interface HoloPosition {
  id: string
  name: string
  /** Which angle of the disc is at the top, clockwise in degrees. */
  phiDeg: number
}

export interface HoloScene {
  version: typeof SCENE_VERSION
  positions: HoloPosition[]
  objects: HoloSceneObject[]
  lampMm?: Vec3
}

/**
 * Convert a 3D point or direction vector from scene space (+x right, +y up, +z toward viewer)
 * into the disc picture frame used by Rust `design::solve` / `design::render` (+x right, +y DOWN,
 * +z toward viewer).
 *
 * The disc picture frame is y DOWN, so a naive copy mirrors the model; the 3D editor display
 * must match what the burned disc shows. Keep this conversion isolated here (mirroring
 * `disc_core::holo::model3d::scene_to_disc`).
 */
export function sceneToDiscVec3(v: Vec3): Vec3 {
  return [v[0], -v[1], v[2]]
}

/** Viewer eye position in scene space (6 o'clock side, looking down at `tiltDeg`, swaying along x). */
export function viewerEyeSceneMm(distanceMm: number, tiltDeg: number, headXMm: number): Vec3 {
  const a = (tiltDeg * Math.PI) / 180
  return [headXMm, -distanceMm * Math.sin(a), distanceMm * Math.cos(a)]
}

/** Viewer eye position converted into the disc picture frame via `sceneToDiscVec3`. */
export function viewerEyeDiscMm(distanceMm: number, tiltDeg: number, headXMm: number): Vec3 {
  return sceneToDiscVec3(viewerEyeSceneMm(distanceMm, tiltDeg, headXMm))
}

export function discRadiiMm(preset: DiscGeometryPreset | null | undefined): { inner: number; outer: number } {
  if (!preset || preset.media === 'cd') {
    return { inner: DEFAULT_DATA_INNER_MM, outer: DEFAULT_DATA_OUTER_MM }
  }
  return { inner: preset.innerRadiusMm, outer: preset.outerRadiusMm }
}

/** Where a new object starts: in the best region, clear of those already there. */
export function defaultObjectPlacement(
  preset: DiscGeometryPreset | null | undefined,
  existingCount = 0,
): Pick<HoloSceneObject, 'positionMm' | 'rotationDeg' | 'scaleMm'> {
  const { inner, outer } = discRadiiMm(preset)
  const rMid = 0.5 * (inner + outer)
  // The first stands at the top, the next at the bottom, then beside them.
  const presets: Vec3[] = [
    [0, rMid, DEFAULT_OBJECT_DEPTH_MM],
    [0, -rMid, DEFAULT_OBJECT_DEPTH_MM],
    [-16, rMid - 4, 4],
    [16, rMid - 4, 4],
  ]
  const p = presets[existingCount % presets.length]
  return {
    positionMm: [round1(p[0]), round1(p[1]), round1(p[2])],
    rotationDeg: [0, 0, 0],
    scaleMm: DEFAULT_OBJECT_SCALE_MM,
  }
}

export function cloneScene(scene: HoloScene): HoloScene {
  return {
    version: SCENE_VERSION,
    ...(scene.lampMm ? { lampMm: [...scene.lampMm] as Vec3 } : {}),
    positions: scene.positions.map((p) => ({ ...p })),
    objects: scene.objects.map((o) => ({
      ...o,
      positionMm: [...o.positionMm] as Vec3,
      rotationDeg: [...o.rotationDeg] as Vec3,
    })),
  }
}

export function emptyScene(): HoloScene {
  return { version: SCENE_VERSION, positions: [makePosition(0)], objects: [] }
}

export function makePosition(phiDeg: number): HoloPosition {
  return { id: makeObjectId(), name: '', phiDeg }
}

export const wrapDeg = (d: number) => ((Math.round(d * 10) / 10) % 360 + 360) % 360

/**
 * An angle for a new position. Every position has two best regions, opposite each other, so
 * the widest gap is looked for over half a turn, and the side with more room is taken.
 */
export function freePhi(positions: HoloPosition[]): number {
  const at = positions.map((p) => wrapDeg(p.phiDeg) % 180).sort((a, b) => a - b)
  if (!at.length) return 0
  let best = { gap: -1, mid: 0 }
  at.forEach((a, i) => {
    const gap = (i + 1 < at.length ? at[i + 1] : at[0] + 180) - a
    if (gap > best.gap + 1e-6) best = { gap, mid: a + gap / 2 }
  })
  const room = (d: number) => Math.min(...positions.map((p) => Math.abs(((d - p.phiDeg + 540) % 360) - 180)))
  return wrapDeg(room(best.mid + 180) > room(best.mid) + 1e-6 ? best.mid + 180 : best.mid)
}

export function makeObjectId(): string {
  return `obj-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`
}

export function fileBaseName(path: string): string {
  const file = path.split(/[\\/]/).pop() ?? path
  return file.replace(/\.[^.]+$/, '') || file
}

export function round1(v: number): number {
  return Math.round(v * 10) / 10
}

/** Plain-word depth description for the selected object: 浮出 8 mm / 贴在盘面 / 嵌入 6 mm. */
export function formatDepthWords(
  zMm: number,
  translate: (key: string, params?: Record<string, string | number>) => string,
): string {
  const rounded = Math.round(zMm)
  if (rounded > 0) return translate('holo.depthFloat', { n: rounded })
  if (rounded < 0) return translate('holo.depthSunk', { n: Math.abs(rounded) })
  return translate('holo.depthFlush')
}

function asVec3(raw: unknown, fallback: Vec3): Vec3 {
  if (!Array.isArray(raw) || raw.length !== 3) return [...fallback]
  const out = raw.map((n, i) => (typeof n === 'number' && Number.isFinite(n) ? n : fallback[i]))
  return [out[0], out[1], out[2]]
}

/** Parse and validate a versioned scene JSON payload. Throws a concise error on invalid data. */
export function validateScene(raw: unknown): HoloScene {
  if (!raw || typeof raw !== 'object') throw new Error('Invalid scene payload')
  const rec = raw as Record<string, unknown>
  if (rec.version !== SCENE_VERSION) {
    throw new Error(`Unsupported scene version: ${String(rec.version)}`)
  }
  if (!Array.isArray(rec.objects)) throw new Error('Scene objects must be an array')
  const positions: HoloPosition[] = []
  for (const item of Array.isArray(rec.positions) ? rec.positions : []) {
    const q = item as Record<string, unknown> | null
    if (q && typeof q.id === 'string' && q.id && typeof q.phiDeg === 'number' && Number.isFinite(q.phiDeg)) {
      positions.push({ id: q.id, name: typeof q.name === 'string' ? q.name : '', phiDeg: wrapDeg(q.phiDeg) })
    }
  }
  // A scene from before there were positions is one, not turned.
  if (!positions.length) positions.push(makePosition(0))
  const objects: HoloSceneObject[] = []
  for (const item of rec.objects) {
    if (!item || typeof item !== 'object') continue
    const o = item as Record<string, unknown>
    if (typeof o.path !== 'string' || !o.path.trim()) continue
    objects.push({
      id: typeof o.id === 'string' && o.id ? o.id : makeObjectId(),
      path: o.path,
      name: typeof o.name === 'string' && o.name ? o.name : fileBaseName(o.path),
      visible: typeof o.visible === 'boolean' ? o.visible : true,
      positionMm: asVec3(o.positionMm, [0, -41, DEFAULT_OBJECT_DEPTH_MM]),
      rotationDeg: asVec3(o.rotationDeg, [0, 0, 0]),
      scaleMm:
        typeof o.scaleMm === 'number' && Number.isFinite(o.scaleMm)
          ? Math.min(200, Math.max(1, o.scaleMm))
          : DEFAULT_OBJECT_SCALE_MM,
      positionId: positions.some((q) => q.id === o.positionId) ? (o.positionId as string) : positions[0].id,
    })
  }
  return Array.isArray(rec.lampMm) ? { version: SCENE_VERSION, positions, objects, lampMm: asVec3(rec.lampMm, LAMP_HELD_MM) } : { version: SCENE_VERSION, positions, objects }
}

export function saveLastScene(scene: HoloScene): void {
  try {
    localStorage.setItem(LAST_SCENE_STORAGE_KEY, JSON.stringify(scene))
  } catch {
    // Storage quota or restricted context; ignore silently.
  }
}

export function loadLastScene(): HoloScene | null {
  try {
    const raw = localStorage.getItem(LAST_SCENE_STORAGE_KEY)
    if (!raw) return null
    return validateScene(JSON.parse(raw))
  } catch {
    return null
  }
}
