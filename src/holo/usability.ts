import { DoubleSide, ShaderMaterial, Vector3 } from 'three'
import { DEFAULT_DISTANCE_MM, DEFAULT_HEAD_SPAN_MM, LAMP_HELD_MM } from './constants'
import { CROP_GLSL } from './cropGlsl'
import { DEFAULT_CROP_DEG } from './scene'

/** What the zones show: depth for 3D models, colour for colour pictures. */
export type ZoneKind = 'depth' | 'colour'

/**
 * Colours the disc by how well each place suits what is selected, more violet the better; where it
 * cannot work, or lies outside the current position's crop (`cropGlsl`, in its frame turned by
 * `phi`), it is hatched. `g_t` is the part of light and view direction along the track, as in
 * `holo::scene::g_at`.
 *
 * Depth: the more `g_t` changes as the head moves from side to side, the deeper a model shows.
 * Colour: the two eyes must see the same `g_t` (`turntable::EYE_HALF_MM` either side), and `|g_t|`
 * must let the codebook's 0.469 to 1.25 per um reach the whole of 400 to 700 nm, as the colour
 * study found.
 */
export function zonesMaterial(): ShaderMaterial {
  return new ShaderMaterial({
    transparent: true,
    depthWrite: false,
    side: DoubleSide,
    uniforms: {
      lamp: { value: new Vector3(...LAMP_HELD_MM) },
      distance: { value: DEFAULT_DISTANCE_MM },
      span: { value: DEFAULT_HEAD_SPAN_MM },
      phi: { value: 0 },
      crop: { value: (DEFAULT_CROP_DEG * Math.PI) / 180 },
      kind: { value: 0 },
      side: { value: 0 },
      axis: { value: 0 },
    },
    vertexShader: `
      varying vec2 at;
      void main() { at = position.xy; gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0); }
    `,
    fragmentShader: `
      varying vec2 at;
      uniform vec3 lamp;
      uniform float distance, span, phi, crop, side, axis, kind;
${CROP_GLSL}
      float gtAt(vec3 p, vec2 r, float eyeX) {
        vec3 g = normalize(lamp - p) + normalize(vec3(eyeX, 0.0, distance) - p);
        return -g.x * r.y + g.y * r.x;
      }
      void main() {
        vec3 p = vec3(at, 0.0);
        vec2 r = normalize(at);
        bool dead;
        float strength;
        if (kind < 0.5) {
          float lo = 1e9, hi = -1e9;
          for (int i = 0; i < 9; i++) {
            float gt = gtAt(p, r, span * (float(i) / 4.0 - 1.0));
            lo = min(lo, gt);
            hi = max(hi, gt);
          }
          dead = lo * hi <= 0.0 || max(abs(lo), abs(hi)) < 0.08;
          strength = smoothstep(0.056, 0.24, hi - lo);
        } else {
          float l = gtAt(p, r, -32.0), rr = gtAt(p, r, 32.0);
          float m = 0.5 * (abs(l) + abs(rr));
          float agree = 1.0 - smoothstep(0.02, 0.09, abs(l - rr));
          // Share of 400 to 700 nm that wavelengths m / 1.25 to m / 0.469 um cover.
          float reach = clamp((min(m / 0.469, 0.7) - max(m / 1.25, 0.4)) / 0.3, 0.0, 1.0);
          strength = l * rr <= 0.0 ? 0.0 : agree * reach;
          dead = strength < 0.12;
        }
        float c = cos(phi), s = sin(phi);
        vec2 q = vec2(at.x * c - at.y * s, at.x * s + at.y * c);
        bool cropped = !inCrop(q, crop, side, axis);
        float line = fract((at.x + at.y) / 1.41421 / 2.4);
        gl_FragColor = (dead || cropped) ? vec4(vec3(0.92), line < 0.22 ? 0.27 : 0.0) : vec4(0.565, 0.27, 0.976, strength * 0.47);
      }
    `,
  })
}
