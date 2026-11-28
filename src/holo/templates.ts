// What a new design starts from.
import { mdiCircleOutline, mdiRotateRight, mdiViewGridOutline } from '@mdi/js'
import { emptyScene, makePosition, makeSectorCrop, type HoloScene } from './scene'

export interface DesignTemplate {
  id: string
  /** i18n keys. */
  name: string
  description: string
  icon: string
  make: () => HoloScene
}

/** Twelve positions, one every 30°, each cropped to ±15° at its top: the twelve tile the disc. */
function twelvePictures(): HoloScene {
  const scene = emptyScene()
  scene.positions = Array.from({ length: 12 }, (_, i) => makePosition(i * 30, [makeSectorCrop(15, 'up', 0)]))
  return scene
}

/** One spinning position over the whole ring: a colour picture that stays right as the disc turns. */
function spinPicture(): HoloScene {
  const scene = emptyScene()
  scene.positions = [{ ...makePosition(0, []), spin: true }]
  return scene
}

export const DESIGN_TEMPLATES: readonly DesignTemplate[] = [
  { id: 'empty', name: 'template.empty', description: 'template.emptyHelp', icon: mdiCircleOutline, make: emptyScene },
  { id: 'spin', name: 'template.spin', description: 'template.spinHelp', icon: mdiRotateRight, make: spinPicture },
  { id: 'twelve', name: 'template.twelve', description: 'template.twelveHelp', icon: mdiViewGridOutline, make: twelvePictures },
]
