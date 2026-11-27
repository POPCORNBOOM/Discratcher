// A position's crop in the 3D editor's shaders, as `flat::kept` decides it for the burn: within
// `crop` of the axis at the kept ends. `p` is in the position's own frame, y up, 12 o'clock +y.
import type { CropSide, HoloPosition } from './scene'

export const CROP_GLSL = `
bool inCrop(vec2 p, float crop, float side, float axis) {
  if (side < 0.5 && crop >= 1.5707) return true;
  vec2 u = vec2(sin(axis), cos(axis));
  float a = atan(abs(p.x * u.y - p.y * u.x), dot(p, u));
  float c = min(crop, 1.5708);
  bool top = a <= c;
  bool bottom = a >= 3.14159265 - c;
  return side < 0.5 ? (top || bottom) : side < 1.5 ? top : bottom;
}
float rayDistance(vec2 p, float angle) {
  vec2 d = vec2(sin(angle), cos(angle));
  return dot(p, d) > 0.0 ? abs(p.x * d.y - p.y * d.x) : length(p);
}
// How far p is from the crop's straight edges, in p's units; far away when nothing is cropped.
float cropEdge(vec2 p, float crop, float side, float axis) {
  if (side < 0.5 && crop >= 1.5707) return 1e9;
  float c = min(crop, 1.5708);
  float e = 1e9;
  if (side < 0.5 || side < 1.5) e = min(e, min(rayDistance(p, axis - c), rayDistance(p, axis + c)));
  if (side < 0.5 || side > 1.5) e = min(e, min(rayDistance(p, axis + 3.14159265 - c), rayDistance(p, axis + 3.14159265 + c)));
  return e;
}
`

const SIDE_INDEX: Record<CropSide, number> = { both: 0, up: 1, down: 2 }

/** The `crop`, `side` and `axis` uniforms' values for `pos`; none keeps everything. */
export function cropUniforms(pos: HoloPosition | undefined): { crop: number; side: number; axis: number } {
  if (!pos) return { crop: Math.PI / 2, side: 0, axis: 0 }
  return { crop: (pos.cropDeg * Math.PI) / 180, side: SIDE_INDEX[pos.cropSide], axis: (pos.cropOffsetDeg * Math.PI) / 180 }
}

/** Whether disc point `(x, y)` (position frame, +y up) lies inside `pos`'s kept sector (`inCrop`). */
export function inPositionCrop(x: number, y: number, pos: HoloPosition | undefined): boolean {
  if (!pos || (pos.cropSide === 'both' && pos.cropDeg >= 90)) return true
  const axis = (pos.cropOffsetDeg * Math.PI) / 180
  const ux = Math.sin(axis)
  const uy = Math.cos(axis)
  const a = Math.atan2(Math.abs(x * uy - y * ux), x * ux + y * uy)
  const c = Math.min((pos.cropDeg * Math.PI) / 180, Math.PI / 2)
  const top = a <= c
  const bottom = a >= Math.PI - c
  return pos.cropSide === 'both' ? top || bottom : pos.cropSide === 'up' ? top : bottom
}
