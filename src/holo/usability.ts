import { DoubleSide, ShaderMaterial, Vector3 } from 'three'
import { DEFAULT_DISTANCE_MM, DEFAULT_HEAD_SPAN_MM, LAMP_HELD_MM } from './constants'
import { CROP_GLSL } from './cropGlsl'
import { DEFAULT_CROP_DEG } from './scene'

/** What the zones show: depth for models, colour for colour pictures, none for dead-only, crop for rim/sector only. */
export type ZoneKind = 'depth' | 'colour' | 'none' | 'crop'

export const ZONE_KIND_VALUE: Record<ZoneKind, number> = { depth: 0, colour: 1, none: 2, crop: 3 }

/**
 * Colours the disc by how well each place suits what is selected, more violet the better; where the
 * light cannot work it is hatched. When nothing is selected (`none`), only the optically unusable
 * area (where neither depth nor colour can work) is hatched and no violet overlay is drawn. When a
 * grey carved picture is selected (`crop`), only the crop border is shown. The current position's
 * crop (`cropGlsl`, in its frame turned by `phi`) is marked by a violet outline while the optical
 * zones remain visible across the whole ring. `g_t` and `g_r` are the parts of light and view
 * direction along the track and radius, as in `holo::scene::g_at`.
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
      kind: { value: ZONE_KIND_VALUE.none },
      inner: { value: 0 },
      outer: { value: 0 },
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
      uniform float distance, span, phi, crop, side, axis, kind, inner, outer;
${CROP_GLSL}
      vec2 gAt(vec3 p, vec2 r, float eyeX) {
        vec3 g = normalize(lamp - p) + normalize(vec3(eyeX, 0.0, distance) - p);
        return vec2(-g.x * r.y + g.y * r.x, dot(g.xy, r));
      }
      float depthAt(vec3 p, vec2 r) {
        float lo = 1e9, hi = -1e9, grMax = 0.0;
        for (int i = 0; i < 9; i++) {
          vec2 g = gAt(p, r, span * (float(i) / 4.0 - 1.0));
          lo = min(lo, g.x);
          hi = max(hi, g.x);
          grMax = max(grMax, abs(g.y));
        }
        float absLo = min(abs(lo), abs(hi)), absHi = max(abs(lo), abs(hi));
        float band = smoothstep(0.15, 0.22, absLo) * (1.0 - smoothstep(0.70, 0.84, absHi));
        float rr = grMax / 0.55;
        return lo * hi <= 0.0 ? 0.0 : smoothstep(0.056, 0.22, hi - lo) * band * exp(-0.5 * rr * rr);
      }
      float colourAt(vec3 p, vec2 r) {
        vec2 gl = gAt(p, r, -32.0), gc = gAt(p, r, 0.0), gr = gAt(p, r, 32.0);
        float lo = min(abs(gl.x), min(abs(gc.x), abs(gr.x)));
        float hi = max(abs(gl.x), max(abs(gc.x), abs(gr.x)));
        float grMax = max(abs(gl.y), max(abs(gc.y), abs(gr.y)));
        float agree = 1.0 - smoothstep(0.02, 0.085, abs(gl.x - gr.x));
        float reach = smoothstep(0.19, 0.25, lo) * (1.0 - smoothstep(0.48, 0.60, hi));
        float effGt = min(1.0, pow(0.33 / max(abs(gc.x), 0.24), 1.5));
        float br = grMax / 0.46;
        return gl.x * gr.x <= 0.0 ? 0.0 : agree * reach * effGt * exp(-0.5 * br * br);
      }
      void main() {
        float c = cos(phi), s = sin(phi);
        vec2 q = vec2(at.x * c - at.y * s, at.x * s + at.y * c);
        vec4 violet = vec4(0.565, 0.27, 0.976, 1.0);
        bool kept = inCrop(q, crop, side, axis);
        float rim = min(length(at) - inner, outer - length(at));
        bool whole = side < 0.5 && crop >= 1.5707;
        if (cropEdge(q, crop, side, axis) < 0.35 || (kept && !whole && rim < 0.7)) {
          gl_FragColor = vec4(violet.rgb, 0.9);
          return;
        }
        if (kind > 2.5) discard;
        vec3 p = vec3(at, 0.0);
        vec2 r = normalize(at);
        bool dead;
        float strength;
        if (kind < 0.5) {
          strength = depthAt(p, r);
          dead = strength < 0.12;
        } else if (kind < 1.5) {
          strength = colourAt(p, r);
          dead = strength < 0.12;
        } else {
          strength = 0.0;
          dead = depthAt(p, r) < 0.12 && colourAt(p, r) < 0.12;
        }
        if (kind > 1.5 && !dead) discard;
        float line = fract((at.x + at.y) / 1.41421 / 2.4);
        gl_FragColor = dead ? vec4(vec3(0.92), line < 0.22 ? 0.27 : 0.0) : vec4(violet.rgb, strength * 0.47);
      }
    `,
  })
}
