import { fileName, readLayer } from '../layers'
import type { ArtworkAnalysis, DiscGeometryPreset, Layer } from '../types'
import {
  DEFAULT_DATA_INNER_MM,
  DEFAULT_DATA_OUTER_MM,
  DEFAULT_OBJECT_DEPTH_MM,
  DEFAULT_OBJECT_SCALE_MM,
  LAMP_HELD_MM,
  type Vec3,
} from './constants'
import {
  cloneMotion,
  defaultMotion,
  IDENTITY_MATRIX_STRINGS,
  readMotion,
  VAR_NAME,
  type HoloMotion,
  type MotionVar,
} from './motion'

/** 2: modes moved from positions to models, and positions order their models and pictures. */
export const SCENE_VERSION = 2 as const

/** How a model's views are made: real parallax, or transform animation. */
export type HoloMode = 'real' | 'transform'

export interface HoloInkStyle {
  lines: number
  shading: number
  texture: number
}

export const DEFAULT_INK_STYLE: HoloInkStyle = { lines: 0.85, shading: 0.25, texture: 0.8 }
export const DEFAULT_CROP_DEG = 30
export const DEFAULT_PARALLAX = 2

export function defaultInkStyle(): HoloInkStyle {
  return { ...DEFAULT_INK_STYLE }
}

export interface HoloSceneObject {
  id: string
  path: string
  name: string
  visible: boolean
  positionMm: Vec3
  rotationDeg: Vec3
  /** Size along each of the model's own axes, mm. */
  scaleMm: Vec3
  positionId: string
  style: HoloInkStyle
  motion: HoloMotion
  mode: HoloMode
  /** Parallax boost in real mode. */
  parallax: number
  /** How far the disc tilts either way in transform mode, degrees; 0 takes the default. */
  tiltDeg: number
}

/** Which ends of a position's crop axis are kept: the top, the bottom, or both (`model3d::CropSide`). */
export type CropSide = 'both' | 'up' | 'down'
export const CROP_SIDES: readonly CropSide[] = ['up', 'down', 'both']
/** The crop axis's offset turns at most this far either way, degrees. */
export const MAX_CROP_OFFSET_DEG = 180

export type MaskChannel = 'luma' | 'alpha'
export type CropKind = 'sector' | 'polygon' | 'circle' | 'mask'

export interface HoloCropBase {
  id: string
  name: string
  visible: boolean
}

export interface HoloSectorCrop extends HoloCropBase {
  kind: 'sector'
  /** Half-angle kept either side of the crop axis, degrees. */
  deg: number
  side: CropSide
  offsetDeg: number
}

export interface HoloCircleCrop extends HoloCropBase {
  kind: 'circle'
  /** Centre as a share of the disc diameter (`-0.5..0.5`), y down. */
  x: number
  y: number
  /** Diameter as a share of the disc diameter. */
  size: number
  stretchX: number
  stretchY: number
  rotationDeg: number
}

export interface HoloPolygonCrop extends HoloCropBase {
  kind: 'polygon'
  sides: number
  x: number
  y: number
  size: number
  stretchX: number
  stretchY: number
  rotationDeg: number
}

export interface HoloMaskCrop extends HoloCropBase {
  kind: 'mask'
  path: string
  width: number
  height: number
  x: number
  y: number
  size: number
  stretchX: number
  stretchY: number
  rotationDeg: number
  channel: MaskChannel
  invert: boolean
}

export type HoloCrop = HoloSectorCrop | HoloCircleCrop | HoloPolygonCrop | HoloMaskCrop
export type HoloShapeCrop = HoloCircleCrop | HoloPolygonCrop | HoloMaskCrop

export function makeCropId(): string {
  return `crop-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 7)}`
}

export function makeSectorCrop(deg = DEFAULT_CROP_DEG, side: CropSide = 'both', offsetDeg = 0): HoloSectorCrop {
  return { id: makeCropId(), name: '', visible: true, kind: 'sector', deg, side, offsetDeg }
}

export function makeCircleCrop(): HoloCircleCrop {
  return { id: makeCropId(), name: '', visible: true, kind: 'circle', x: 0, y: -0.35, size: 0.34, stretchX: 1, stretchY: 1, rotationDeg: 0 }
}

export function makePolygonCrop(sides = 6): HoloPolygonCrop {
  return { id: makeCropId(), name: '', visible: true, kind: 'polygon', sides: Math.min(12, Math.max(3, Math.round(sides))), x: 0, y: -0.35, size: 0.36, stretchX: 1, stretchY: 1, rotationDeg: 0 }
}

export function makeMaskCrop(file: string, a: ArtworkAnalysis): HoloMaskCrop {
  return {
    id: makeCropId(),
    name: fileName(file),
    visible: true,
    kind: 'mask',
    path: file,
    width: Math.max(1, a.width),
    height: Math.max(1, a.height),
    x: 0,
    y: 0,
    size: 1,
    stretchX: 1,
    stretchY: 1,
    rotationDeg: 0,
    channel: a.hasAlpha ? 'alpha' : 'luma',
    invert: false,
  }
}

export function cloneCrop(crop: HoloCrop, freshId = false): HoloCrop {
  return { ...crop, ...(freshId ? { id: makeCropId() } : {}) }
}

/** One way the disc is turned to be looked at, with its own models, pictures, and crops. */
export interface HoloPosition {
  id: string
  name: string
  /** Which angle of the disc is at the top, clockwise in degrees. */
  phiDeg: number
  /** Crop shapes active on this position; empty keeps the whole ring, multiple unite. */
  crops: HoloCrop[]
  /** How hard every view is held white rather than sharper and in colour; absent takes each model's mode's default. */
  neutral?: number
  /** Its models' and pictures' ids, top first: where two want the same disc, the higher wins. */
  order: string[]
}

/**
 * A picture placed as on the carve page in its position's own frame: upright with the position at
 * the top (`disc_core::holo::model3d::SceneImage`).
 */
/** `chroma`: for a colour picture, how hard its hues are matched; absent takes `DEFAULT_PICTURE_CHROMA`. */
export type HoloImage = Layer & { positionId: string; colour: boolean; chroma?: number }

export interface HoloScene {
  version: typeof SCENE_VERSION
  positions: HoloPosition[]
  objects: HoloSceneObject[]
  /** The positions' pictures. */
  images: HoloImage[]
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
  const spots: [number, number][] = [
    [0, rMid],
    [0, -rMid],
    [-16, rMid - 4],
    [16, rMid - 4],
  ]
  const [x, y] = spots[existingCount % spots.length]
  return {
    positionMm: [round1(x), round1(y), DEFAULT_OBJECT_DEPTH_MM],
    rotationDeg: [0, 0, 0],
    scaleMm: [DEFAULT_OBJECT_SCALE_MM, DEFAULT_OBJECT_SCALE_MM, DEFAULT_OBJECT_SCALE_MM],
  }
}

export function cloneScene(scene: HoloScene): HoloScene {
  return {
    version: SCENE_VERSION,
    ...(scene.lampMm ? { lampMm: [...scene.lampMm] as Vec3 } : {}),
    positions: scene.positions.map((p) => ({ ...p, crops: p.crops.map((c) => cloneCrop(c)), order: [...p.order] })),
    objects: scene.objects.map((o) => ({
      ...o,
      positionMm: [...o.positionMm] as Vec3,
      rotationDeg: [...o.rotationDeg] as Vec3,
      scaleMm: [...o.scaleMm] as Vec3,
      style: { ...o.style },
      motion: cloneMotion(o.motion),
    })),
    images: scene.images.map((i) => ({ ...i })),
  }
}

export function emptyScene(): HoloScene {
  return { version: SCENE_VERSION, positions: [makePosition(0)], objects: [], images: [] }
}

export function makePosition(phiDeg: number, crops: HoloCrop[] = [makeSectorCrop(DEFAULT_CROP_DEG, 'both', 0)]): HoloPosition {
  return { id: makeObjectId(), name: '', phiDeg, crops: crops.map((c) => cloneCrop(c)), order: [] }
}

/** A model or a picture in a position's list. */
export type HoloItem = { kind: 'model'; item: HoloSceneObject } | { kind: 'picture'; item: HoloImage }

/**
 * Position `q`'s models and pictures, top first: as its `order` lists them, then any it leaves
 * out (`turntable::items`).
 */
export function positionItems(scene: HoloScene, q: HoloPosition): HoloItem[] {
  const all: HoloItem[] = [
    ...scene.objects.filter((o) => o.positionId === q.id).map((item) => ({ kind: 'model' as const, item })),
    ...scene.images.filter((i) => i.positionId === q.id).map((item) => ({ kind: 'picture' as const, item })),
  ]
  const listed = q.order.flatMap((id) => all.filter((x) => x.item.id === id))
  return [...new Set([...listed, ...all])]
}

/** Each position's `order` as it stands: gone ids dropped, new ones added at the bottom. */
export function normalizeOrder(scene: HoloScene): void {
  for (const q of scene.positions) {
    const ids = positionItems(scene, q).map((x) => x.item.id)
    if (ids.join() !== q.order.join()) q.order = ids
  }
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

function asStyle(raw: unknown): HoloInkStyle {
  const s = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : null
  const pick = (k: keyof HoloInkStyle) => {
    const v = s?.[k]
    return typeof v === 'number' && Number.isFinite(v) ? Math.min(1, Math.max(0, v)) : DEFAULT_INK_STYLE[k]
  }
  return { lines: pick('lines'), shading: pick('shading'), texture: pick('texture') }
}

function asSamples(raw: unknown): number[][] | null {
  if (!Array.isArray(raw) || raw.length === 0) return null
  const out: number[][] = []
  for (const row of raw) {
    if (!Array.isArray(row) || row.length !== 16) return null
    if (!row.every((v) => typeof v === 'number' && Number.isFinite(v))) return null
    out.push([...(row as number[])])
  }
  return out
}

function asMotion(raw: unknown): HoloMotion {
  if (!raw || typeof raw !== 'object') return defaultMotion()
  const m = raw as Record<string, unknown>
  const preset = typeof m.preset === 'string' ? m.preset : 'none'
  const matrix =
    Array.isArray(m.matrix) && m.matrix.length === 16 && m.matrix.every((v) => typeof v === 'string')
      ? [...(m.matrix as string[])]
      : null
  const vars: MotionVar[] = Array.isArray(m.vars)
    ? (m.vars as unknown[]).flatMap((v) => {
        const x = v as Record<string, unknown> | null
        return x && typeof x.name === 'string' && VAR_NAME.test(x.name) && typeof x.value === 'number' && Number.isFinite(x.value)
          ? [{ name: x.name, value: x.value }]
          : []
      })
    : []
  const read = readMotion(preset, matrix, vars)
  if (read) return read
  // A custom matrix that no longer evaluates keeps what was solved from it.
  const samples = asSamples(m.samples)
  if (preset === 'custom' && samples) return { preset, matrix: matrix ?? [...IDENTITY_MATRIX_STRINGS], vars, samples }
  return defaultMotion()
}

const asMode = (v: unknown): HoloMode => (v === 'transform' ? 'transform' : 'real')
const asParallax = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) ? Math.min(6, Math.max(1, round1(v))) : DEFAULT_PARALLAX)
const asTilt = (v: unknown) => (typeof v === 'number' && Number.isFinite(v) ? Math.min(30, Math.max(0, v)) : 0)

function asCrop(raw: unknown): HoloCrop | null {
  if (!raw || typeof raw !== 'object') return null
  const c = raw as Record<string, unknown>
  const num = (v: unknown, d: number, lo: number, hi: number) =>
    typeof v === 'number' && Number.isFinite(v) ? Math.min(hi, Math.max(lo, v)) : d
  const id = typeof c.id === 'string' && c.id ? c.id : makeCropId()
  const name = typeof c.name === 'string' ? c.name : ''
  const visible = typeof c.visible === 'boolean' ? c.visible : true
  if (c.kind === 'sector') {
    const side: CropSide = CROP_SIDES.includes(c.side as CropSide) ? (c.side as CropSide) : 'both'
    return {
      id,
      name,
      visible,
      kind: 'sector',
      deg: num(c.deg, DEFAULT_CROP_DEG, 5, side === 'both' ? 90 : 180),
      side,
      offsetDeg: num(c.offsetDeg, 0, -MAX_CROP_OFFSET_DEG, MAX_CROP_OFFSET_DEG),
    }
  }
  if (c.kind === 'circle') {
    return {
      id,
      name,
      visible,
      kind: 'circle',
      x: num(c.x, 0, -1, 1),
      y: num(c.y, -0.35, -1, 1),
      size: num(c.size, 0.34, 0.02, 3),
      stretchX: num(c.stretchX, 1, 0.1, 4),
      stretchY: num(c.stretchY, 1, 0.1, 4),
      rotationDeg: num(c.rotationDeg, 0, -180, 180),
    }
  }
  if (c.kind === 'polygon') {
    return {
      id,
      name,
      visible,
      kind: 'polygon',
      sides: Math.round(num(c.sides, 6, 3, 12)),
      x: num(c.x, 0, -1, 1),
      y: num(c.y, -0.35, -1, 1),
      size: num(c.size, 0.36, 0.02, 3),
      stretchX: num(c.stretchX, 1, 0.1, 4),
      stretchY: num(c.stretchY, 1, 0.1, 4),
      rotationDeg: num(c.rotationDeg, 0, -180, 180),
    }
  }
  if (c.kind === 'mask' && typeof c.path === 'string' && c.path.trim()) {
    return {
      id,
      name: name || fileName(c.path),
      visible,
      kind: 'mask',
      path: c.path,
      width: Math.round(num(c.width, 1, 1, 65536)),
      height: Math.round(num(c.height, 1, 1, 65536)),
      x: num(c.x, 0, -1, 1),
      y: num(c.y, 0, -1, 1),
      size: num(c.size, 1, 0.02, 3),
      stretchX: num(c.stretchX, 1, 0.1, 4),
      stretchY: num(c.stretchY, 1, 0.1, 4),
      rotationDeg: num(c.rotationDeg, 0, -180, 180),
      channel: c.channel === 'alpha' ? 'alpha' : 'luma',
      invert: !!c.invert,
    }
  }
  return null
}

/** Parse and validate a versioned scene JSON payload, bringing older versions up to date. */
export function validateScene(raw: unknown): HoloScene {
  if (!raw || typeof raw !== 'object') throw new Error('Invalid scene payload')
  const rec = raw as Record<string, unknown>
  if (rec.version !== 1 && rec.version !== SCENE_VERSION) {
    throw new Error(`Unsupported scene version: ${String(rec.version)}`)
  }
  // Version 1 kept the mode, parallax and tilt on the position; its models take them over.
  const legacy = new Map<string, { mode: HoloMode; parallax: number; tiltDeg: number }>()
  if (!Array.isArray(rec.objects)) throw new Error('Scene objects must be an array')
  const positions: HoloPosition[] = []
  for (const item of Array.isArray(rec.positions) ? rec.positions : []) {
    const q = item as Record<string, unknown> | null
    if (q && typeof q.id === 'string' && q.id && typeof q.phiDeg === 'number' && Number.isFinite(q.phiDeg)) {
      let crops: HoloCrop[]
      if (Array.isArray(q.crops)) {
        crops = q.crops.map(asCrop).filter((c): c is HoloCrop => c !== null)
      } else {
        const cropDeg =
          typeof q.cropDeg === 'number' && Number.isFinite(q.cropDeg)
            ? Math.min(90, Math.max(5, q.cropDeg))
            : DEFAULT_CROP_DEG
        const cropSide = CROP_SIDES.includes(q.cropSide as CropSide) ? (q.cropSide as CropSide) : 'both'
        const cropOffsetDeg =
          typeof q.cropOffsetDeg === 'number' && Number.isFinite(q.cropOffsetDeg)
            ? Math.min(MAX_CROP_OFFSET_DEG, Math.max(-MAX_CROP_OFFSET_DEG, q.cropOffsetDeg))
            : 0
        crops = cropSide === 'both' && cropDeg >= 90 ? [] : [makeSectorCrop(cropDeg, cropSide, cropOffsetDeg)]
      }
      legacy.set(q.id, { mode: asMode(q.mode), parallax: asParallax(q.parallax), tiltDeg: asTilt(q.tiltDeg) })
      positions.push({
        id: q.id,
        name: typeof q.name === 'string' ? q.name : '',
        phiDeg: wrapDeg(q.phiDeg),
        crops,
        ...(typeof q.neutral === 'number' && Number.isFinite(q.neutral) ? { neutral: Math.min(3, Math.max(0, q.neutral)) } : {}),
        order: Array.isArray(q.order) ? q.order.filter((x): x is string => typeof x === 'string') : [],
      })
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
      // One size in older files is the same on all three axes.
      scaleMm: (typeof o.scaleMm === 'number' ? asVec3([o.scaleMm, o.scaleMm, o.scaleMm], [DEFAULT_OBJECT_SCALE_MM, DEFAULT_OBJECT_SCALE_MM, DEFAULT_OBJECT_SCALE_MM]) : asVec3(o.scaleMm, [DEFAULT_OBJECT_SCALE_MM, DEFAULT_OBJECT_SCALE_MM, DEFAULT_OBJECT_SCALE_MM])).map((v) => Math.min(200, Math.max(1, v))) as Vec3,
      positionId: positions.some((q) => q.id === o.positionId) ? (o.positionId as string) : positions[0].id,
      style: asStyle(o.style),
      motion: asMotion(o.motion),
      ...(rec.version === 1
        ? (legacy.get(o.positionId as string) ?? legacy.get(positions[0].id) ?? { mode: 'real', parallax: DEFAULT_PARALLAX, tiltDeg: 0 })
        : { mode: asMode(o.mode), parallax: asParallax(o.parallax), tiltDeg: asTilt(o.tiltDeg) }),
    })
  }
  const images: HoloImage[] = []
  for (const item of Array.isArray(rec.images) ? rec.images : []) {
    const l = readLayer(item)
    const rawImg = item as Record<string, unknown>
    const positionId = rawImg.positionId
    if (l) {
      images.push({
        ...l,
        positionId: positions.some((q) => q.id === positionId) ? (positionId as string) : positions[0].id,
        colour: !!rawImg.colour,
        ...(typeof rawImg.chroma === 'number' && Number.isFinite(rawImg.chroma) ? { chroma: Math.min(20, Math.max(0, rawImg.chroma)) } : {}),
      })
    }
  }
  const scene: HoloScene = { version: SCENE_VERSION, positions, objects, images }
  normalizeOrder(scene)
  return Array.isArray(rec.lampMm) ? { ...scene, lampMm: asVec3(rec.lampMm, LAMP_HELD_MM) } : scene
}
