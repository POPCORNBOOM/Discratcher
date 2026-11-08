import { defineStore } from 'pinia'
import { computed, ref, shallowRef, watch } from 'vue'
import { call, CANCELLED } from '../api'
import { notifyError } from '../notices'
import type { ArtField } from '../discRenderer'
import type { CarveJobConfig, DiscGeometryPreset, OpticalDriveInfo } from '../types'

export type View = 'home' | 'models' | 'calibrate' | 'carve' | 'process'

/** What the menu bar can ask of the carve page. */
export interface CarveCommands {
  choose: () => void
  exportIso: () => void
  burn: () => void
}

export const useStudio = defineStore('studio', () => {
  const view = ref<View>('home')
  const drives = ref<OpticalDriveInfo[]>([])
  const presets = ref<DiscGeometryPreset[]>([])
  const driveKey = ref('')
  const presetId = ref('')
  const error = ref('')
  /** Name of the long job in flight; the machine has one drive and one output at a time. */
  const job = ref('')
  const carve = shallowRef<CarveCommands | null>(null)
  /** The code the user searched for when calibration was entered from a failed search. */
  const pendingCode = ref('')
  /** A result being pointed at: the disc previews it before anything is chosen. */
  const discPreview = shallowRef<DiscGeometryPreset | null>(null)
  /** The picture being carved, drawn on the disc itself; DiscStage places it from `config`. */
  const discArt = shallowRef<ArtField | null>(null)
  const discZoom = ref(1)
  const discPan = ref({ x: 0, y: 0 })
  /** The code field has focus: the disc comes forward and aims at its stamped code. */
  const codeFocus = ref(false)
  /** False where WebGL is unavailable, so the page falls back to a plain image. */
  const discRendered = ref(false)

  const drive = computed(() => drives.value.find((d) => keyOf(d) === driveKey.value) ?? null)
  const preset = computed(() => presets.value.find((p) => p.id === presetId.value) ?? presets.value[0] ?? null)

  const config = ref({
    sourceImagePath: '',
    outputIsoPath: '',
    invertImage: false,
    rotationDeg: 0,
    cropMode: 'square-center' as CarveJobConfig['cropMode'],
    contrast: 0,
    servoSafeDither: true,
    maxDarkDutyCycle: 0.65,
    offsetX: 0,
    offsetY: 0,
    scale: 1,
  })

  // A disc whose MID names exactly one configuration needs no choosing.
  watch(
    () => drive.value?.mediaId,
    (id) => {
      const hits = presets.value.filter((p) => p.mediaId === id)
      if (id && hits.length === 1) presetId.value = hits[0].id
    },
  )

  const carveReady = computed(() => !!config.value.sourceImagePath && !!preset.value && !job.value)

  // Every route into a page is recorded, so one back button serves the whole app.
  const trail = ref<View[]>([])
  const canBack = computed(() => trail.value.length > 0)
  const origin = computed(() => trail.value[trail.value.length - 1] ?? 'home')
  let returning = false
  watch(
    view,
    (_, previous) => {
      if (!returning) trail.value.push(previous)
      returning = false
      // A hovered row can vanish without ever reporting that the pointer left it.
      discPreview.value = null
      // Leaving the page drops the code field's focus without a blur event.
      codeFocus.value = false
    },
    { flush: 'sync' },
  )
  function back() {
    const to = trail.value.pop()
    if (!to) return
    returning = true
    view.value = to
  }

  function keyOf(d: OpticalDriveInfo) {
    return d.id || `${d.vendor}/${d.product}`
  }

  async function refreshDrives() {
    drives.value = await call<OpticalDriveInfo[]>('list_optical_drives')
    if (!drive.value && drives.value.length) driveKey.value = keyOf(drives.value[0])
  }

  async function refreshPresets() {
    presets.value = await call<DiscGeometryPreset[]>('get_presets')
    if (!preset.value && presets.value.length) presetId.value = presets.value[0].id
    if (!presetId.value && presets.value.length) presetId.value = presets.value[0].id
  }

  /** Runs a job, surfacing failure in the title bar. A cancel is not a failure. */
  async function run<T>(name: string, f: () => Promise<T>): Promise<T | null> {
    if (job.value) return null
    job.value = name
    error.value = ''
    try {
      return await f()
    } catch (e) {
      const m = String(e)
      if (m !== CANCELLED) {
        error.value = m
        notifyError(m)
      }
      return null
    } finally {
      job.value = ''
    }
  }

  return { view, drives, presets, driveKey, presetId, drive, preset, config, error, job, carve, carveReady, canBack, origin, back, pendingCode, discPreview, discArt, discZoom, discPan, discRendered, codeFocus, keyOf, refreshDrives, refreshPresets, run }
})
