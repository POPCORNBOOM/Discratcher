import { defineStore } from 'pinia'
import { computed, ref, shallowRef, watch } from 'vue'
import { call, CANCELLED } from '../api'
import { isCd } from '../etaModel'
import { notifyError } from '../notices'
import { useComposition } from './composition'
import type { DiscGeometryPreset, OpticalDriveInfo, PresetLook } from '../types'

export type View = 'home' | 'models' | 'calibrate' | 'carve' | 'process'

/** What the menu bar can ask of the carve page. */
export interface CarveCommands {
  /** Adds a picture to the composition. */
  choose: () => void
  exportIso: () => void
  burn: () => void
  newComposition: () => void
  openComposition: () => void
  save: () => void
  saveAs: () => void
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
  const discPreview = shallowRef<PresetLook | null>(null)
  const discZoom = ref(1)
  const discPan = ref({ x: 0, y: 0 })
  /** The code field has focus: the disc comes forward and aims at its stamped code. */
  const codeFocus = ref(false)
  /** False where WebGL is unavailable, so the page falls back to a plain image. */
  const discRendered = ref(false)

  const drive = computed(() => drives.value.find((d) => keyOf(d) === driveKey.value) ?? null)
  const preset = computed(() => presets.value.find((p) => p.id === presetId.value) ?? presets.value[0] ?? null)

  /** The burn settings; the pictures live in the composition store. */
  const config = ref({
    servoSafeDither: true,
    maxDarkDutyCycle: 0.65,
  })

  // A disc whose MID names exactly one configuration needs no choosing. Not for the
  // virtual drive: the list falls back to it whenever the real one drops out for a
  // moment, as it does while a disc is swapped, and its preset then replaced the
  // user's own just before a burn.
  watch(
    () => (drive.value?.simulated ? '' : drive.value?.mediaId),
    (id) => {
      const hits = presets.value.filter((p) => p.mediaId === id)
      if (id && hits.length === 1) presetId.value = hits[0].id
    },
  )

  // A calibration for the other kind of disc can only fail, so when the disc in the drive
  // changes kind and exactly one calibration fits it, that one is selected.
  watch(
    // The calibrations may load after the drive is first seen.
    [() => (drive.value?.mediaPresent && drive.value.supported && !drive.value.simulated ? isCd(drive.value) : null), () => presets.value.length] as const,
    ([cd]) => {
      if (cd === null || (preset.value?.media === 'cd') === cd) return
      const fits = presets.value.filter((p) => (p.media === 'cd') === cd)
      if (fits.length === 1) presetId.value = fits[0].id
    },
  )

  const composition = useComposition()
  const carveReady = computed(() => composition.layers.length > 0 && !!preset.value && !job.value)

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

  /** The virtual drive is selected only because no real one was listed. */
  let stoodIn = false
  async function refreshDrives() {
    drives.value = await call<OpticalDriveInfo[]>('list_optical_drives')
    // A real drive's key changes with its disc, and the drive can drop out of the list
    // for a moment while one is swapped. The virtual drive may stand in then, but only
    // until a real one is back: left selected, it sent a calibration to nowhere.
    const real = drives.value.find((d) => !d.simulated)
    if (!drive.value && drives.value.length) {
      driveKey.value = keyOf(real ?? drives.value[0])
      stoodIn = !real
    } else if (stoodIn && real) {
      driveKey.value = keyOf(real)
      stoodIn = false
    }
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

  return { view, drives, presets, driveKey, presetId, drive, preset, config, error, job, carve, carveReady, canBack, origin, back, pendingCode, discPreview, discZoom, discPan, discRendered, codeFocus, keyOf, refreshDrives, refreshPresets, run }
})
