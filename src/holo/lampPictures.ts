// Small pictures of the disc under each lamp preset, for the new-design wizard: the lamp's glare
// band hatched by the same shader the editor uses, drawn once and kept.
import { Color, Mesh, MeshBasicMaterial, OrthographicCamera, RingGeometry, Scene, Vector3, WebGLRenderer } from 'three'
import { DEFAULT_DATA_INNER_MM, DEFAULT_DATA_OUTER_MM, DEFAULT_DISTANCE_MM, DISC_HOLE_RADIUS_MM, DISC_PHYSICAL_OUTER_MM, type Vec3 } from './constants'
import { ZONE_KIND_VALUE, zonesMaterial } from './usability'

const cache = new Map<string, string>()

/** One PNG data URL per lamp, `px` across, of the disc seen from the eye with its glare band hatched. */
export function lampPictures(lamps: Record<string, Vec3>, px: number): Record<string, string> {
  const missing = Object.entries(lamps).filter(([id, at]) => !cache.has(`${id}:${at}:${px}`))
  if (missing.length) {
    const canvas = document.createElement('canvas')
    const renderer = new WebGLRenderer({ canvas, antialias: true, alpha: true, preserveDrawingBuffer: true })
    renderer.setPixelRatio(1)
    renderer.setSize(px, px, false)
    renderer.setClearColor(new Color(0), 0)
    const r = DISC_PHYSICAL_OUTER_MM
    const camera = new OrthographicCamera(-r, r, r, -r, -10, 10)
    const scene = new Scene()
    const base = new Mesh(new RingGeometry(DISC_HOLE_RADIUS_MM, DISC_PHYSICAL_OUTER_MM, 128), new MeshBasicMaterial({ color: 0x2a2c38 }))
    const data = new Mesh(new RingGeometry(DEFAULT_DATA_INNER_MM, DEFAULT_DATA_OUTER_MM, 128), new MeshBasicMaterial({ color: 0x3a3d4c }))
    const zones = zonesMaterial()
    zones.uniforms.kind.value = ZONE_KIND_VALUE.none
    zones.uniforms.distance.value = DEFAULT_DISTANCE_MM
    zones.uniforms.hatchMm.value = 5
    const glare = new Mesh(new RingGeometry(DEFAULT_DATA_INNER_MM, DEFAULT_DATA_OUTER_MM, 128), zones)
    data.position.z = 0.01
    glare.position.z = 0.02
    scene.add(base, data, glare)
    try {
      for (const [id, at] of missing) {
        ;(zones.uniforms.lamp.value as Vector3).set(...at)
        renderer.render(scene, camera)
        cache.set(`${id}:${at}:${px}`, canvas.toDataURL('image/png'))
      }
    } finally {
      for (const m of [base, data, glare]) {
        m.geometry.dispose()
        ;(m.material as MeshBasicMaterial).dispose()
      }
      renderer.dispose()
      renderer.forceContextLoss()
    }
  }
  return Object.fromEntries(Object.entries(lamps).map(([id, at]) => [id, cache.get(`${id}:${at}:${px}`) ?? '']))
}
