/**
 * Single source of truth for hologram viewing geometry and physical disc dimensions.
 * Shared across the 3D editor, usability overlay, preview renderer, and post-burn viewing guide.
 */

export type Vec3 = [number, number, number]

/** Default fixed phone-flashlight offset from disc centre (scene space: +x right, +y up, +z viewer). */
export const LAMP_RIGHT_CM = 15
export const LAMP_UP_CM = 40
export const LAMP_RIGHT_MM = LAMP_RIGHT_CM * 10
export const LAMP_UP_MM = LAMP_UP_CM * 10

export const LAMP_HELD_MM: Vec3 = [LAMP_RIGHT_MM, 0, LAMP_UP_MM]
export const LAMP_CEILING_MM: Vec3 = [0, 200, 1200]

/** Transform animation viewing geometry (Winner A): lamp next to the eye, one-sided left-right disc tilt. */
export const TRANSFORM_TILT_START_DEG = 6.5
export const TRANSFORM_TILT_END_DEG = 22.5
export const TRANSFORM_LAMP_UP_CM = 40
export const TRANSFORM_LAMP_MM: Vec3 = [0, 0, TRANSFORM_LAMP_UP_CM * 10]

export function transformTiltDeg(t: number): number {
  return TRANSFORM_TILT_START_DEG + (TRANSFORM_TILT_END_DEG - TRANSFORM_TILT_START_DEG) * t
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
export const DEFAULT_OBJECT_DEPTH_MM = 8

/** Sentinel path resolved by both Rust `model3d` and the browser mock to the bundled star sample. */
export const BUILTIN_SAMPLE_PATH = 'builtin:star'

/** The lamp's row in the object list: selectable and movable, never deleted. */
export const LAMP_ID = 'lamp'
/** How far the lamp can be moved from the disc centre (mm): sideways, and height. */
export const LAMP_REACH_MM = 400
export const LAMP_MIN_UP_MM = 100
export const LAMP_MAX_UP_MM = 800
