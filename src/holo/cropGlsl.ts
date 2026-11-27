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
`

const SIDE_INDEX: Record<CropSide, number> = { both: 0, up: 1, down: 2 }

/** The `crop`, `side` and `axis` uniforms' values for `pos`; none keeps everything. */
export function cropUniforms(pos: HoloPosition | undefined): { crop: number; side: number; axis: number } {
  if (!pos) return { crop: Math.PI / 2, side: 0, axis: 0 }
  return { crop: (pos.cropDeg * Math.PI) / 180, side: SIDE_INDEX[pos.cropSide], axis: (pos.cropOffsetDeg * Math.PI) / 180 }
}
