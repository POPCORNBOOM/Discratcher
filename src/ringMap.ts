// Pictures wrapped round the written ring: the disc unrolled into a band, as
// `artwork::ring_unroll` does it. Positions are shares of the diameter, y down, with the rim at 0.5.
import type { Layer } from './types'

/** The written ring, as shares of the diameter: a DVD's data area, 24.173 to 57.879 mm. */
export const RING_INNER = 24.173 / (2 * 57.879)
export const RING_OUTER = 0.5
export const MAX_RING_TURNS = 12
/** The middle circle's length round, and the band's width. */
export const RING_ROUND = Math.PI * (RING_INNER + RING_OUTER)
export const RING_BAND = RING_OUTER - RING_INNER

/** `artwork::ring_unroll` in GLSL: `ringUnroll(p, turns)` is the unrolled point and 1, or 0 off the spiral. */
export const RING_GLSL = `
vec3 ringUnroll(vec2 p, float turns) {
  float n = clamp(floor(turns + 0.5), 1.0, ${MAX_RING_TURNS}.0);
  float roundLen = ${RING_ROUND.toFixed(9)};
  float band = ${RING_BAND.toFixed(9)} / n;
  float a = fract(atan(p.x, -p.y) / 6.283185307 + 0.5);
  float depth = (${RING_OUTER.toFixed(9)} - length(p)) / band;
  if (n < 1.5) return vec3((a - 0.5) * roundLen, (depth - 0.5) * band, 1.0);
  float u = depth - a;
  float lap = floor(u);
  float along = lap + a;
  return vec3((along - n * 0.5) * roundLen, (u - lap - 0.5) * band, step(0.0, along) * (1.0 - step(n, along)));
}
`

/**
 * Wrapped round the ring, `l` is centred and as tall as one turn's band, over as many turns as
 * keep its shape best.
 */
export function ringFit(l: Pick<Layer, 'width' | 'height'>): Pick<Layer, 'x' | 'y' | 'size' | 'rotationDeg' | 'stretchX' | 'stretchY' | 'ringTurns'> {
  const aspect = Math.max(1, l.width) / Math.max(1, l.height)
  const ringTurns = Math.min(MAX_RING_TURNS, Math.max(1, Math.round(Math.sqrt((aspect * RING_BAND) / RING_ROUND))))
  const long = Math.max(l.width, l.height, 1)
  return { x: 0, y: 0, rotationDeg: 0, stretchX: 1, stretchY: 1, ringTurns, size: ((RING_BAND / ringTurns) * long) / Math.max(1, l.height) }
}
