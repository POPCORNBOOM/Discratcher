// The flat positions' pictures in the 3D editor: each lies on the disc in its position's frame,
// greyed as `artwork::Composition::sample` greys it, full where it is burned and faint outside.
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
  type Texture,
} from 'three'
import { layerExtent } from '../layers'
import { CROP_GLSL, cropUniforms } from './cropGlsl'
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
uniform float gain, shift, invert, crop, side, axis, inner, outer, opacity;
varying vec2 vUv;
varying vec2 vAt;
${CROP_GLSL}
void main() {
  vec4 t = texture2D(map, vUv);
  float luma = dot(t.rgb, vec3(0.299, 0.587, 0.114)) * 255.0;
  float v = clamp((luma - 128.0) * gain + 128.0 + shift, 0.0, 255.0);
  if (invert > 0.5) v = 255.0 - v;
  float r = length(vAt);
  bool kept = r >= inner && r <= outer && inCrop(vAt, crop, side, axis);
  gl_FragColor = vec4(vec3(v / 255.0), t.a * opacity * (kept ? 0.95 : 0.2));
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
  selectedId: string | null
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
            map: { value: null }, toGroup: { value: new Matrix4() },
            gain: { value: 1 }, shift: { value: 0 }, invert: { value: 0 },
            crop: { value: Math.PI / 2 }, side: { value: 0 }, axis: { value: 0 }, inner: { value: 0 }, outer: { value: 0 }, opacity: { value: 1 },
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
      e.mesh.scale.set(ew * across, eh * across, 1)
      e.mesh.position.set(img.x * across, -img.y * across, 0.06 + order * 0.002)
      // Clockwise as seen.
      e.mesh.rotation.set(0, 0, -(img.rotationDeg * Math.PI) / 180)
      e.mesh.renderOrder = 2 + order
      e.mesh.updateMatrix()
      e.mesh.visible = !!e.texture && (!st.only || st.only === img.id)
      e.outline.visible = img.id === st.selectedId
      const pos = st.positions.find((q) => q.id === img.positionId)
      const u = e.mesh.material.uniforms
      u.map.value = e.texture
      u.toGroup.value.copy(e.mesh.matrix)
      u.gain.value = contrastGain(img.contrast)
      u.shift.value = Math.min(100, Math.max(-100, img.brightness)) * 2.55
      u.invert.value = img.invert ? 1 : 0
      const c = cropUniforms(pos)
      u.crop.value = c.crop
      u.side.value = c.side
      u.axis.value = c.axis
      u.inner.value = st.innerRadiusMm
      u.outer.value = st.outerRadiusMm
      u.opacity.value = img.positionId === st.positionId ? 1 : 0.4
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
