// 2D selection frame drawn on the disc plane around a selected flat picture:
// outline, four corner scale handles, four mid-edge stretch handles, and a top-edge rotate knob.
import {
  BufferGeometry,
  CircleGeometry,
  DoubleSide,
  Float32BufferAttribute,
  Group,
  Line,
  LineBasicMaterial,
  LineLoop,
  MathUtils,
  Mesh,
  MeshBasicMaterial,
  PlaneGeometry,
  Vector3,
  type PerspectiveCamera,
  type Raycaster,
} from 'three'

export type CornerHandleKind = 'tl' | 'tr' | 'br' | 'bl'
export type EdgeHandleKind = 'l' | 'r' | 't' | 'b'
export type PictureHandleKind = CornerHandleKind | EdgeHandleKind | 'rotate'

export interface PictureFrameTarget {
  id: string
  positionId: string
  /** Picture centre in its position group's XY plane (mm). */
  cx: number
  cy: number
  /** Z height of the picture mesh on the disc (mm). */
  z: number
  /** Picture dimensions on the disc (mm), stretch included. */
  widthMm: number
  heightMm: number
  /** Clockwise rotation in degrees, matching HoloImage.rotationDeg. */
  rotationDeg: number
}

const ACCENT_HEX = 0x9045f9
const HOVER_HEX = 0xae79fb
const FILL_HEX = 0xffffff
const FRAME_Z_LIFT = 0.04
const MIN_HANDLE_MM = 1.4
const MAX_HANDLE_MM = 7
const HANDLE_VIEW_RATIO = 0.032
const STEM_RATIO = 2.8
const HIT_SCALE = 1.85

const CORNERS: readonly { kind: CornerHandleKind; sx: number; sy: number }[] = [
  { kind: 'tl', sx: -1, sy: 1 },
  { kind: 'tr', sx: 1, sy: 1 },
  { kind: 'br', sx: 1, sy: -1 },
  { kind: 'bl', sx: -1, sy: -1 },
]

const EDGES: readonly { kind: EdgeHandleKind; sx: number; sy: number; horizontal: boolean }[] = [
  { kind: 't', sx: 0, sy: 1, horizontal: true },
  { kind: 'b', sx: 0, sy: -1, horizontal: true },
  { kind: 'l', sx: -1, sy: 0, horizontal: false },
  { kind: 'r', sx: 1, sy: 0, horizontal: false },
]

interface HandleVisual {
  kind: PictureHandleKind
  group: Group
  borderMat: MeshBasicMaterial
  hitMesh: Mesh
}

/**
 * Create the reusable 2D manipulation frame for the currently selected flat picture.
 * Visual meshes render above pictures without writing depth and disable raycasting;
 * only the invisible handle hit meshes participate in `pickHandle()`.
 */
export function createPictureFrame(groupFor: (positionId: string) => Group) {
  const root = new Group()
  root.visible = false

  const lineMat = new LineBasicMaterial({
    color: ACCENT_HEX,
    depthTest: false,
    depthWrite: false,
  })
  const fillMat = new MeshBasicMaterial({
    color: FILL_HEX,
    side: DoubleSide,
    depthTest: false,
    depthWrite: false,
  })
  const hitMat = new MeshBasicMaterial({
    transparent: true,
    opacity: 0,
    colorWrite: false,
    side: DoubleSide,
    depthTest: false,
    depthWrite: false,
  })

  const outlineGeo = new BufferGeometry()
  outlineGeo.setAttribute('position', new Float32BufferAttribute(new Float32Array(4 * 3), 3))
  const outline = new LineLoop(outlineGeo, lineMat)
  outline.renderOrder = 50
  outline.raycast = () => {}
  root.add(outline)

  const stemGeo = new BufferGeometry()
  stemGeo.setAttribute('position', new Float32BufferAttribute(new Float32Array(2 * 3), 3))
  const stem = new Line(stemGeo, lineMat)
  stem.renderOrder = 50
  stem.raycast = () => {}
  root.add(stem)

  const squareOuterGeo = new PlaneGeometry(1, 1)
  const squareInnerGeo = new PlaneGeometry(0.68, 0.68)
  const edgeHOuterGeo = new PlaneGeometry(1.15, 0.56)
  const edgeHInnerGeo = new PlaneGeometry(0.84, 0.28)
  const edgeVOuterGeo = new PlaneGeometry(0.56, 1.15)
  const edgeVInnerGeo = new PlaneGeometry(0.28, 0.84)
  const circleOuterGeo = new CircleGeometry(0.6, 24)
  const circleInnerGeo = new CircleGeometry(0.42, 24)
  const circleHitGeo = new CircleGeometry(0.6 * HIT_SCALE, 16)

  const handles = new Map<PictureHandleKind, HandleVisual>()
  const hitMeshes: Mesh[] = []

  function addHandle(
    kind: PictureHandleKind,
    outerGeo: BufferGeometry,
    innerGeo: BufferGeometry,
    hitGeo: BufferGeometry,
    hitScaleX: number,
    hitScaleY: number,
  ): HandleVisual {
    const group = new Group()
    const borderMat = new MeshBasicMaterial({
      color: ACCENT_HEX,
      side: DoubleSide,
      depthTest: false,
      depthWrite: false,
    })
    const border = new Mesh(outerGeo, borderMat)
    border.renderOrder = 51
    border.raycast = () => {}

    const fill = new Mesh(innerGeo, fillMat)
    fill.renderOrder = 52
    fill.raycast = () => {}

    const hitMesh = new Mesh(hitGeo, hitMat)
    hitMesh.scale.set(hitScaleX, hitScaleY, 1)
    hitMesh.userData.frameHandle = kind
    hitMesh.renderOrder = 53

    group.add(border, fill, hitMesh)
    root.add(group)
    const visual: HandleVisual = { kind, group, borderMat, hitMesh }
    handles.set(kind, visual)
    hitMeshes.push(hitMesh)
    return visual
  }

  // Register knob, then corners, then edges so corners and knob stay easy to grab on tiny pictures.
  const knob = addHandle('rotate', circleOuterGeo, circleInnerGeo, circleHitGeo, 1, 1)
  for (const c of CORNERS) {
    addHandle(c.kind, squareOuterGeo, squareInnerGeo, squareOuterGeo, HIT_SCALE, HIT_SCALE)
  }
  for (const e of EDGES) {
    if (e.horizontal) {
      addHandle(e.kind, edgeHOuterGeo, edgeHInnerGeo, edgeHOuterGeo, 1.4, 2.4)
    } else {
      addHandle(e.kind, edgeVOuterGeo, edgeVInnerGeo, edgeVOuterGeo, 2.4, 1.4)
    }
  }

  let halfW = 10
  let halfH = 10
  let handleSize = 2.5
  let hovered: PictureHandleKind | null = null

  const worldPos = new Vector3()
  const centerNdc = new Vector3()
  const handleNdc = new Vector3()

  function updateHandleScales() {
    for (const h of handles.values()) {
      const boost = h.kind === hovered ? 1.15 : 1
      const s = handleSize * boost
      h.group.scale.set(s, s, 1)
      h.borderMat.color.setHex(h.kind === hovered ? HOVER_HEX : ACCENT_HEX)
    }
  }

  function updateGeometry() {
    const outlinePos = outlineGeo.getAttribute('position') as Float32BufferAttribute
    outlinePos.setXYZ(0, -halfW, -halfH, 0)
    outlinePos.setXYZ(1, halfW, -halfH, 0)
    outlinePos.setXYZ(2, halfW, halfH, 0)
    outlinePos.setXYZ(3, -halfW, halfH, 0)
    outlinePos.needsUpdate = true
    outlineGeo.computeBoundingSphere()

    for (const c of CORNERS) {
      handles.get(c.kind)!.group.position.set(c.sx * halfW, c.sy * halfH, 0)
    }
    for (const e of EDGES) {
      handles.get(e.kind)!.group.position.set(e.sx * halfW, e.sy * halfH, 0)
    }

    const stemTop = halfH + handleSize * STEM_RATIO
    const stemPos = stemGeo.getAttribute('position') as Float32BufferAttribute
    stemPos.setXYZ(0, 0, halfH, 0)
    stemPos.setXYZ(1, 0, stemTop, 0)
    stemPos.needsUpdate = true
    stemGeo.computeBoundingSphere()

    knob.group.position.set(0, stemTop, 0)
    updateHandleScales()
  }

  function sync(target: PictureFrameTarget | null) {
    if (!target) {
      root.visible = false
      if (hovered !== null) {
        hovered = null
        updateHandleScales()
      }
      return
    }
    const parent = groupFor(target.positionId)
    if (root.parent !== parent) parent.add(root)
    halfW = Math.max(target.widthMm / 2, 0.25)
    halfH = Math.max(target.heightMm / 2, 0.25)
    root.position.set(target.cx, target.cy, target.z + FRAME_Z_LIFT)
    root.rotation.set(0, 0, -MathUtils.degToRad(target.rotationDeg))
    root.scale.set(1, 1, 1)
    root.visible = true
    updateGeometry()
    root.updateMatrixWorld(true)
  }

  function updateForCamera(camera: PerspectiveCamera) {
    if (!root.visible) return
    root.getWorldPosition(worldPos)
    const dist = camera.position.distanceTo(worldPos)
    const halfView = dist * Math.tan(MathUtils.degToRad(camera.fov / 2))
    const nextSize = MathUtils.clamp(halfView * HANDLE_VIEW_RATIO, MIN_HANDLE_MM, MAX_HANDLE_MM)
    if (Math.abs(nextSize - handleSize) < 0.02) return
    handleSize = nextSize
    updateGeometry()
  }

  function pickHandle(raycaster: Raycaster): PictureHandleKind | null {
    if (!root.visible) return null
    root.updateMatrixWorld(true)
    const hits = raycaster.intersectObjects(hitMeshes, false)
    return (hits[0]?.object.userData.frameHandle as PictureHandleKind | undefined) ?? null
  }

  function setHovered(kind: PictureHandleKind | null) {
    if (kind === hovered) return
    hovered = kind
    updateHandleScales()
  }

  /** Pick nwse or nesw resize cursor based on the corner's on-screen diagonal from the picture centre. */
  function cornerCursor(kind: CornerHandleKind, camera: PerspectiveCamera): 'nwse' | 'nesw' {
    root.updateMatrixWorld(true)
    root.getWorldPosition(centerNdc).project(camera)
    handles.get(kind)!.group.getWorldPosition(handleNdc).project(camera)
    const dx = handleNdc.x - centerNdc.x
    const dy = -(handleNdc.y - centerNdc.y)
    return dx * dy >= 0 ? 'nwse' : 'nesw'
  }

  /** Pick ew or ns resize cursor based on the edge handle's on-screen direction from the picture centre. */
  function edgeCursor(kind: EdgeHandleKind, camera: PerspectiveCamera): 'ew' | 'ns' {
    root.updateMatrixWorld(true)
    root.getWorldPosition(centerNdc).project(camera)
    handles.get(kind)!.group.getWorldPosition(handleNdc).project(camera)
    const dx = Math.abs(handleNdc.x - centerNdc.x)
    const dy = Math.abs(handleNdc.y - centerNdc.y)
    return dx >= dy ? 'ew' : 'ns'
  }

  function dispose() {
    root.removeFromParent()
    outlineGeo.dispose()
    stemGeo.dispose()
    squareOuterGeo.dispose()
    squareInnerGeo.dispose()
    edgeHOuterGeo.dispose()
    edgeHInnerGeo.dispose()
    edgeVOuterGeo.dispose()
    edgeVInnerGeo.dispose()
    circleOuterGeo.dispose()
    circleInnerGeo.dispose()
    circleHitGeo.dispose()
    lineMat.dispose()
    fillMat.dispose()
    hitMat.dispose()
    for (const h of handles.values()) h.borderMat.dispose()
    handles.clear()
    hitMeshes.length = 0
  }

  return { sync, updateForCamera, pickHandle, setHovered, cornerCursor, edgeCursor, dispose }
}
