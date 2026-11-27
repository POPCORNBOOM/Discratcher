<template>
  <div
    ref="host"
    class="vp3d"
    :class="{ grabbing: planeDragging, pointing: !!hoveredMarker }"
    @pointerdown="onPointerDown"
    @pointermove="onPointerMove"
    @pointerup="onPointerUp"
    @pointercancel="onPointerUp"
  >
    <canvas ref="canvas" class="vp3d-canvas" />
    <div v-if="webglError" class="vp3d-fallback notice warn">
      {{ webglError }}
    </div>
  </div>
</template>

<script setup lang="ts">
import {
  AdditiveBlending,
  AmbientLight,
  BufferGeometry,
  CircleGeometry,
  Color,
  DirectionalLight,
  DoubleSide,
  Float32BufferAttribute,
  Group,
  HemisphereLight,
  Line,
  LineBasicMaterial,
  LineLoop,
  MathUtils,
  Matrix4,
  Mesh,
  MeshBasicMaterial,
  MeshStandardMaterial,
  PCFShadowMap,
  PerspectiveCamera,
  Plane,
  Raycaster,
  RingGeometry,
  Scene,
  ShaderMaterial,
  ShadowMaterial,
  SphereGeometry,
  Uint16BufferAttribute,
  Uint32BufferAttribute,
  Vector2,
  Vector3,
  WebGLRenderer,
} from 'three'
import { OrbitControls } from 'three/examples/jsm/controls/OrbitControls.js'
import { TransformControls } from 'three/examples/jsm/controls/TransformControls.js'
import { onBeforeUnmount, onMounted, ref, watch } from 'vue'
import {
  DEFAULT_TILT_DEG,
  DISC_HOLE_RADIUS_MM,
  DISC_PHYSICAL_OUTER_MM,
  LAMP_ID,
  LAMP_MAX_UP_MM,
  LAMP_MIN_UP_MM,
  LAMP_REACH_MM,
  type Vec3,
} from '../holo/constants'
import { createAxesGizmo } from '../holo/axesGizmo'
import { createFlatPictures } from '../holo/flatPictures'
import { sampleMotion } from '../holo/motion'
import type { DisplayMeshData } from '../holo/sampleMesh'
import { DEFAULT_CROP_DEG, round1, type HoloImage, type HoloPosition, type HoloSceneObject } from '../holo/scene'
import { zonesMaterial } from '../holo/usability'

export type GizmoMode = 'translate' | 'rotate' | 'scale'

const props = defineProps<{
  objects: HoloSceneObject[]
  positions: HoloPosition[]
  positionId: string
  meshes: ReadonlyMap<string, DisplayMeshData>
  selectedId: string | null
  mode: GizmoMode
  snap: boolean
  innerRadiusMm: number
  outerRadiusMm: number
  lampMm: Vec3
  showLight: boolean
  distanceMm: number
  previewT: number
  /** The flat positions' pictures, and their thumbnails by path. */
  images: HoloImage[]
  thumbnails: Record<string, string>
  selectedImageId: string | null
}>()

const emit = defineEmits<{
  select: [id: string | null]
  'select-image': [id: string]
  /** A picture moved, turned or sized with the handles: its layer's new place (`Layer`). */
  'image-transform': [payload: { id: string; x: number; y: number; size: number; rotationDeg: number }]
  position: [id: string]
  'transform-start': [id: string]
  'transform-change': [payload: { id: string; positionMm: Vec3; rotationDeg: Vec3; scaleMm: Vec3 }]
  'transform-end': [id: string]
}>()

const SNAP_TRANSLATE_MM = 2
const SNAP_ROTATE_RAD = MathUtils.degToRad(15)
const SNAP_SCALE_MM = 2
const CAMERA_FOV_DEG = 32
const CAMERA_VIEW_DIST_MM = 275
const LAMP_CONE_SEGMENTS = 96
const LIGHT_FADE_MS = 200

const host = ref<HTMLElement | null>(null)
const canvas = ref<HTMLCanvasElement | null>(null)
const webglError = ref('')
const planeDragging = ref(false)

let renderer: WebGLRenderer | null = null
let scene: Scene | null = null
let camera: PerspectiveCamera | null = null
let axes: ReturnType<typeof createAxesGizmo> | null = null
let pictures: ReturnType<typeof createFlatPictures> | null = null
let orbit: OrbitControls | null = null
let gizmo: TransformControls | null = null
let resizeObs: ResizeObserver | null = null
let animFrame = 0

// Scene sub-groups and dynamic references
let discGroup: Group | null = null
let usabilityMesh: Mesh | null = null
let dropLine: Line | null = null
let dropDot: Mesh | null = null
let shadowMat: ShadowMaterial | null = null
let discBaseMat: MeshStandardMaterial | null = null
let ringLineMats: LineBasicMaterial[] = []

/** Each position's objects, turned as the disc is turned. */
const positionGroups = new Map<string, Group>()
let markers: Group | null = null
/** The angle at the top as drawn, easing to the current position's. */
let shownPhi = 0
let lampMesh: Mesh | null = null
let lampCone: Mesh<BufferGeometry, ShaderMaterial> | null = null
let lightFade = 0
let lastFrameMs = 0

/** Each object: the mesh you place, and in transform mode a ghost of it moved by T at the preview tilt. */
type ObjectEntry = { mesh: Mesh; ghost: Mesh; wrapper: Group; path: string }
const objectMeshes = new Map<string, ObjectEntry>()
const geometryCache = new Map<string, BufferGeometry>()
const matPos = new Matrix4()
const matNeg = new Matrix4()
const matT = new Matrix4()

const raycaster = new Raycaster()
// The arrows on the rim are thin lines: let a click come near.
raycaster.params.Line.threshold = 3
const pointerNdc = new Vector2()
const dragPlane = new Plane(new Vector3(0, 0, 1), 0)
const dragHit = new Vector3()
const dragOffset = new Vector3()

const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)').matches
let gizmoDragging = false
let pointerDownPos = { x: 0, y: 0 }
let dragTargetId: string | null = null
let dragTargetZ = 0
let dragStarted = false
let suppressSync = false

function makeCirclePoints(radius: number, segments = 128, z = 0.08): Vector3[] {
  const pts: Vector3[] = []
  for (let i = 0; i < segments; i++) {
    const a = (i / segments) * Math.PI * 2
    pts.push(new Vector3(Math.cos(a) * radius, Math.sin(a) * radius, z))
  }
  return pts
}

function buildDisc() {
  if (!scene) return
  if (discGroup) {
    scene.remove(discGroup)
    discGroup.traverse((o) => {
      if (o instanceof Mesh || o instanceof Line) {
        o.geometry.dispose()
      }
    })
  }
  ringLineMats.forEach((m) => m.dispose())
  ringLineMats = []
  discGroup = new Group()
  const rIn = props.innerRadiusMm
  const rOut = props.outerRadiusMm

  // 1. Translucent polycarbonate disc: objects below z = 0 show through as behind glass.
  const baseGeo = new RingGeometry(DISC_HOLE_RADIUS_MM, DISC_PHYSICAL_OUTER_MM, 128)
  discBaseMat = new MeshStandardMaterial({
    color: 0x2a2c38,
    roughness: 0.35,
    metalness: 0.18,
    transparent: true,
    opacity: 0.38,
    side: DoubleSide,
    depthWrite: false,
  })
  const baseMesh = new Mesh(baseGeo, discBaseMat)
  baseMesh.renderOrder = 1
  discGroup.add(baseMesh)

  // 2. Soft shadow catcher on the disc plane for objects floating above (z > 0).
  const shadowGeo = new RingGeometry(DISC_HOLE_RADIUS_MM, DISC_PHYSICAL_OUTER_MM, 96)
  shadowMat = new ShadowMaterial({ opacity: 0.42, depthWrite: false })
  const shadowMesh = new Mesh(shadowGeo, shadowMat)
  shadowMesh.position.z = 0.04
  shadowMesh.receiveShadow = true
  shadowMesh.renderOrder = 2
  discGroup.add(shadowMesh)

  // 3. Where depth shows, worked out on the graphics card so that a lamp can be dragged.
  usabilityMesh?.geometry.dispose()
  ;(usabilityMesh?.material as ShaderMaterial | undefined)?.dispose()
  usabilityMesh = new Mesh(new RingGeometry(rIn, rOut, 128), zonesMaterial())
  usabilityMesh.position.z = 0.06
  usabilityMesh.renderOrder = 3
  discGroup.add(usabilityMesh)

  // 4. Crisp ring borders (center hole, data inner, data outer, physical outer).
  const ringSpecs: [number, number, number][] = [
    [DISC_HOLE_RADIUS_MM, 0x525466, 0.65],
    [rIn, 0x9045f9, 0.55],
    [rOut, 0x9045f9, 0.65],
    [DISC_PHYSICAL_OUTER_MM, 0x5d6072, 0.7],
  ]
  for (const [radius, hex, opacity] of ringSpecs) {
    const geo = new BufferGeometry().setFromPoints(makeCirclePoints(radius))
    const mat = new LineBasicMaterial({ color: hex, transparent: true, opacity })
    ringLineMats.push(mat)
    const loop = new LineLoop(geo, mat)
    loop.renderOrder = 4
    discGroup.add(loop)
  }

  scene.add(discGroup)
  updateZones()
}

function groupFor(id: string): Group {
  let g = positionGroups.get(id)
  if (!g) {
    g = new Group()
    positionGroups.set(id, g)
    scene?.add(g)
  }
  return g
}

const currentPosition = () => props.positions.find((p) => p.id === props.positionId)
const currentPhi = () => currentPosition()?.phiDeg ?? 0
const currentCrop = () => currentPosition()?.cropDeg ?? DEFAULT_CROP_DEG

/** Turn every position's objects to where they stand with `shownPhi` at the top. */
function turnGroups() {
  const left = turnLeft()
  const settled = Math.abs(left) < 0.05
  for (const p of props.positions) {
    groupFor(p.id).rotation.z = p.id === props.positionId && settled ? 0 : -MathUtils.degToRad(p.phiDeg - shownPhi)
  }
  if (markers) markers.rotation.z = MathUtils.degToRad(shownPhi)
  const u = (usabilityMesh?.material as ShaderMaterial | undefined)?.uniforms
  if (u) u.phi.value = settled ? 0 : MathUtils.degToRad(left)
}

/** Degrees still to turn, the short way round. */
function turnLeft(): number {
  return ((currentPhi() - shownPhi + 540) % 360) - 180
}

/** A small arrow at the viewer's side of the rim for every position, the current one lit. */
/** One arrow outside the rim per position, pointing at where it turns to; click one to go there. */
const hoveredMarker = ref<string | null>(null)

function buildMarkers() {
  if (!scene) return
  markers?.removeFromParent()
  markers?.traverse((o) => {
    if (o instanceof Line) {
      o.geometry.dispose()
      ;(o.material as LineBasicMaterial).dispose()
    }
  })
  markers = new Group()
  const r = DISC_PHYSICAL_OUTER_MM
  for (const p of props.positions) {
    const arrow = new Group()
    arrow.rotation.z = -MathUtils.degToRad(p.phiDeg)
    arrow.userData.positionId = p.id
    const line = new Line(
      new BufferGeometry().setFromPoints([new Vector3(-3.5, -r - 4.5, 0.08), new Vector3(0, -r - 1.5, 0.08), new Vector3(3.5, -r - 4.5, 0.08)]),
      new LineBasicMaterial({ transparent: true }),
    )
    arrow.add(line)
    markers.add(arrow)
  }
  scene.add(markers)
  paintMarkers()
  turnGroups()
}

function paintMarkers() {
  for (const m of markers?.children ?? []) {
    const id = m.userData.positionId as string
    const mat = (m.children[0] as Line).material as LineBasicMaterial
    const here = id === props.positionId
    const hot = id === hoveredMarker.value
    mat.color.setHex(here ? 0xae79fb : hot ? 0xd0d2dc : 0x77777f)
    mat.opacity = here || hot ? 1 : 0.7
  }
}

/** How near an arrow the pointer must be to take it, CSS pixels, whatever the view. */
const MARKER_REACH_PX = 16
const markerAt_ = new Vector3()

/** The arrow nearest the pointer on screen, if within reach. */
function markerAt(e: PointerEvent): string | null {
  const el = canvas.value
  if (!markers || !camera || !el) return null
  const rect = el.getBoundingClientRect()
  let best: string | null = null
  let bestD = MARKER_REACH_PX
  for (const arrow of markers.children) {
    markerAt_.set(0, -DISC_PHYSICAL_OUTER_MM - 3, 0.08).applyMatrix4(arrow.matrixWorld).project(camera)
    if (markerAt_.z > 1) continue
    const x = rect.left + ((markerAt_.x + 1) / 2) * rect.width
    const y = rect.top + ((1 - markerAt_.y) / 2) * rect.height
    const d = Math.hypot(e.clientX - x, e.clientY - y)
    if (d < bestD) {
      bestD = d
      best = arrow.userData.positionId as string
    }
  }
  return best
}

function updateZones() {
  const u = (usabilityMesh?.material as ShaderMaterial | undefined)?.uniforms
  if (!u) return
  ;(u.lamp.value as Vector3).set(...props.lampMm)
  u.distance.value = props.distanceMm
  const left = turnLeft()
  u.phi.value = Math.abs(left) < 0.05 ? 0 : MathUtils.degToRad(left)
  u.crop.value = MathUtils.degToRad(currentCrop())
}

function ensureDropIndicator() {
  if (!scene) return
  const lineGeo = new BufferGeometry().setFromPoints([new Vector3(0, 0, 0), new Vector3(0, 0, 1)])
  const lineMat = new LineBasicMaterial({ color: 0x9045f9, transparent: true, opacity: 0.85 })
  dropLine = new Line(lineGeo, lineMat)
  dropLine.visible = false
  dropLine.renderOrder = 5
  scene.add(dropLine)

  const dotGeo = new CircleGeometry(1.4, 24)
  const dotMat = new MeshBasicMaterial({ color: 0x9045f9, transparent: true, opacity: 0.9, side: DoubleSide, depthWrite: false })
  dropDot = new Mesh(dotGeo, dotMat)
  dropDot.position.z = 0.12
  dropDot.visible = false
  dropDot.renderOrder = 5
  scene.add(dropDot)
}

function updateDropIndicator(obj: HoloSceneObject | undefined) {
  if (!dropLine || !dropDot) return
  if (!obj || !obj.visible) {
    dropLine.visible = false
    dropDot.visible = false
    return
  }
  const [x, y, z] = obj.positionMm
  dropDot.position.set(x, y, 0.12)
  dropDot.visible = true

  const posAttr = dropLine.geometry.getAttribute('position') as Float32BufferAttribute
  posAttr.setXYZ(0, x, y, 0.12)
  posAttr.setXYZ(1, x, y, z)
  posAttr.needsUpdate = true
  dropLine.visible = Math.abs(z) > 0.4
}

function getOrCreateGeometry(path: string, data: DisplayMeshData): BufferGeometry {
  let geo = geometryCache.get(path)
  if (geo) return geo
  geo = new BufferGeometry()
  geo.setAttribute('position', new Float32BufferAttribute(data.positions, 3))
  geo.setAttribute('normal', new Float32BufferAttribute(data.normals, 3))
  geo.setIndex(new Uint32BufferAttribute(data.indices, 1))
  geo.computeBoundingSphere()
  geo.computeBoundingBox()
  geometryCache.set(path, geo)
  return geo
}

function applyThemeColors() {
  if (!scene) return
  if (discBaseMat) {
    discBaseMat.color.setHex(0x2a2c38)
    discBaseMat.opacity = 0.38
  }
  if (shadowMat) {
    shadowMat.opacity = 0.42
  }
  updateObjectMaterials()
  updateZones()
}

function updateObjectMaterials() {
  for (const [id, entry] of objectMeshes) {
    const mat = entry.mesh.material as MeshStandardMaterial
    const selected = id === props.selectedId
    // Another position's objects are only there to be kept clear of.
    const here = props.objects.find((o) => o.id === id)?.positionId === props.positionId
    mat.transparent = !here
    mat.opacity = here ? 1 : 0.22
    mat.depthWrite = here
    entry.mesh.castShadow = here
    if (selected) {
      mat.color.setHex(0xeee9ff)
      mat.emissive.setHex(0x9045f9)
      mat.emissiveIntensity = 0.18
    } else {
      mat.color.setHex(0xb8bcc8)
      mat.emissive.setHex(0x000000)
      mat.emissiveIntensity = 0
    }
  }
}

/**
 * Show where T puts the object at the preview tilt as a ghost beside the mesh you edit, so that
 * moving, turning and sizing the object never pass through T. Matches Rust `place_scene_object_with`:
 * scene_p = pos + L * (scale * R * p) + tr, i.e. translate(pos) * T * translate(-pos) on the mesh.
 */
function applyMotionWrapper(entry: ObjectEntry, obj: HoloSceneObject) {
  const { mesh, ghost, wrapper } = entry
  const moving = obj.mode === 'transform' && obj.positionId === props.positionId && obj.motion.samples.length > 0
  if (!moving) {
    wrapper.visible = false
    return
  }
  const m = sampleMotion(obj.motion.samples, props.previewT)
  const still = m.every((v, i) => Math.abs(v - (i % 5 === 0 ? 1 : 0)) < 1e-6)
  wrapper.visible = obj.visible && !still
  if (!wrapper.visible) return
  ghost.position.copy(mesh.position)
  ghost.rotation.copy(mesh.rotation)
  ghost.scale.copy(mesh.scale)
  matT.set(
    m[0], m[1], m[2], m[3],
    m[4], m[5], m[6], m[7],
    m[8], m[9], m[10], m[11],
    0, 0, 0, 1,
  )
  const p = mesh.position
  matPos.makeTranslation(p.x, p.y, p.z)
  matNeg.makeTranslation(-p.x, -p.y, -p.z)
  wrapper.matrix.copy(matPos).multiply(matT).multiply(matNeg)
  wrapper.matrixWorldNeedsUpdate = true
}

function syncMotionTransforms() {
  for (const obj of props.objects) {
    const entry = objectMeshes.get(obj.id)
    if (!entry) continue
    applyMotionWrapper(entry, obj)
  }
}

function syncObjects() {
  if (!scene || suppressSync) return
  const activeIds = new Set(props.objects.map((o) => o.id))

  // Remove deleted objects
  for (const [id, entry] of objectMeshes) {
    if (!activeIds.has(id)) {
      if (gizmo?.object === entry.mesh) gizmo.detach()
      entry.mesh.removeFromParent()
      entry.wrapper.removeFromParent()
      ;(entry.mesh.material as MeshStandardMaterial).dispose()
      ;(entry.ghost.material as MeshStandardMaterial).dispose()
      objectMeshes.delete(id)
    }
  }

  // Create or update current objects
  for (const obj of props.objects) {
    const data = props.meshes.get(obj.path)
    if (!data) continue
    let entry = objectMeshes.get(obj.id)
    if (!entry || entry.path !== obj.path) {
      if (entry) {
        if (gizmo?.object === entry.mesh) gizmo.detach()
        entry.mesh.removeFromParent()
        entry.wrapper.removeFromParent()
        ;(entry.mesh.material as MeshStandardMaterial).dispose()
        ;(entry.ghost.material as MeshStandardMaterial).dispose()
      }
      const geo = getOrCreateGeometry(obj.path, data)
      const mat = new MeshStandardMaterial({
        color: 0xd0d2dc,
        roughness: 0.42,
        metalness: 0.12,
        side: DoubleSide,
      })
      const mesh = new Mesh(geo, mat)
      mesh.castShadow = true
      mesh.receiveShadow = true
      mesh.userData.objectId = obj.id
      // Match Rust `rot_xyz` (Rz * Ry * Rx).
      mesh.rotation.order = 'ZYX'
      // The ghost cannot be picked or dragged: it only shows T.
      const ghost = new Mesh(geo, new MeshStandardMaterial({ color: 0xae79fb, roughness: 0.6, transparent: true, opacity: 0.35, depthWrite: false, side: DoubleSide }))
      ghost.rotation.order = 'ZYX'
      ghost.raycast = () => {}
      const wrapper = new Group()
      wrapper.matrixAutoUpdate = false
      wrapper.visible = false
      wrapper.add(ghost)
      entry = { mesh, ghost, wrapper, path: obj.path }
      objectMeshes.set(obj.id, entry)
    }
    const { wrapper, mesh } = entry
    const group = groupFor(obj.positionId)
    if (mesh.parent !== group) group.add(mesh)
    if (wrapper.parent !== group) group.add(wrapper)
    mesh.visible = obj.visible
    mesh.position.set(obj.positionMm[0], obj.positionMm[1], obj.positionMm[2])
    mesh.rotation.set(
      MathUtils.degToRad(obj.rotationDeg[0]),
      MathUtils.degToRad(obj.rotationDeg[1]),
      MathUtils.degToRad(obj.rotationDeg[2]),
      'ZYX',
    )
    mesh.scale.set(...obj.scaleMm)
    applyMotionWrapper(entry, obj)
  }

  for (const [id, g] of positionGroups) {
    if (!props.positions.some((p) => p.id === id)) {
      g.removeFromParent()
      positionGroups.delete(id)
    }
  }
  turnGroups()
  updateObjectMaterials()
  syncLamp()
  syncGizmoAttachment()
  updateDropIndicator(props.objects.find((o) => o.id === props.selectedId && o.positionId === props.positionId))
}

/** The lamp and the soft oblique cone of light it casts onto the disc. */
function buildLamp() {
  if (!scene) return
  lampMesh = new Mesh(new SphereGeometry(7, 24, 16), new MeshBasicMaterial({ color: 0xfff3d6 }))
  lampMesh.userData.objectId = LAMP_ID

  // Vertex 0 is the apex at the lamp (along = 0); vertices 1..N are the disc outer circle (along = 1).
  const positions = new Float32Array((LAMP_CONE_SEGMENTS + 1) * 3)
  const along = new Float32Array(LAMP_CONE_SEGMENTS + 1)
  const indices = new Uint16Array(LAMP_CONE_SEGMENTS * 3)
  for (let i = 0; i < LAMP_CONE_SEGMENTS; i++) {
    along[i + 1] = 1
    indices[i * 3] = 0
    indices[i * 3 + 1] = i + 1
    indices[i * 3 + 2] = ((i + 1) % LAMP_CONE_SEGMENTS) + 1
  }
  const coneGeo = new BufferGeometry()
  coneGeo.setAttribute('position', new Float32BufferAttribute(positions, 3))
  coneGeo.setAttribute('along', new Float32BufferAttribute(along, 1))
  coneGeo.setIndex(new Uint16BufferAttribute(indices, 1))

  lightFade = props.showLight ? 1 : 0
  const coneMat = new ShaderMaterial({
    transparent: true,
    depthWrite: false,
    blending: AdditiveBlending,
    side: DoubleSide,
    uniforms: {
      fade: { value: lightFade },
    },
    vertexShader: `
      attribute float along;
      varying float vAlong;
      void main() {
        vAlong = along;
        gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
      }
    `,
    fragmentShader: `
      uniform float fade;
      varying float vAlong;
      void main() {
        float f = fade * fade * (3.0 - 2.0 * fade);
        float alpha = mix(0.02, 0.10, vAlong) * f;
        gl_FragColor = vec4(1.0, 0.953, 0.839, alpha);
      }
    `,
  })
  lampCone = new Mesh(coneGeo, coneMat)
  lampCone.renderOrder = 11
  lampCone.raycast = () => {}
  lampCone.visible = lightFade > 0

  scene.add(lampMesh, lampCone)
  syncLamp()
}

function syncLamp() {
  if (!lampMesh || !lampCone) return
  if (!(gizmoDragging && gizmo?.object === lampMesh)) lampMesh.position.set(...props.lampMm)
  const pos = lampCone.geometry.getAttribute('position') as Float32BufferAttribute
  const at = lampMesh.position
  pos.setXYZ(0, at.x, at.y, at.z)
  const r = props.outerRadiusMm
  for (let i = 0; i < LAMP_CONE_SEGMENTS; i++) {
    const a = (i / LAMP_CONE_SEGMENTS) * Math.PI * 2
    pos.setXYZ(i + 1, Math.cos(a) * r, Math.sin(a) * r, 0)
  }
  pos.needsUpdate = true
  lampCone.geometry.computeBoundingSphere()
}

/** Fade the light cone in or out over LIGHT_FADE_MS, hiding it once fully transparent. */
function stepLightFade(dtMs: number) {
  if (!lampCone) return
  const target = props.showLight ? 1 : 0
  if (lightFade !== target) {
    if (reducedMotion) {
      lightFade = target
    } else {
      const step = dtMs / LIGHT_FADE_MS
      lightFade = lightFade < target ? Math.min(target, lightFade + step) : Math.max(target, lightFade - step)
    }
    lampCone.material.uniforms.fade.value = lightFade
  }
  lampCone.visible = lightFade > 0
}

function syncGizmoAttachment() {
  if (!gizmo) return
  // A lamp has a place and nothing else to change.
  gizmo.setMode(props.selectedId === LAMP_ID ? 'translate' : props.mode)
  // A picture lies on the disc: it moves and sizes across it and turns about its normal.
  const picture = pictures?.meshOf(props.selectedImageId)
  if (picture && props.images.some((i) => i.id === props.selectedImageId && i.positionId === props.positionId)) {
    const turning = props.mode === 'rotate'
    gizmo.showX = gizmo.showY = !turning
    gizmo.showZ = turning
    if (gizmo.object !== picture) gizmo.attach(picture)
    return
  }
  gizmo.showX = gizmo.showY = gizmo.showZ = true
  if (props.selectedId === LAMP_ID && lampMesh) {
    if (gizmo.object !== lampMesh) gizmo.attach(lampMesh)
    return
  }
  const selObj = props.objects.find((o) => o.id === props.selectedId && o.visible && o.positionId === props.positionId)
  const entry = selObj ? objectMeshes.get(selObj.id) : undefined
  if (entry) {
    if (gizmo.object !== entry.mesh) gizmo.attach(entry.mesh)
  } else {
    gizmo.detach()
  }
}

function syncGizmoModeAndSnap() {
  if (!gizmo) return
  gizmo.setMode(props.selectedId === LAMP_ID ? 'translate' : props.mode)
  gizmo.setTranslationSnap(props.snap ? SNAP_TRANSLATE_MM : null)
  gizmo.setRotationSnap(props.snap ? SNAP_ROTATE_RAD : null)
  gizmo.setScaleSnap(props.snap ? SNAP_SCALE_MM : null)
}

function resetCamera() {
  if (!camera || !orbit) return
  const tiltRad = MathUtils.degToRad(DEFAULT_TILT_DEG)
  camera.position.set(0, -CAMERA_VIEW_DIST_MM * Math.sin(tiltRad), CAMERA_VIEW_DIST_MM * Math.cos(tiltRad))
  orbit.target.set(0, 0, 0)
  orbit.update()
}

/** A camera pose: where it looks and where it stands. */
interface Pose { target: Vector3; position: Vector3 }
/** A pose in flight, eased from `from` to `to` about the target, z up. */
let flight: { from: Pose; to: Pose; start: number; ms: number; done: () => void } | null = null
/** Where the camera was before it went face-on, to go back to. */
let beforeFaceOn: Pose | null = null

const pose = (): Pose => ({ target: orbit!.target.clone(), position: camera!.position.clone() })

/**
 * Straight down on the disc, framed as the preview frames it, turned by `turnDeg` as the preview
 * turns it relative to this view: what stands at the top here stands at `turnDeg` round from it.
 */
function faceOnPose(turnDeg: number): Pose {
  const w = Math.max(1, host.value?.clientWidth ?? 1)
  const h = Math.max(1, host.value?.clientHeight ?? 1)
  // As HoloPreview frames it: the whole disc and a little room, across the narrower side.
  const need = (props.outerRadiusMm * 1.24) / Math.min(1, w / h)
  const dist = need / Math.tan(MathUtils.degToRad(CAMERA_FOV_DEG / 2))
  const d = MathUtils.degToRad(turnDeg)
  // Screen-up on the disc, and the camera leaning a hair the other way so that is what OrbitControls keeps up.
  const up = new Vector3(Math.sin(d), Math.cos(d), 0)
  const lean = 0.002
  return { target: new Vector3(), position: up.multiplyScalar(-dist * Math.sin(lean)).setZ(dist * Math.cos(lean)) }
}

/** Ease the camera to `to` over `ms`; a flight already going is dropped where it is. */
function flyTo(to: Pose, ms: number): Promise<void> {
  if (!camera || !orbit) return Promise.resolve()
  flight?.done()
  if (ms <= 0 || reducedMotion) {
    flight = null
    orbit.target.copy(to.target)
    camera.position.copy(to.position)
    orbit.update()
    return Promise.resolve()
  }
  return new Promise((done) => {
    flight = { from: pose(), to, start: performance.now(), ms, done }
  })
}

/** One step of the flight: the target slides, the camera swings round it on a sphere. */
function stepFlight(now: number) {
  if (!flight || !camera || !orbit) return
  const k = Math.min(1, (now - flight.start) / flight.ms)
  const e = k < 0.5 ? 4 * k * k * k : 1 - (-2 * k + 2) ** 3 / 2
  const sph = (p: Pose) => {
    const o = p.position.clone().sub(p.target)
    const r = o.length()
    return { r, polar: Math.acos(MathUtils.clamp(o.z / r, -1, 1)), azimuth: Math.atan2(o.y, o.x) }
  }
  const a = sph(flight.from)
  const b = sph(flight.to)
  const turn = ((b.azimuth - a.azimuth + 3 * Math.PI) % (2 * Math.PI)) - Math.PI
  const r = a.r * (b.r / a.r) ** e
  const polar = a.polar + (b.polar - a.polar) * e
  const azimuth = a.azimuth + turn * e
  orbit.target.lerpVectors(flight.from.target, flight.to.target, e)
  camera.position.set(Math.sin(polar) * Math.cos(azimuth), Math.sin(polar) * Math.sin(azimuth), Math.cos(polar)).multiplyScalar(r).add(orbit.target)
  if (k >= 1) {
    const done = flight.done
    flight = null
    done()
  }
}

/** Go straight down on the disc as the preview shows it, turned by `turnDeg`; remembers where it was. */
function faceOn(turnDeg: number, ms: number): Promise<void> {
  if (!camera || !orbit) return Promise.resolve()
  beforeFaceOn ??= pose()
  return flyTo(faceOnPose(turnDeg), ms)
}

/** Back to where the camera was before `faceOn`. */
function faceOff(ms: number): Promise<void> {
  const back = beforeFaceOn
  beforeFaceOn = null
  return back ? flyTo(back, ms) : Promise.resolve()
}

function focusSelected() {
  if (!camera || !orbit || !props.selectedId) return
  if (props.selectedId === LAMP_ID) {
    // Far enough back to see the lamp and the disc together.
    const reach = Math.hypot(...props.lampMm)
    orbit.target.set(props.lampMm[0] / 2, props.lampMm[1] / 2, props.lampMm[2] / 2)
    camera.position.copy(orbit.target).add(new Vector3(0, -Math.max(300, reach * 2.4), reach * 0.5))
    orbit.update()
    return
  }
  const obj = props.objects.find((o) => o.id === props.selectedId)
  if (!obj) return
  const target = new Vector3(obj.positionMm[0], obj.positionMm[1], obj.positionMm[2])
  const dir = camera.position.clone().sub(orbit.target)
  if (dir.lengthSq() < 1) dir.set(0, -50, 120)
  const desiredDist = Math.max(85, Math.max(...obj.scaleMm) * 4.5)
  dir.normalize().multiplyScalar(desiredDist)
  orbit.target.copy(target)
  camera.position.copy(target).add(dir)
  orbit.update()
}

function updatePointerNdc(e: PointerEvent) {
  const el = canvas.value
  if (!el) return
  const rect = el.getBoundingClientRect()
  pointerNdc.x = ((e.clientX - rect.left) / rect.width) * 2 - 1
  pointerNdc.y = -((e.clientY - rect.top) / rect.height) * 2 + 1
}

function pickObjectId(e: PointerEvent): string | null {
  if (!camera) return null
  updatePointerNdc(e)
  raycaster.setFromCamera(pointerNdc, camera)
  const visibleMeshes: Mesh[] = lampMesh ? [lampMesh] : []
  for (const { mesh } of objectMeshes.values()) {
    if (mesh.visible) visibleMeshes.push(mesh)
  }
  const hits = raycaster.intersectObjects(visibleMeshes, false)
  return (hits[0]?.object.userData.objectId as string | undefined) ?? null
}

/** The shown position's picture under the pointer, when no model is: pictures lie under models. */
function pickImageId(e: PointerEvent): string | null {
  if (!camera || !pictures) return null
  updatePointerNdc(e)
  raycaster.setFromCamera(pointerNdc, camera)
  const hits = raycaster.intersectObjects(pictures.pickable(props.positionId, props.images), false)
  return (hits[0]?.object.userData.imageId as string | undefined) ?? null
}

function onPointerDown(e: PointerEvent) {
  if (e.button !== 0 || gizmoDragging) return
  // If pointer is hovering a TransformControls handle, let the gizmo own this drag.
  if (gizmo?.axis) return
  pointerDownPos = { x: e.clientX, y: e.clientY }

  const mark = markerAt(e)
  if (mark) {
    emit('position', mark)
    return
  }
  const hitId = pickObjectId(e)
  if (!hitId) {
    const imageId = pickImageId(e)
    if (imageId && imageId !== props.selectedImageId) emit('select-image', imageId)
    return
  }

  if (hitId !== props.selectedId) {
    emit('select', hitId)
  }

  // In translate mode, dragging directly on the model slides it across the disc plane (XY) at its current z.
  if (props.mode === 'translate' && camera) {
    const obj = props.objects.find((o) => o.id === hitId)
    if (!obj || obj.positionId !== props.positionId) return
    dragTargetId = hitId
    dragTargetZ = obj.positionMm[2]
    dragPlane.set(new Vector3(0, 0, 1), -dragTargetZ)
    updatePointerNdc(e)
    raycaster.setFromCamera(pointerNdc, camera)
    if (raycaster.ray.intersectPlane(dragPlane, dragHit)) {
      dragOffset.set(obj.positionMm[0] - dragHit.x, obj.positionMm[1] - dragHit.y, 0)
      dragStarted = false
      planeDragging.value = true
      if (orbit) orbit.enabled = false
      ;(e.currentTarget as HTMLElement | null)?.setPointerCapture(e.pointerId)
    }
  }
}

function onPointerMove(e: PointerEvent) {
  if (!planeDragging.value && !gizmoDragging) {
    const mark = markerAt(e)
    if (mark !== hoveredMarker.value) {
      hoveredMarker.value = mark
      paintMarkers()
    }
  }
  if (!planeDragging.value || !dragTargetId || !camera) return
  const dist = Math.hypot(e.clientX - pointerDownPos.x, e.clientY - pointerDownPos.y)
  if (!dragStarted && dist < 3) return
  if (!dragStarted) {
    dragStarted = true
    emit('transform-start', dragTargetId)
  }
  updatePointerNdc(e)
  raycaster.setFromCamera(pointerNdc, camera)
  if (!raycaster.ray.intersectPlane(dragPlane, dragHit)) return

  let nx = dragHit.x + dragOffset.x
  let ny = dragHit.y + dragOffset.y
  if (props.snap) {
    nx = Math.round(nx / SNAP_TRANSLATE_MM) * SNAP_TRANSLATE_MM
    ny = Math.round(ny / SNAP_TRANSLATE_MM) * SNAP_TRANSLATE_MM
  }
  nx = MathUtils.clamp(round1(nx), -DISC_PHYSICAL_OUTER_MM, DISC_PHYSICAL_OUTER_MM)
  ny = MathUtils.clamp(round1(ny), -DISC_PHYSICAL_OUTER_MM, DISC_PHYSICAL_OUTER_MM)

  const obj = props.objects.find((o) => o.id === dragTargetId)
  const entry = objectMeshes.get(dragTargetId)
  if (!obj || !entry) return

  entry.mesh.position.set(nx, ny, dragTargetZ)
  applyMotionWrapper(entry, obj)
  updateDropIndicator({ ...obj, positionMm: [nx, ny, dragTargetZ] })
  suppressSync = true
  emit('transform-change', {
    id: dragTargetId,
    positionMm: [nx, ny, round1(dragTargetZ)],
    rotationDeg: [...obj.rotationDeg] as Vec3,
    scaleMm: [...obj.scaleMm] as Vec3,
  })
  suppressSync = false
}

function onPointerUp(e: PointerEvent) {
  if (planeDragging.value) {
    const finishedId = dragTargetId
    const wasDragged = dragStarted
    planeDragging.value = false
    dragTargetId = null
    dragStarted = false
    if (orbit) orbit.enabled = true
    if (wasDragged && finishedId) emit('transform-end', finishedId)
    return
  }
  if (e.button !== 0 || gizmoDragging) return
  const moved = Math.hypot(e.clientX - pointerDownPos.x, e.clientY - pointerDownPos.y)
  if (moved > 4 || markerAt(e)) return
  const hitId = pickObjectId(e) ?? pickImageId(e)
  if (!hitId && !gizmo?.axis) {
    emit('select', null)
  }
}

function handleResize() {
  if (!host.value || !renderer || !camera) return
  const w = Math.max(1, host.value.clientWidth)
  const h = Math.max(1, host.value.clientHeight)
  camera.aspect = w / h
  camera.updateProjectionMatrix()
  renderer.setSize(w, h, false)
}

function animate() {
  animFrame = requestAnimationFrame(animate)
  // Hidden behind the preview or another page: leave the graphics card to them.
  if (!host.value?.offsetParent) {
    lastFrameMs = 0
    return
  }
  const now = performance.now()
  const dt = lastFrameMs ? Math.min(100, now - lastFrameMs) : 0
  lastFrameMs = now
  const left = turnLeft()
  if (left !== 0) {
    const jump = Math.abs(left) < 0.05 || reducedMotion
    shownPhi = jump ? currentPhi() : shownPhi + left * 0.16
    turnGroups()
  }
  stepLightFade(dt)
  stepFlight(now)
  orbit?.update()
  if (renderer && scene && camera) {
    renderer.render(scene, camera)
    axes?.render(renderer, camera)
  }
}

onMounted(() => {
  const el = canvas.value
  const container = host.value
  if (!el || !container) return


  try {
    renderer = new WebGLRenderer({ canvas: el, antialias: true, alpha: true, powerPreference: 'high-performance' })
  } catch (err) {
    webglError.value = String(err)
    return
  }
  renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 2))
  renderer.shadowMap.enabled = true
  renderer.shadowMap.type = PCFShadowMap

  scene = new Scene()
  axes = createAxesGizmo()
  pictures = createFlatPictures(groupFor, () => {})
  camera = new PerspectiveCamera(CAMERA_FOV_DEG, 1, 1, 8000)
  // Scene space: +x right, +y up (12 o'clock), +z toward viewer.
  camera.up.set(0, 0, 1)

  // Soft three-point studio lighting + soft disc shadow from above.
  const hemi = new HemisphereLight(0xffffff, 0x3b3d52, 0.9)
  hemi.position.set(0, 0, 200)
  scene.add(hemi)

  const amb = new AmbientLight(0xffffff, 0.35)
  scene.add(amb)

  const keyLight = new DirectionalLight(0xffffff, 1.15)
  keyLight.position.set(80, 40, 240)
  keyLight.castShadow = true
  keyLight.shadow.mapSize.set(1024, 1024)
  keyLight.shadow.camera.near = 20
  keyLight.shadow.camera.far = 500
  const d = 80
  keyLight.shadow.camera.left = -d
  keyLight.shadow.camera.right = d
  keyLight.shadow.camera.top = d
  keyLight.shadow.camera.bottom = -d
  keyLight.shadow.bias = -0.0005
  keyLight.shadow.radius = 4
  scene.add(keyLight)

  const rimLight = new DirectionalLight(0xd8c4ff, 0.45)
  rimLight.position.set(-120, 90, 80)
  scene.add(rimLight)

  orbit = new OrbitControls(camera, el)
  orbit.enableDamping = true
  orbit.dampingFactor = 0.08
  orbit.minDistance = 65
  orbit.maxDistance = 3600
  orbit.maxPolarAngle = Math.PI * 0.85

  gizmo = new TransformControls(camera, el)
  gizmo.setSize(0.85)
  gizmo.setSpace('world')
  gizmo.addEventListener('dragging-changed', (ev) => {
    gizmoDragging = Boolean(ev.value)
    if (orbit) orbit.enabled = !gizmoDragging
    const id = (gizmo?.object?.userData.imageId as string | undefined) ?? props.selectedId
    if (!id) return
    if (gizmoDragging) {
      emit('transform-start', id)
    } else {
      emit('transform-end', id)
      syncMotionTransforms()
    }
  })
  gizmo.addEventListener('objectChange', () => {
    const mesh = gizmo?.object
    if (mesh?.userData.imageId && gizmoDragging) {
      movePicture(mesh as Mesh)
      return
    }
    const id = props.selectedId
    if (!mesh || !id || !gizmoDragging) return
    if (id === LAMP_ID) {
      const at: Vec3 = [
        MathUtils.clamp(Math.round(mesh.position.x), -LAMP_REACH_MM, LAMP_REACH_MM),
        MathUtils.clamp(Math.round(mesh.position.y), -LAMP_REACH_MM, LAMP_REACH_MM),
        MathUtils.clamp(Math.round(mesh.position.z), LAMP_MIN_UP_MM, LAMP_MAX_UP_MM),
      ]
      mesh.position.set(...at)
      syncLamp()
      emit('transform-change', { id, positionMm: at, rotationDeg: [0, 0, 0], scaleMm: [1, 1, 1] })
      return
    }
    const obj = props.objects.find((o) => o.id === id)
    if (!obj) return

    // Each axis on its own; the centre handle moves all three together.
    let nextScale: Vec3 = [...obj.scaleMm] as Vec3
    if (props.mode === 'scale') {
      nextScale = [mesh.scale.x, mesh.scale.y, mesh.scale.z].map((v) => {
        const s = MathUtils.clamp(round1(v), 2, 120)
        return props.snap ? Math.max(2, Math.round(s / SNAP_SCALE_MM) * SNAP_SCALE_MM) : s
      }) as Vec3
      mesh.scale.set(...nextScale)
    }

    const pos: Vec3 = [
      MathUtils.clamp(round1(mesh.position.x), -DISC_PHYSICAL_OUTER_MM, DISC_PHYSICAL_OUTER_MM),
      MathUtils.clamp(round1(mesh.position.y), -DISC_PHYSICAL_OUTER_MM, DISC_PHYSICAL_OUTER_MM),
      MathUtils.clamp(round1(mesh.position.z), -40, 40),
    ]
    mesh.position.set(pos[0], pos[1], pos[2])

    const rot: Vec3 = [
      round1(MathUtils.radToDeg(mesh.rotation.x)),
      round1(MathUtils.radToDeg(mesh.rotation.y)),
      round1(MathUtils.radToDeg(mesh.rotation.z)),
    ]

    const entry = objectMeshes.get(id)
    if (entry) applyMotionWrapper(entry, obj)
    updateDropIndicator({ ...obj, positionMm: pos, rotationDeg: rot, scaleMm: nextScale })
    suppressSync = true
    emit('transform-change', { id, positionMm: pos, rotationDeg: rot, scaleMm: nextScale })
    suppressSync = false
  })
  scene.add(gizmo.getHelper())

  shownPhi = currentPhi()
  buildDisc()
  buildMarkers()
  buildLamp()
  ensureDropIndicator()
  applyThemeColors()
  syncGizmoModeAndSnap()
  syncObjects()
  syncPictures()
  resetCamera()

  resizeObs = new ResizeObserver(handleResize)
  resizeObs.observe(container)
  handleResize()
  animate()
})

watch(() => [props.innerRadiusMm, props.outerRadiusMm], () => {
  buildDisc()
  buildMarkers()
  syncLamp()
})
watch(() => [props.positions, props.positionId], () => {
  buildMarkers()
  updateZones()
}, { deep: true })
watch(() => [props.lampMm, props.distanceMm], () => {
  updateZones()
  syncLamp()
}, { deep: true })
watch(() => [props.mode, props.snap], () => {
  syncGizmoModeAndSnap()
  syncGizmoAttachment()
})
watch(() => props.previewT, () => syncMotionTransforms())
watch(() => [props.objects, props.meshes, props.selectedId, props.positions, props.positionId], () => syncObjects(), { deep: true })

function syncPictures() {
  pictures?.sync({
    images: props.images, positions: props.positions, positionId: props.positionId, thumbnails: props.thumbnails,
    selectedId: props.selectedImageId, innerRadiusMm: props.innerRadiusMm, outerRadiusMm: props.outerRadiusMm,
  })
  syncGizmoAttachment()
}

/**
 * A picture's mesh after the handles moved it, read back as its layer: kept flat on the disc and
 * its own shape, the size taken from whichever side was pulled the more.
 */
function movePicture(mesh: Mesh) {
  const id = mesh.userData.imageId as string
  const img = props.images.find((i) => i.id === id)
  if (!img) return
  const across = 2 * props.outerRadiusMm
  const long = Math.max(img.width, img.height, 1)
  const [w, h] = [(across * img.width) / long, (across * img.height) / long]
  const fx = mesh.scale.x / (img.size * w)
  const fy = mesh.scale.y / (img.size * h)
  const size = MathUtils.clamp(Math.round(img.size * (Math.abs(fx - 1) >= Math.abs(fy - 1) ? fx : fy) * 1000) / 1000, 0.02, 3)
  mesh.scale.set(size * w, size * h, 1)
  mesh.rotation.x = mesh.rotation.y = 0
  emit('image-transform', {
    id,
    x: Math.round((mesh.position.x / across) * 1000) / 1000,
    y: Math.round((-mesh.position.y / across) * 1000) / 1000,
    size,
    rotationDeg: round1(-MathUtils.radToDeg(mesh.rotation.z)),
  })
}
watch(() => [props.images, props.thumbnails, props.selectedImageId, props.positions, props.positionId, props.innerRadiusMm, props.outerRadiusMm], syncPictures, { deep: true })

onBeforeUnmount(() => {
  pictures?.dispose()
  cancelAnimationFrame(animFrame)
  axes?.dispose()
  resizeObs?.disconnect()
  gizmo?.detach()
  gizmo?.dispose()
  orbit?.dispose()
  for (const { mesh } of objectMeshes.values()) {
    ;(mesh.material as MeshStandardMaterial).dispose()
  }
  objectMeshes.clear()
  for (const geo of geometryCache.values()) {
    geo.dispose()
  }
  geometryCache.clear()
  ringLineMats.forEach((m) => m.dispose())
  discBaseMat?.dispose()
  shadowMat?.dispose()
  if (dropLine) {
    dropLine.geometry.dispose()
    ;(dropLine.material as LineBasicMaterial).dispose()
  }
  if (dropDot) {
    dropDot.geometry.dispose()
    ;(dropDot.material as MeshBasicMaterial).dispose()
  }
  if (lampMesh) {
    lampMesh.geometry.dispose()
    ;(lampMesh.material as MeshBasicMaterial).dispose()
  }
  if (lampCone) {
    lampCone.geometry.dispose()
    lampCone.material.dispose()
  }
  renderer?.dispose()
  // dispose() keeps the WebGL context; WebKit allows only a few, and every hot reload or remount
  // would leak one until the page stalls.
  renderer?.forceContextLoss()
  renderer = null
  scene = null
  camera = null
  orbit = null
  gizmo = null
})

defineExpose({ resetCamera, focusSelected, faceOn, faceOff })
</script>

<style scoped>
.vp3d {
  position: relative;
  width: 100%;
  height: 100%;
  overflow: hidden;
  cursor: grab;
}
.vp3d.grabbing {
  cursor: grabbing;
}
.vp3d.pointing {
  cursor: pointer;
}
.vp3d-canvas {
  display: block;
  width: 100%;
  height: 100%;
}
.vp3d-fallback {
  position: absolute;
  inset: auto var(--oc-space-md) var(--oc-space-md);
}
</style>
