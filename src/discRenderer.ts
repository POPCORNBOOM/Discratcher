// A DVD rendered from its optics rather than painted. The rainbow is the grating equation
// d(sinθi + sinθm) = mλ solved per pixel against the radial track pitch, so it moves
// with the light and the tilt the way the real thing does.

const MAX_PIXEL_RATIO = 2

const SPECTRAL_SAMPLES = 18
const NM_LOW = 400
const NM_HIGH = 700

const lobe = (nm: number, mean: number, low: number, high: number) => {
  const d = (nm - mean) / (nm < mean ? low : high)
  return Math.exp(-0.5 * d * d)
}

/** CIE 1931 observer (Wyman, Sloan and Shirley's fit), converted to linear sRGB. */
function observer(nm: number) {
  const x = 1.056 * lobe(nm, 599.8, 37.9, 31) + 0.362 * lobe(nm, 442, 16, 26.7) - 0.065 * lobe(nm, 501.1, 20.4, 26.2)
  const y = 0.821 * lobe(nm, 568.8, 46.9, 40.5) + 0.286 * lobe(nm, 530.9, 16.3, 31.1)
  const z = 1.217 * lobe(nm, 437, 11.8, 36) + 0.681 * lobe(nm, 459, 26, 13.8)
  return [3.2406 * x - 1.5372 * y - 0.4986 * z, -0.9689 * x + 1.8758 * y + 0.0415 * z, 0.0557 * x - 0.204 * y + 1.057 * z]
}

const glslFloat = (v: number) => v.toFixed(5)
const wavelengths = Array.from({ length: SPECTRAL_SAMPLES }, (_, i) => NM_LOW + ((NM_HIGH - NM_LOW) * i) / (SPECTRAL_SAMPLES - 1))
const SPECTRUM_TABLE = `
const int SPECTRAL = ${SPECTRAL_SAMPLES};
const float LAMBDA_UM[SPECTRAL] = float[](${wavelengths.map((nm) => glslFloat(nm / 1000)).join(', ')});
const vec3 OBSERVER[SPECTRAL] = vec3[](${wavelengths.map((nm) => `vec3(${observer(nm).map(glslFloat).join(', ')})`).join(', ')});
`

const VERTEX = `#version 300 es
void main() {
  vec2 p = vec2((gl_VertexID << 1) & 2, gl_VertexID & 2);
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`

const FRAGMENT = `#version 300 es
precision highp float;
uniform vec2 uRes;
uniform mat3 uRot;
uniform vec3 uCenter;
uniform vec2 uPointer;
uniform float uSpin;
uniform float uTime;
uniform float uScale;
uniform vec3 uWin;
uniform float uPitch;
uniform float uSeed;
uniform float uDim;
uniform float uFx;
uniform float uEtchMix;
uniform float uArtMix;
uniform vec4 uArtGeo;
uniform vec2 uArtShift;
uniform vec3 uArtTone;
uniform float uCodeLight;
uniform vec3 uDye;
uniform sampler2D uEtchA;
uniform sampler2D uEtchB;
uniform sampler2D uArt;
out vec4 fragColor;

const float PI = 3.14159265;
const float FOCAL = 3.1;
const float DISC_MM = 60.0;
${SPECTRUM_TABLE}

float hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
float noise1(float x) {
  float i = floor(x), f = fract(x);
  f = f * f * (3.0 - 2.0 * f);
  return mix(hash(vec2(i, 1.7)), hash(vec2(i + 1.0, 1.7)), f);
}
float noise2(vec2 p) {
  vec2 i = floor(p), f = fract(p);
  f = f * f * (3.0 - 2.0 * f);
  return mix(mix(hash(i), hash(i + vec2(1, 0)), f.x), mix(hash(i + vec2(0, 1)), hash(i + vec2(1, 1)), f.x), f.y);
}
float bump(float x) { return exp(-x * x); }

float softbox(vec3 r, vec3 axis, vec3 u, vec3 v, vec2 size, float blur) {
  float d = dot(r, axis);
  if (d <= 0.0) return 0.0;
  vec3 q = r / d;
  vec2 s = abs(vec2(dot(q, u), dot(q, v)));
  return (1.0 - smoothstep(size.x, size.x + blur, s.x)) * (1.0 - smoothstep(size.y, size.y + blur, s.y));
}

// The room the disc mirrors: dark, with one long strip and a faint cool bounce. A mirror
// only reads as metal when most of what it reflects is black.
vec3 room(vec3 r, float blur, vec3 pointerDir) {
  vec3 c = mix(vec3(0.004, 0.004, 0.006), vec3(0.03, 0.032, 0.042), smoothstep(-0.6, 1.0, r.y));
  c += vec3(1.0, 0.98, 0.95) * 3.2 * softbox(r, normalize(vec3(-0.5, 0.74, 0.46)), normalize(vec3(0.83, 0.56, 0.0)), normalize(vec3(-0.26, 0.38, -0.89)), vec2(0.8, 0.025), blur);
  c += vec3(0.6, 0.72, 1.0) * 0.5 * softbox(r, normalize(vec3(0.75, -0.25, 0.6)), normalize(vec3(0.3, 0.95, 0.0)), normalize(vec3(-0.6, 0.1, 0.8)), vec2(0.5, 0.05), blur);
  c += vec3(1.0, 0.96, 0.98) * 2.5 * pow(max(dot(r, pointerDir), 0.0), mix(2400.0, 200.0, clamp(blur * 6.0, 0.0, 1.0)));
  return c;
}

// The grating equation, summed over wavelength. Tracks sit a pitch d apart, so light of
// wavelength λ leaves in order m only where (L + V)·radial = mλ/d. Each wavelength lands
// on its own radius, and that separation is the rainbow. The source's size blurs it.
vec3 diffraction(vec3 lightDir, vec3 view, vec3 radial, vec3 along, float sourceSize, float spread) {
  vec3 h = lightDir + view;
  float sideways = dot(h, along);
  float envelope = exp(-sideways * sideways / (spread * spread));
  if (envelope < 0.002) return vec3(0.0);
  float across = abs(dot(h, radial));
  vec3 c = vec3(0.0);
  for (int i = 0; i < SPECTRAL; i++) {
    for (int m = 1; m <= 2; m++) {
      float order = float(m);
      float miss = (across - order * LAMBDA_UM[i] / uPitch) / sourceSize;
      c += OBSERVER[i] * exp(-miss * miss) / (order * order);
    }
  }
  return max(c, vec3(0.0)) * envelope / float(SPECTRAL);
}

vec3 backdrop(vec2 p) {
  vec2 q = p - uCenter.xy - vec2(0.05, 0.3);
  float pool = exp(-dot(q, q) * 0.9);
  vec3 c = mix(vec3(0.010, 0.010, 0.013), vec3(0.042, 0.036, 0.066), pool);
  return c;
}

// What the carver writes under a point, as grey and coverage; mirrors trace() in
// preview.rs. xy is in disc units with y up. uArtGeo holds the recorded band's inner and
// outer radius, the rotation and the scale; uArtShift the offset as a share of the
// diameter; uArtTone the contrast gain, invert and the guard's floor.
vec2 carvedAt(vec2 xy) {
  float r = length(xy) * DISC_MM;
  vec2 p = vec2(xy.x, -xy.y) * DISC_MM / (2.0 * uArtGeo.y);
  float c = cos(uArtGeo.z), s = sin(uArtGeo.z);
  vec2 q = vec2(c * p.x + s * p.y, -s * p.x + c * p.y);
  vec2 sh = vec2(c * uArtShift.x + s * uArtShift.y, -s * uArtShift.x + c * uArtShift.y);
  vec2 uv = (q - sh) / uArtGeo.w + 0.5;
  // Sampled before any branch, so the mipmap level is sound at every edge.
  vec2 t = texture(uArt, uv).rg;
  float y = clamp((t.r * 255.0 - 128.0) * uArtTone.x + 128.0, 0.0, 255.0) / 255.0;
  y = mix(y, 1.0 - y, uArtTone.y);
  // Outside the picture, and in its letterbox, the disc stays light.
  float inside = step(0.0, uv.x) * step(0.0, uv.y) * step(uv.x, 1.0) * step(uv.y, 1.0);
  float grey = max(mix(1.0, y, t.g * inside), uArtTone.z);
  // Unrecorded mirror band and lead areas read as the bare disc.
  float band = step(uArtGeo.x, r) * step(r, uArtGeo.y);
  return vec2(mix(232.0 / 255.0, grey, band), step(15.0, r) * step(r, 59.0));
}

vec3 aces(vec3 x) { return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0); }

void main() {
  // uWin frames a window of the scene (centre x, y and half-size) so a lens can re-render it up close.
  vec2 p = uWin.xy + (gl_FragCoord.xy - 0.5 * uRes) / uRes.y * 2.0 * uWin.z;
  vec3 ro = vec3(0.0, 0.0, FOCAL);
  vec3 rd = normalize(vec3(p, -FOCAL));
  vec3 n = uRot * vec3(0.0, 0.0, 1.0);
  float t = dot(uCenter - ro, n) / dot(rd, n);
  vec3 hit = ro + t * rd;
  vec3 local = transpose(uRot) * (hit - uCenter) / uScale;
  float rr = length(local.xy) * DISC_MM;
  float aa = max(fwidth(rr), 0.02);
  vec2 shownArt = carvedAt(local.xy);

  // Shadow thrown on the wall behind, offset away from the key light.
  vec3 wallHit = ro + rd * ((uCenter.z - 0.75 - ro.z) / rd.z);
  vec3 wall = transpose(uRot) * (wallHit - uCenter - vec3(0.2, -0.26, -0.75)) / uScale;
  float shadow = 1.0 - 0.8 * smoothstep(1.45, 0.72, length(wall.xy));
  vec3 color = backdrop(p) * shadow;

  float solid = smoothstep(7.5 - aa, 7.5 + aa, rr) * (1.0 - smoothstep(60.0 - aa, 60.0 + aa, rr));
  if (solid > 0.0 && t > 0.0) {
    vec3 radial = uRot * vec3(local.xy / max(length(local.xy), 1e-5), 0.0);
    vec3 along = cross(n, radial);
    vec3 view = -rd;

    // Moulded edges: each ring tilts the surface for a fraction of a millimetre, which
    // is what draws the thin bright lines a real disc has.
    float slope = 0.0;
    slope += -0.55 * bump((rr - 7.9) / 0.5);
    slope += 0.30 * bump((rr - 16.6) / 0.35) - 0.30 * bump((rr - 17.3) / 0.35);
    slope += 0.10 * bump((rr - 18.0) / 0.12);
    slope += 0.07 * bump((rr - 22.6) / 0.14);
    slope += 0.60 * bump((rr - 59.6) / 0.42);
    // Moulding stress: discs are never optically flat.
    float angle = atan(local.y, local.x);
    slope += 0.0012 * sin(rr * 0.23 + 1.3 * sin(angle * 2.0 + 0.6));
    vec3 normal = normalize(n + radial * slope + along * 0.0006 * sin(angle * 3.0 + rr * 0.11));

    float data = smoothstep(22.7, 23.0, rr) * (1.0 - smoothstep(58.2, 58.5, rr));
    float mirror = smoothstep(18.0, 18.1, rr) * (1.0 - smoothstep(22.7, 23.0, rr));
    float metal = data + mirror;

    vec3 pointer = normalize(vec3(uPointer * vec2(2.4, 1.8), 1.5) - hit);
    vec3 r = reflect(rd, normal);

    // Aluminium under a dye layer: a true mirror, so the sharp room comes straight through,
    // tinted twice by the dye. The tracks add only a faint streak across themselves.
    vec3 mirrorRoom = room(r, 0.004, pointer);
    vec3 streak = vec3(0.0);
    float weight = 0.0;
    for (int i = -4; i <= 4; i++) {
      float k = float(i) / 4.0;
      float w = exp(-k * k * 3.0);
      streak += room(normalize(r + radial * k * 0.16), 0.03, pointer) * w;
      weight += w;
    }
    streak /= weight;

    // Written data dims in bands where the recording speed changed zones.
    float zones = 0.9 + 0.1 * noise1(rr * 0.62 + uSeed * 91.0) * (0.7 + 0.3 * noise1(rr * 3.1 + uSeed * 37.0));
    vec3 dyeTwice = uDye * uDye;

    vec3 rainbow = diffraction(normalize(vec3(-3.4, 2.4, 1.5) - hit), view, radial, along, 0.05, 0.13);
    rainbow += 0.8 * diffraction(normalize(vec3(3.6, 1.8, 1.4) - hit), view, radial, along, 0.05, 0.12);
    rainbow += 0.6 * diffraction(normalize(vec3(-1.0, -3.6, 1.4) - hit), view, radial, along, 0.05, 0.11);
    rainbow += 1.0 * diffraction(pointer, view, radial, along, 0.04, 0.05);

    vec3 metalColor = mix(vec3(0.88, 0.89, 0.91) * (0.8 * mirrorRoom + 0.2 * streak), dyeTwice * zones * (0.86 * mirrorRoom + 0.14 * streak * 0.4), data);
    metalColor += rainbow * data * zones * 6.0 * uFx;
    metalColor = mix(metalColor, vec3(0.02, 0.02, 0.024) * mix(vec3(1.0), uDye, data), (1.0 - uFx) * metal);

    // The mirror band holds the stamper's etched code.
    vec2 spun = mat2(cos(uSpin), -sin(uSpin), sin(uSpin), cos(uSpin)) * local.xy;
    float etch = mix(texture(uEtchA, spun * 0.5 + 0.5).r, texture(uEtchB, spun * 0.5 + 0.5).r, uEtchMix);
    metalColor *= 1.0 - 0.72 * etch * mirror * (1.0 - 0.75 * uCodeLight);
    // When the code field has focus the characters are struck by a low warm key light:
    // raised letters catch it, and a soft ring says where to look.
    metalColor += vec3(1.0, 0.94, 0.82) * etch * mirror * uCodeLight * 0.55;
    metalColor += vec3(0.10, 0.10, 0.12) * bump((rr - 20.6) / 1.8) * uCodeLight;

    // A carved image is the dark and light of the data layer, so it sits under the
    // rainbow and the lacquer. The image does not spin: a disc being composed is at rest.
    float carved = data * shownArt.y * uArtMix;
    float ink = pow(shownArt.x, 2.0);
    metalColor = mix(metalColor, metalColor * (0.2 + ink) + ink * 0.02, carved);

    // Clear hub: mostly what is behind it, tinted by the polycarbonate.
    vec3 clearHub = color * vec3(0.80, 0.82, 0.86) + vec3(0.012, 0.013, 0.016);
    clearHub += room(normalize(r + radial * 0.05), 0.10, pointer) * 0.045;

    vec3 surface = mix(clearHub, metalColor, metal);

    // Outer lip beyond the recording layer is clear too.
    float lip = smoothstep(58.5, 58.8, rr);
    surface = mix(surface, clearHub * 1.2, lip);

    // Lacquer: dust and hairlines only show where light catches them.
    vec2 cell = spun * 70.0;
    float dust = step(0.9992, hash(floor(cell * 6.0))) * noise2(cell * 2.0);
    float hairline = smoothstep(0.99, 1.0, noise2(vec2(atan(spun.y, spun.x) * 7.0, length(spun) * 180.0)));

    // Polycarbonate coat: a sharp mirror over the blurred metal, the source of the depth.
    float fresnel = 0.045 + 0.955 * pow(1.0 - max(dot(normal, view), 0.0), 5.0);
    vec3 coat = room(r, 0.012, pointer);
    surface += coat * fresnel * (1.0 + hairline + 2.0 * dust) * uFx;

    color = mix(color, surface * uDim, solid);
  }

  float plain = uArtMix * (1.0 - uFx) * shownArt.y * solid * smoothstep(22.7, 23.0, rr) * (1.0 - smoothstep(58.2, 58.5, rr));

  float vignette = 1.0 - 0.34 * smoothstep(0.7, 2.3, length(p * vec2(0.8, 1.0)));
  color = aces(color * vignette * 1.05);
  color = pow(color, vec3(1.0 / 2.2));
  // With the lights off the preview must show the image's own grey levels, untouched.
  color = mix(color, shownArt.xxx, plain);
  color += (hash(gl_FragCoord.xy + fract(uTime)) - 0.5) / 255.0;
  fragColor = vec4(color, 1.0);
}`

/** The code real discs carry round the hub, drawn once and spun with the disc. */
function etchCanvas(code: string) {
  const size = 2048
  const c = document.createElement('canvas')
  c.width = c.height = size
  const g = c.getContext('2d')!
  g.fillStyle = '#000'
  g.fillRect(0, 0, size, size)
  g.fillStyle = '#fff'
  g.font = '600 15px "SF Mono", Menlo, monospace'
  g.textAlign = 'center'
  const unit = `${code}   ·   `
  const text = unit.repeat(Math.ceil(ETCH_CHARS / unit.length))
  const radius = (20.6 / 60) * (size / 2)
  const chars = [...text]
  chars.forEach((ch, i) => {
    g.save()
    g.translate(size / 2, size / 2)
    g.rotate((i / chars.length) * Math.PI * 2)
    g.translate(0, -radius)
    g.fillText(ch, 0, 0)
    g.restore()
  })
  return c
}

function compile(gl: WebGL2RenderingContext, type: number, source: string) {
  const s = gl.createShader(type)!
  gl.shaderSource(s, source)
  gl.compileShader(s)
  if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(gl.getShaderInfoLog(s) ?? 'shader failed')
  return s
}

const ETCH_CHARS = 150

export interface DiscState {
  x: number
  y: number
  scale: number
  dim: number
  /** 1 for the full light show, 0 for a plain view of the disc. */
  fx: number
  tiltX: number
  tiltY: number
  dye: [number, number, number]
  pitch: number
  seed: number
  etchMix: number
  artMix: number
  /** Where the picture sits; see `artPlacement`. */
  art: ArtPlacement
  /** 0 = ordinary light, 1 = the stamped code is lit for reading. */
  codeLight: number
  /** Centre x, y and half-size of the part of the scene to draw; the default is all of it. */
  window?: [number, number, number]
  pointerX: number
  pointerY: number
  spin: number
  time: number
}

/** The picture before tone: per pixel its luma, then 255 inside the picture and 0 in the letterbox. */
export interface ArtField {
  size: number
  data: Uint8Array
}

/** The picture's place and tone on the disc, as the carver reads them. */
export interface ArtPlacement {
  innerMm: number
  outerMm: number
  rotationRad: number
  scale: number
  /** Offset as a share of the disc's diameter, y down. */
  shiftX: number
  shiftY: number
  contrast: number
  invert: boolean
  /** Lightest the guard lets a dark pixel be, 0 to 1. */
  floor: number
}

export const NO_ART: ArtPlacement = { innerMm: 24, outerMm: 58, rotationRad: 0, scale: 1, shiftX: 0, shiftY: 0, contrast: 0, invert: false, floor: 0 }

/** The gain of the carver's contrast curve (contrast_lut in artwork.rs). */
function contrastGain(contrast: number) {
  const c = Math.min(100, Math.max(-100, contrast)) * 2.55
  return (259 * (c + 255)) / (255 * (259 - c))
}

function rotation(x: number, y: number) {
  const [cx, sx, cy, sy] = [Math.cos(x), Math.sin(x), Math.cos(y), Math.sin(y)]
  // Column-major Ry * Rx.
  return new Float32Array([cy, 0, -sy, sy * sx, cx, cy * sx, sy * cx, -sx, cy * cx])
}

export function createDiscRenderer(canvas: HTMLCanvasElement) {
  const gl = canvas.getContext('webgl2', { antialias: false, alpha: false, preserveDrawingBuffer: true })
  if (!gl) return null
  const program = gl.createProgram()!
  gl.attachShader(program, compile(gl, gl.VERTEX_SHADER, VERTEX))
  gl.attachShader(program, compile(gl, gl.FRAGMENT_SHADER, FRAGMENT))
  gl.linkProgram(program)
  if (!gl.getProgramParameter(program, gl.LINK_STATUS)) throw new Error(gl.getProgramInfoLog(program) ?? 'link failed')
  gl.useProgram(program)
  gl.bindVertexArray(gl.createVertexArray())

  gl.pixelStorei(gl.UNPACK_FLIP_Y_WEBGL, true)
  const textures = [gl.createTexture()!, gl.createTexture()!, gl.createTexture()!]

  function upload(unit: number, source: TexImageSource) {
    gl!.activeTexture(gl!.TEXTURE0 + unit)
    gl!.bindTexture(gl!.TEXTURE_2D, textures[unit])
    gl!.texImage2D(gl!.TEXTURE_2D, 0, gl!.RGBA, gl!.RGBA, gl!.UNSIGNED_BYTE, source)
    gl!.generateMipmap(gl!.TEXTURE_2D)
    gl!.texParameteri(gl!.TEXTURE_2D, gl!.TEXTURE_MIN_FILTER, gl!.LINEAR_MIPMAP_LINEAR)
    gl!.texParameteri(gl!.TEXTURE_2D, gl!.TEXTURE_WRAP_S, gl!.CLAMP_TO_EDGE)
    gl!.texParameteri(gl!.TEXTURE_2D, gl!.TEXTURE_WRAP_T, gl!.CLAMP_TO_EDGE)
  }
  /** Rows are top first, as the carver indexes them, so this one is not flipped. */
  function uploadField(f: ArtField) {
    gl!.activeTexture(gl!.TEXTURE2)
    gl!.bindTexture(gl!.TEXTURE_2D, textures[2])
    gl!.pixelStorei(gl!.UNPACK_FLIP_Y_WEBGL, false)
    gl!.pixelStorei(gl!.UNPACK_ALIGNMENT, 1)
    gl!.texImage2D(gl!.TEXTURE_2D, 0, gl!.RG8, f.size, f.size, 0, gl!.RG, gl!.UNSIGNED_BYTE, f.data)
    gl!.pixelStorei(gl!.UNPACK_FLIP_Y_WEBGL, true)
    gl!.pixelStorei(gl!.UNPACK_ALIGNMENT, 4)
    gl!.generateMipmap(gl!.TEXTURE_2D)
    gl!.texParameteri(gl!.TEXTURE_2D, gl!.TEXTURE_MIN_FILTER, gl!.LINEAR_MIPMAP_LINEAR)
    gl!.texParameteri(gl!.TEXTURE_2D, gl!.TEXTURE_WRAP_S, gl!.CLAMP_TO_EDGE)
    gl!.texParameteri(gl!.TEXTURE_2D, gl!.TEXTURE_WRAP_T, gl!.CLAMP_TO_EDGE)
  }
  uploadField({ size: 2, data: new Uint8Array(8) })

  /** Which of the two etch textures is showing; the other holds the one fading in. */
  let front = 0

  const at = (name: string) => gl.getUniformLocation(program, name)
  const u = {
    res: at('uRes'), rot: at('uRot'), center: at('uCenter'), pointer: at('uPointer'), spin: at('uSpin'), time: at('uTime'),
    scale: at('uScale'), win: at('uWin'), pitch: at('uPitch'), seed: at('uSeed'), dim: at('uDim'), fx: at('uFx'), etchMix: at('uEtchMix'), artMix: at('uArtMix'),
    artGeo: at('uArtGeo'), artShift: at('uArtShift'), artTone: at('uArtTone'), codeLight: at('uCodeLight'), dye: at('uDye'), etchA: at('uEtchA'), etchB: at('uEtchB'), art: at('uArt'),
  }
  gl.uniform1i(u.art, 2)

  function resize() {
    const dpr = Math.min(window.devicePixelRatio || 1, MAX_PIXEL_RATIO)
    canvas.width = Math.round(canvas.clientWidth * dpr)
    canvas.height = Math.round(canvas.clientHeight * dpr)
    gl!.viewport(0, 0, canvas.width, canvas.height)
  }

  function draw(s: DiscState) {
    gl!.uniform2f(u.res, canvas.width, canvas.height)
    gl!.uniformMatrix3fv(u.rot, false, rotation(s.tiltX, s.tiltY))
    gl!.uniform3f(u.center, s.x, s.y, 0.0)
    gl!.uniform1f(u.scale, s.scale)
    gl!.uniform3f(u.win, ...(s.window ?? [0, 0, 1]))
    gl!.uniform1f(u.dim, s.dim)
    gl!.uniform1f(u.fx, s.fx)
    gl!.uniform3f(u.dye, ...s.dye)
    gl!.uniform1f(u.pitch, s.pitch)
    gl!.uniform1f(u.seed, s.seed)
    gl!.uniform1f(u.etchMix, s.etchMix)
    gl!.uniform1f(u.artMix, s.artMix)
    const a = s.art
    gl!.uniform4f(u.artGeo, a.innerMm, a.outerMm, a.rotationRad, a.scale > 0 ? a.scale : 1)
    gl!.uniform2f(u.artShift, a.shiftX, a.shiftY)
    gl!.uniform3f(u.artTone, contrastGain(a.contrast), a.invert ? 1 : 0, a.floor)
    gl!.uniform1f(u.codeLight, s.codeLight)
    gl!.uniform1i(u.etchA, front)
    gl!.uniform1i(u.etchB, 1 - front)
    gl!.uniform2f(u.pointer, s.pointerX, s.pointerY)
    gl!.uniform1f(u.spin, s.spin)
    gl!.uniform1f(u.time, s.time)
    gl!.drawArrays(gl!.TRIANGLES, 0, 3)
  }

  return {
    resize,
    draw,
    /** The first call shows the code at once; later ones load it to fade in. */
    setEtch: (code: string, immediate: boolean) => upload(immediate ? front : 1 - front, etchCanvas(code)),
    swapEtch: () => (front = 1 - front),
    setArt: (field: ArtField) => uploadField(field),
    dispose: () => gl.getExtension('WEBGL_lose_context')?.loseContext(),
  }
}
