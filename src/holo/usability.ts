import { DoubleSide, ShaderMaterial, Vector3, type Texture } from 'three'
import { DEFAULT_DISTANCE_MM, DEFAULT_HEAD_SPAN_MM, LAMP_HELD_MM } from './constants'

/** What the zones show: depth for models, colour for colour pictures, none for dead-only, crop for rim/sector only. */
export type ZoneKind = 'depth' | 'colour' | 'none' | 'crop'

export const ZONE_KIND_VALUE: Record<ZoneKind, number> = { depth: 0, colour: 1, none: 2, crop: 3 }

/**
 * Colours the disc by how well each place suits what is selected, more violet the better; where the
 * light cannot work it is hatched. When nothing is selected (`none`), only the lamp's glare band is
 * hatched: where some eye in the head span sees the track part of the light flip sign or come within
 * 0.15 of it, the rainbow streak of the grooves covers everything. Depth and colour both read 0
 * there, so the hatching only grows when something is selected. When a
 * grey carved picture is selected (`crop`), only the crop border is shown. The current position's
 * crop (`cropMap`, in its frame turned by `phi`) is marked by a violet outline while the optical
 * zones remain visible across the whole ring. `g_t` and `g_r` are the parts of light and view
 * direction along the track and radius, as in `holo::scene::g_at`.
 */
export function zonesMaterial(initialCropMap: Texture | null = null): ShaderMaterial {
  return new ShaderMaterial({
    transparent: true,
    depthWrite: false,
    side: DoubleSide,
    uniforms: {
      lamp: { value: new Vector3(...LAMP_HELD_MM) },
      distance: { value: DEFAULT_DISTANCE_MM },
      span: { value: DEFAULT_HEAD_SPAN_MM },
      phi: { value: 0 },
      cropMap: { value: initialCropMap },
      hasCrop: { value: 0 },
      kind: { value: ZONE_KIND_VALUE.none },
      inner: { value: 0 },
      outer: { value: 0 },
      hatchMm: { value: 2.4 },
    },
    vertexShader: `
      varying vec2 at;
      void main() { at = position.xy; gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0); }
    `,
    fragmentShader: `
      varying vec2 at;
      uniform sampler2D cropMap;
      uniform vec3 lamp;
      uniform float distance, span, phi, hasCrop, kind, inner, outer, hatchMm;
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
      bool glareAt(vec3 p, vec2 r) {
        float lo = 1e9, hi = -1e9;
        for (int i = 0; i < 9; i++) {
          float gt = gAt(p, r, span * (float(i) / 4.0 - 1.0)).x;
          lo = min(lo, gt);
          hi = max(hi, gt);
        }
        return lo * hi <= 0.0 || min(abs(lo), abs(hi)) < 0.15;
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
      bool cropEdge(vec2 cuv, float px) {
        float lo = 1.0;
        float hi = 0.0;
        vec2 d = vec2(px, 0.0);
        vec2 e = vec2(0.0, px);
        vec2 dg = vec2(px * 0.7071, px * 0.7071);
        float s0 = texture2D(cropMap, cuv + d).r;
        float s1 = texture2D(cropMap, cuv - d).r;
        float s2 = texture2D(cropMap, cuv + e).r;
        float s3 = texture2D(cropMap, cuv - e).r;
        float s4 = texture2D(cropMap, cuv + dg).r;
        float s5 = texture2D(cropMap, cuv - dg).r;
        float s6 = texture2D(cropMap, cuv + vec2(dg.x, -dg.y)).r;
        float s7 = texture2D(cropMap, cuv + vec2(-dg.x, dg.y)).r;
        lo = min(min(min(s0, s1), min(s2, s3)), min(min(s4, s5), min(s6, s7)));
        hi = max(max(max(s0, s1), max(s2, s3)), max(max(s4, s5), max(s6, s7)));
        return lo < 0.5 && hi >= 0.5;
      }
      void main() {
        float c = cos(phi), s = sin(phi);
        vec2 q = vec2(at.x * c - at.y * s, at.x * s + at.y * c);
        vec4 violet = vec4(0.565, 0.27, 0.976, 1.0);
        if (hasCrop > 0.5) {
          float across = max(2.0 * outer, 1e-4);
          vec2 cuv = vec2(q.x, -q.y) / across + 0.5;
          float m = texture2D(cropMap, cuv).r;
          float rim = min(length(at) - inner, outer - length(at));
          if (cropEdge(cuv, 0.35 / across) || (m >= 0.5 && rim < 0.7)) {
            gl_FragColor = vec4(violet.rgb, 0.9);
            return;
          }
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
          dead = glareAt(p, r);
        }
        if (kind > 1.5 && !dead) discard;
        float line = fract((at.x + at.y) / 1.41421 / hatchMm);
        gl_FragColor = dead ? vec4(vec3(0.92), line < 0.22 ? 0.27 : 0.0) : vec4(violet.rgb, strength * 0.47);
      }
    `,
  })
}
