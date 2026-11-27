// What a new design starts from.
import { mdiCircleOutline, mdiViewGridOutline } from '@mdi/js'
import { emptyScene, makePosition, type HoloScene } from './scene'

export interface DesignTemplate {
  id: string
  /** i18n keys. */
  name: string
  description: string
  icon: string
  make: () => HoloScene
}

/** Twelve positions, one every 15° so the twelve kept pairs tile the disc without overlap. */
function twelvePictures(): HoloScene {
  const scene = emptyScene()
  scene.positions = Array.from({ length: 12 }, (_, i) => makePosition(i * 15, 7.5))
  return scene
}

export const DESIGN_TEMPLATES: readonly DesignTemplate[] = [
  { id: 'empty', name: 'template.empty', description: 'template.emptyHelp', icon: mdiCircleOutline, make: emptyScene },
  { id: 'twelve', name: 'template.twelve', description: 'template.twelveHelp', icon: mdiViewGridOutline, make: twelvePictures },
]
