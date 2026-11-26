import { defineStore } from 'pinia'
import { computed, ref, shallowRef, watch } from 'vue'
import { call, CANCELLED } from '../api'
import { normalizeCode } from '../catalog'
import { isCd } from '../etaModel'
import { notifyError } from '../notices'
import { useComposition } from './composition'
import type { DiscGeometryPreset, OpticalDriveInfo, PresetLook } from '../types'

export type View = 'home' | 'models' | 'calibrate' | 'carve' | 'holo' | 'process'

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
  /** The system stopped answering about drives, so an empty list proves nothing. */
  const driveStuck = ref(false)
  /** The drive is busy with a disc it struggles to read and is being left alone a while. */
  const driveBusy = ref(false)
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

  // A disc whose MID names exactly one configuration needs no choosing.
  watch(
    () => drive.value?.mediaId,
    (id) => {
      if (!id) return
      // The disc names itself and the presets are named the way the discs print
      // them, so the two are compared without their spacing: "RITEK F16" and
      // "RITEKF16" are the same disc.
      const key = normalizeCode(id)
      const hits = presets.value.filter((p) => normalizeCode(p.mediaId) === key)
      if (hits.length === 1) presetId.value = hits[0].id
    },
  )

  // A calibration for the other kind of disc can only fail, so when the disc in the drive
  // changes kind and exactly one calibration fits it, that one is selected.
  watch(
    // The calibrations may load after the drive is first seen.
    [() => (drive.value && drive.value.disc !== 'none' && drive.value.disc !== 'unsupported' ? isCd(drive.value) : null), () => presets.value.length] as const,
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

  /** The disc in the drive can be written over, so a bad burn costs time rather than a disc. */

  function keyOf(d: OpticalDriveInfo) {
    return d.id || `${d.vendor}/${d.product}`
  }

  /** Answers in a row without a drive before it counts as gone. */
  const MISSES_BEFORE_GONE = 3
  let misses = 0
  /** `fresh` asks the drive about the disc even when it is the one already known: worth it
   *  for a user's refresh or the end of a job, not for the poll, which a slow disc would cost. */
  async function refreshDrives(fresh = false) {
    let found: OpticalDriveInfo[] = []
    let stuck = false
    try {
      found = await call<OpticalDriveInfo[]>('list_optical_drives', { fresh })
      driveBusy.value = false
    } catch (e) {
      // Busy with its disc: the drive and disc are still what they were.
      if (String(e) === 'busy') {
        driveBusy.value = true
        if (drives.value.length) return
      } else {
        // The system stopped answering about drives: the drive may well be plugged in.
        stuck = true
      }
    }
    // A drive busy with a bad sector, or just let go by a job, misses an answer or two
    // without being gone: keep what was known, and only believe it gone when it stays so.
    if (!found.length && drives.value.length && ++misses < MISSES_BEFORE_GONE) return
    if (found.length) misses = 0
    drives.value = found
    driveStuck.value = stuck
    // A real drive's key changes with its disc, and the drive can drop out of the list
    // for a moment while one is swapped.
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
        // The process page shows its own failure; the title bar only speaks when it is not in view.
        if (view.value !== 'process') notifyError(m)
      }
      return null
    } finally {
      job.value = ''
    }
  }

  return { view, drives, driveStuck, driveBusy, presets, driveKey, presetId, drive, preset, config, error, job, carve, carveReady, canBack, origin, back, pendingCode, discPreview, discZoom, discPan, discRendered, codeFocus, keyOf, refreshDrives, refreshPresets, run }
})
