// The flat positions' pictures in the 3D editor: each lies on the disc in its position's frame,
// greyed as `artwork::Composition::sample` greys it, clipped to the kept region unless selected.
import {
  BufferGeometry,
  Float32BufferAttribute,
  LineBasicMaterial,
  LineLoop,
  LinearFilter,
  Matrix4,
  Mesh,
  NearestFilter,
  PlaneGeometry,
  ShaderMaterial,
  TextureLoader,
  type Group,
  Vector2,
  Vector4,
  type Texture,
} from 'three'
import { layerExtent } from '../layers'
import { RING_GLSL, repeatFill } from '../ringMap'
import type { HoloImage, HoloPosition } from './scene'

const VERTEX = `
uniform mat4 toGroup;
varying vec2 vUv;
varying vec2 vAt;
void main() {
  vUv = uv;
  vAt = (toGroup * vec4(position, 1.0)).xy;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
}
`
const FRAGMENT = `
uniform sampler2D map;
uniform sampler2D cropMap;
uniform float gain, shift, invert, colour, hasCrop, inner, outer, opacity, picked;
// A picture that wraps round the ring or repeats is drawn over the whole disc and placed here, as
// \`Composition\` places it: wide is 1, ring its turns (0 flat), place its centre and turn's cosine
// and sine, ext its width and height, grid a tile cell's width and height (0 when it does not
// repeat), the grid's turn, and the row stagger.
uniform float wide, ring;
uniform vec4 place, grid;
uniform vec2 ext;
uniform float stagger;
varying vec2 vUv;
varying vec2 vAt;
${RING_GLSL}
void main() {
  float r = length(vAt);
  vec2 cuv = vec2(vAt.x, -vAt.y) / max(2.0 * outer, 1e-4) + 0.5;
  bool kept = r >= inner && r <= outer && (hasCrop < 0.5 || texture2D(cropMap, cuv).r >= 0.5);
  if (!kept && picked < 0.5) discard;
  vec2 uv = vUv;
  if (wide > 0.5) {
    vec2 q = vec2(vAt.x, -vAt.y) / max(2.0 * outer, 1e-4);
    if (ring > 0.5) {
      vec3 w = ringUnroll(q, ring);
      if (w.z < 0.5) discard;
      q = w.xy;
    }
    vec2 d = q - place.xy;
    if (grid.x > 0.0) {
      float gc = cos(grid.z), gs = sin(grid.z);
      vec2 g = vec2(gc * d.x + gs * d.y, -gs * d.x + gc * d.y);
      float row = floor(g.y / grid.y + 0.5);
      g.x -= mod(row, 2.0) * stagger * grid.x;
      g.x -= floor(g.x / grid.x + 0.5) * grid.x;
      g.y -= row * grid.y;
      d = vec2(gc * g.x - gs * g.y, gs * g.x + gc * g.y);
    }
    vec2 st = vec2(place.z * d.x + place.w * d.y, -place.w * d.x + place.z * d.y) / ext + 0.5;
    if (st.x < 0.0 || st.y < 0.0 || st.x > 1.0 || st.y > 1.0) discard;
    uv = vec2(st.x, 1.0 - st.y);
  }
  vec4 t = texture2D(map, uv);
  vec3 base = colour > 0.5 ? t.rgb * 255.0 : vec3(dot(t.rgb, vec3(0.299, 0.587, 0.114)) * 255.0);
  vec3 v = clamp((base - 128.0) * gain + 128.0 + shift, 0.0, 255.0);
  if (invert > 0.5) v = 255.0 - v;
  gl_FragColor = vec4(v / 255.0, t.a * opacity * (kept ? 0.95 : 0.2));
}
`

/** `artwork::contrast_gain`. */
const contrastGain = (contrast: number) => {
  const c = Math.min(100, Math.max(-100, contrast)) * 2.55
  return (259 * (c + 255)) / (255 * (259 - c))
}

interface Entry {
  mesh: Mesh<PlaneGeometry, ShaderMaterial>
  outline: LineLoop
  url: string
  texture: Texture | null
}

export interface FlatPictureState {
  images: readonly HoloImage[]
  positions: readonly HoloPosition[]
  positionId: string
  /** Each picture's thumbnail, a data URL, by path. */
  thumbnails: Readonly<Record<string, string>>
  /** Position crop mask lookup. */
  cropFor: (positionId: string) => { texture: Texture; hasCrop: boolean }
  selectedId: string | null
  /** Picture shown unclipped at full bounds when picked. */
  pickedId: string | null
  /** When set, the only picture drawn. */
  only: string | null
  innerRadiusMm: number
  outerRadiusMm: number
}

export function createFlatPictures(groupFor: (positionId: string) => Group, redraw: () => void) {
  const entries = new Map<string, Entry>()
  const loader = new TextureLoader()
  const square = new BufferGeometry().setAttribute('position', new Float32BufferAttribute([-0.5, -0.5, 0, 0.5, -0.5, 0, 0.5, 0.5, 0, -0.5, 0.5, 0], 3))
  const outlineMaterial = new LineBasicMaterial({ color: 0x9045f9, depthTest: false })
  const plane = new PlaneGeometry(1, 1)

  function drop(id: string, e: Entry) {
    e.mesh.removeFromParent()
    e.mesh.material.dispose()
    e.texture?.dispose()
    entries.delete(id)
  }

  function sync(st: FlatPictureState) {
    // Lower in its position's list is drawn first, under what is higher.
    const depth = (i: HoloImage) => {
      const order = st.positions.find((q) => q.id === i.positionId)?.order ?? []
      const at = order.indexOf(i.id)
      return at < 0 ? order.length : at
    }
    const shown = [...st.images].sort((a, b) => depth(b) - depth(a))
    const ids = new Set(shown.map((i) => i.id))
    for (const [id, e] of entries) if (!ids.has(id)) drop(id, e)
    // Share of the diameter to mm, y down to y up: the carve page's frame to the scene's.
    const across = 2 * st.outerRadiusMm
    shown.forEach((img, order) => {
      const url = st.thumbnails[img.path] ?? ''
      let e = entries.get(img.id)
      if (!e) {
        const material = new ShaderMaterial({
          vertexShader: VERTEX,
          fragmentShader: FRAGMENT,
          transparent: true,
          depthWrite: false,
          uniforms: {
            map: { value: null }, cropMap: { value: st.cropFor(img.positionId).texture }, toGroup: { value: new Matrix4() },
            gain: { value: 1 }, shift: { value: 0 }, invert: { value: 0 }, colour: { value: 0 },
            hasCrop: { value: 0 }, inner: { value: 0 }, outer: { value: 0 }, opacity: { value: 1 }, picked: { value: 0 },
            wide: { value: 0 }, ring: { value: 0 }, place: { value: new Vector4() }, grid: { value: new Vector4() }, ext: { value: new Vector2(1, 1) }, stagger: { value: 0 },
          },
        })
        const mesh = new Mesh(plane, material)
        mesh.userData.imageId = img.id
        const outline = new LineLoop(square, outlineMaterial)
        outline.raycast = () => {}
        outline.renderOrder = 10
        mesh.add(outline)
        e = { mesh, outline, url: '', texture: null }
        entries.set(img.id, e)
      }
      if (e.url !== url) {
        e.texture?.dispose()
        e.texture = url ? loader.load(url, redraw) : null
        e.url = url
      }
      if (e.texture) {
        e.texture.magFilter = img.pixelated ? NearestFilter : LinearFilter
        e.texture.needsUpdate = true
      }
      const group = groupFor(img.positionId)
      if (e.mesh.parent !== group) group.add(e.mesh)
      // Picture width and height on the disc, including any single-axis stretch.
      const [ew, eh] = layerExtent(img)
      const wide = img.ring || img.repeat
      const turn = (img.rotationDeg * Math.PI) / 180
      if (wide) {
        e.mesh.scale.set(across, across, 1)
        e.mesh.position.set(0, 0, 0.06 + order * 0.002)
        e.mesh.rotation.set(0, 0, 0)
      } else {
        e.mesh.scale.set(ew * across, eh * across, 1)
        e.mesh.position.set(img.x * across, -img.y * across, 0.06 + order * 0.002)
        // Clockwise as seen.
        e.mesh.rotation.set(0, 0, -turn)
      }
      e.mesh.renderOrder = 2 + order
      e.mesh.updateMatrix()
      e.mesh.visible = !!e.texture && (!st.only || st.only === img.id)
      e.outline.visible = img.id === st.selectedId && !wide
      const crop = st.cropFor(img.positionId)
      const u = e.mesh.material.uniforms
      u.map.value = e.texture
      u.cropMap.value = crop.texture
      u.hasCrop.value = crop.hasCrop ? 1 : 0
      u.toGroup.value.copy(e.mesh.matrix)
      u.gain.value = contrastGain(img.contrast)
      u.shift.value = Math.min(100, Math.max(-100, img.brightness)) * 2.55
      u.invert.value = img.invert ? 1 : 0
      u.colour.value = img.colour ? 1 : 0
      u.inner.value = st.innerRadiusMm
      u.outer.value = st.outerRadiusMm
      u.opacity.value = img.positionId === st.positionId ? 1 : 0.4
      u.picked.value = img.id === st.pickedId ? 1 : 0
      u.wide.value = wide ? 1 : 0
      u.ring.value = img.ring ? img.ringTurns : 0
      ;(u.place.value as Vector4).set(img.x, img.y, Math.cos(turn), Math.sin(turn))
      ;(u.ext.value as Vector2).set(ew || 1, eh || 1)
      const fill = repeatFill(img, ew)
      ;(u.grid.value as Vector4).set(img.repeat ? (ew || 1) / fill : 0, img.repeat ? (eh || 1) / fill : 0, (img.repeatAngle * Math.PI) / 180, 0)
      u.stagger.value = Math.min(1, Math.max(0, img.repeatStagger / 100))
    })
  }

  /** The picture's mesh, to hang the move, turn and size handles on. */
  function meshOf(id: string | null): Mesh | undefined {
    return id ? entries.get(id)?.mesh : undefined
  }

  /** The shown position's pictures, to pick with the pointer. */
  function pickable(positionId: string, images: readonly HoloImage[]): Mesh[] {
    return images.filter((i) => i.positionId === positionId).flatMap((i) => entries.get(i.id)?.mesh ?? [])
  }

  function dispose() {
    for (const [id, e] of entries) drop(id, e)
    square.dispose()
    plane.dispose()
    outlineMaterial.dispose()
  }

  return { sync, meshOf, pickable, dispose }
}
