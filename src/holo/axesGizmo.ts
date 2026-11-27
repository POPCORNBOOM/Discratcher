import {
  BufferGeometry,
  CanvasTexture,
  Float32BufferAttribute,
  Line,
  LineBasicMaterial,
  OrthographicCamera,
  Scene,
  Sprite,
  SpriteMaterial,
  Vector2,
  type Camera,
  type WebGLRenderer,
} from 'three'

/** The same red, green and blue as the move handles. */
const AXES: { label: string; dir: [number, number, number]; color: number; css: string }[] = [
  { label: 'X', dir: [1, 0, 0], color: 0xff0000, css: '#ff0000' },
  { label: 'Y', dir: [0, 1, 0], color: 0x00ff00, css: '#00ff00' },
  { label: 'Z', dir: [0, 0, 1], color: 0x0000ff, css: '#0000ff' },
]
/** Size of the corner it is drawn in, and its gap from the edges, CSS pixels. */
const SIZE = 72
const MARGIN = 8

/**
 * A small set of scene axes drawn in the bottom-left corner after the scene, turned as the view
 * is: x right, y toward 12 o'clock, z toward the viewer.
 */
export function createAxesGizmo() {
  const scene = new Scene()
  const camera = new OrthographicCamera(-1.6, 1.6, 1.6, -1.6, 0.1, 10)
  const owned: { dispose(): void }[] = []
  for (const a of AXES) {
    const geometry = new BufferGeometry()
    geometry.setAttribute('position', new Float32BufferAttribute([0, 0, 0, ...a.dir], 3))
    const material = new LineBasicMaterial({ color: a.color, depthTest: false })
    scene.add(new Line(geometry, material))

    const canvas = document.createElement('canvas')
    canvas.width = canvas.height = 64
    const g = canvas.getContext('2d')!
    g.fillStyle = a.css
    g.font = '600 44px system-ui, sans-serif'
    g.textAlign = 'center'
    g.textBaseline = 'middle'
    g.fillText(a.label, 32, 34)
    const texture = new CanvasTexture(canvas)
    const spriteMaterial = new SpriteMaterial({ map: texture, depthTest: false })
    const label = new Sprite(spriteMaterial)
    label.position.set(a.dir[0] * 1.3, a.dir[1] * 1.3, a.dir[2] * 1.3)
    label.scale.setScalar(0.55)
    scene.add(label)
    owned.push(geometry, material, texture, spriteMaterial)
  }
  const size = new Vector2()

  return {
    /** Draw over what `renderer` last drew, facing the way `view` faces. */
    render(renderer: WebGLRenderer, view: Camera) {
      camera.quaternion.copy(view.quaternion)
      camera.position.set(0, 0, 5).applyQuaternion(view.quaternion)
      camera.updateMatrixWorld()
      const autoClear = renderer.autoClear
      renderer.autoClear = false
      renderer.setScissorTest(true)
      renderer.setScissor(MARGIN, MARGIN, SIZE, SIZE)
      renderer.setViewport(MARGIN, MARGIN, SIZE, SIZE)
      renderer.clearDepth()
      renderer.render(scene, camera)
      renderer.setScissorTest(false)
      renderer.getSize(size)
      renderer.setViewport(0, 0, size.x, size.y)
      renderer.autoClear = autoClear
    },
    dispose() {
      for (const o of owned) o.dispose()
    },
  }
}
