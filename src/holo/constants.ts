/**
 * Single source of truth for hologram viewing geometry and physical disc dimensions.
 * Shared across the 3D editor, usability overlay, preview renderer, and post-burn viewing guide.
 */

export type Vec3 = [number, number, number]

/** How far the lamp can be moved from the disc centre (mm): radius, and height above the disc. */
export const LAMP_REACH_MM = 400
export const LAMP_MIN_UP_MM = 100
export const LAMP_MAX_UP_MM = 800

const round1 = (v: number) => Math.round(v * 10) / 10
const wrapAzimuthDeg = (deg: number) => ((Math.round(deg) % 360) + 360) % 360

/** Convert cylindrical lamp coordinates (radius mm, clockwise azimuth from 12 o'clock deg, height mm) to scene Vec3 (mm). */
export function cylindricalToLampMm(radiusMm: number, azimuthDeg: number, upMm: number): Vec3 {
  const r = Math.min(LAMP_REACH_MM, Math.max(0, radiusMm))
  const rad = (azimuthDeg * Math.PI) / 180
  const z = Math.min(LAMP_MAX_UP_MM, Math.max(LAMP_MIN_UP_MM, Math.round(upMm)))
  return [round1(r * Math.sin(rad)), round1(r * Math.cos(rad)), z]
}

/** Convert scene Vec3 (mm) to cylindrical lamp coordinates (preserving `fallbackAzimuthDeg` at r = 0). */
export function lampMmToCylindrical(
  lampMm: Vec3,
  fallbackAzimuthDeg = 90,
): { radiusMm: number; azimuthDeg: number; upMm: number } {
  const [x, y, z] = lampMm
  const radiusMm = Math.min(LAMP_REACH_MM, round1(Math.hypot(x, y)))
  const azimuthDeg = radiusMm > 0.5 ? wrapAzimuthDeg((Math.atan2(x, y) * 180) / Math.PI) : wrapAzimuthDeg(fallbackAzimuthDeg)
  const upMm = Math.min(LAMP_MAX_UP_MM, Math.max(LAMP_MIN_UP_MM, Math.round(z)))
  return { radiusMm, azimuthDeg, upMm }
}

/** Default fixed phone-flashlight offset from disc centre (scene space: +x right, +y up, +z viewer). */
export const LAMP_RIGHT_CM = 15
export const LAMP_UP_CM = 40
export const LAMP_RIGHT_MM = LAMP_RIGHT_CM * 10
export const LAMP_UP_MM = LAMP_UP_CM * 10

export const LAMP_HELD_MM: Vec3 = cylindricalToLampMm(LAMP_RIGHT_MM, 90, LAMP_UP_MM)
export const LAMP_FAR_MM: Vec3 = cylindricalToLampMm(LAMP_RIGHT_MM, 0, LAMP_UP_MM)
export const LAMP_FAR_RIGHT_MM: Vec3 = cylindricalToLampMm(LAMP_RIGHT_MM, 45, LAMP_UP_MM)

/** Transform animation: the disc tilts left and right, from -tilt to +tilt as t runs 0 to 1. */
export const DEFAULT_TRANSFORM_TILT_DEG = 3.5
export const MAX_TRANSFORM_TILT_DEG = 30

/** The disc's tilt, degrees, at animation parameter `t` in [0, 1] (`turntable::transform_tilt_at`). */
export function transformTiltDeg(tiltDeg: number, t: number): number {
  return tiltDeg * (2 * t - 1)
}

export const DEFAULT_DISTANCE_MM = 350
export const DEFAULT_TILT_DEG = 20
/** The disc faces the eye when it is looked at; the editor's camera only starts tilted. */
export const STAGE_TILT_DEG = 0
export const DEFAULT_HEAD_SPAN_MM = 40
export const DEFAULT_VIEWS = 24

/** Physical 120 mm DVD dimensions (mm). */
export const DISC_PHYSICAL_OUTER_MM = 60
export const DISC_HOLE_RADIUS_MM = 7.5
export const DEFAULT_DATA_INNER_MM = 24.173
export const DEFAULT_DATA_OUTER_MM = 57.879

/** Default placement for newly added 3D models (mm). */
export const DEFAULT_OBJECT_SCALE_MM = 24
export const DEFAULT_OBJECT_DEPTH_MM = 0

/** Sentinel path resolved by both Rust `model3d` and the browser mock to the bundled star sample. */
export const BUILTIN_SAMPLE_PATH = 'builtin:star'

/** The lamp's row in the object list: selectable and movable, never deleted. */
export const LAMP_ID = 'lamp'
