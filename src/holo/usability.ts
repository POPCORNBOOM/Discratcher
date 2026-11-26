import { DoubleSide, ShaderMaterial, Vector3 } from 'three'
import { DEFAULT_DISTANCE_MM, DEFAULT_HEAD_SPAN_MM, LAMP_HELD_MM } from './constants'

/**
 * Colours the disc by the depth each place can show to a head moving from side to side: the
 * more `g_t` (the part of light and view direction along the track, as in `holo::scene::g_at`)
 * changes, the more violet. Where it passes through zero nothing lights up, and it is hatched.
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
    },
    vertexShader: `
      varying vec2 at;
      void main() { at = position.xy; gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0); }
    `,
    fragmentShader: `
      varying vec2 at;
      uniform vec3 lamp;
      uniform float distance, span;
      void main() {
        vec3 p = vec3(at, 0.0);
        vec2 r = normalize(at);
        float lo = 1e9, hi = -1e9;
        for (int i = 0; i < 9; i++) {
          vec3 eye = vec3(span * (float(i) / 4.0 - 1.0), 0.0, distance);
          vec3 g = normalize(lamp - p) + normalize(eye - p);
          float gt = -g.x * r.y + g.y * r.x;
          lo = min(lo, gt);
          hi = max(hi, gt);
        }
        bool dead = lo * hi <= 0.0 || max(abs(lo), abs(hi)) < 0.08;
        float line = fract((at.x + at.y) / 1.41421 / 2.4);
        float strength = smoothstep(0.056, 0.24, hi - lo);
        gl_FragColor = dead ? vec4(vec3(0.92), line < 0.22 ? 0.27 : 0.0) : vec4(0.565, 0.27, 0.976, strength * 0.47);
      }
    `,
  })
}
