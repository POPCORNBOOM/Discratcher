// Development stand-in for the native commands. Never bundled into the Tauri app.
import type { CatalogEntry } from './catalog'
import type { DiscGeometryPreset } from './types'
const handlers = new Map<string, Set<(p: unknown) => void>>()
const emit = (e: string, p: unknown) => handlers.get(e)?.forEach((h) => h(p))
export function on(e: string, cb: (p: unknown) => void) {
  if (!handlers.has(e)) handlers.set(e, new Set())
  handlers.get(e)!.add(cb)
  return () => handlers.get(e)!.delete(cb)
}
// Set from the console to try each state: 'none', 'blank' or 'written'.
declare global { var __mockDisc: string | undefined }
const media = () => globalThis.__mockDisc ?? 'none'
/** Browser-only: put a disc in the mock drive so the whole flow can run. */
export function setMockDisc(state: 'none' | 'blank' | 'written' | 'cd') {
  globalThis.__mockDisc = state
}
const wait = (ms: number) => new Promise((r) => setTimeout(r, ms))
const demo = {
  id: 'mock-reference',
  driveModel: 'HL-DT-ST DVDRAM GUD1N 1.01', mediaId: 'CMC MAG. AM3', stamperCode: 'CMDR47G-CFMWMO3-0237 L110',
  totalSectors: 2295104, channelBitLengthNm: 133.3, innerRadiusMm: 24.173325346, outerRadiusMm: 57.878686008,
  pitchLinear: -1.85716e-5, pitchQuadratic: 1.191945e-4, pitchCubic: -1.373621e-4, basePitchUm: 0.7340606,
  rmsResidualDeg: 0.402, measuredSpanMm: [25.292, 56.914], createdAt: '2026-11-06T21:53:00+08:00',
}
let saved: { id: string }[] = []
let log: { id: string }[] = []
let stop = false
const compositions = new Map<string, unknown>()
const disc = (size: number, dark: number) => {
  const c = document.createElement('canvas')
  c.width = c.height = size
  const g = c.getContext('2d')!
  const s = size / 2
  g.translate(s, s)
  g.fillStyle = '#e8e8e8'
  g.beginPath(); g.arc(0, 0, s * 0.98, 0, 7); g.fill()
  for (let i = 0; i < 12; i++) {
    g.rotate(Math.PI / 6)
    g.fillStyle = `rgb(${dark},${dark},${dark})`
    g.beginPath(); g.ellipse(s * 0.62, 0, s * 0.22, s * 0.06, 0, 0, 7); g.fill()
  }
  g.globalCompositeOperation = 'destination-out'
  g.beginPath(); g.arc(0, 0, s * 0.25, 0, 7); g.fill()
  return c.toDataURL()
}
// Set from the console to try a failed connection.
declare global { var __mockOffline: boolean | undefined; var __mockCalibrationFails: boolean | undefined }
/** Contributions "opened on GitHub" while in the browser, so the library can return them. */
const contributed: DiscGeometryPreset[] = []
const library = () => [{ ...demo, id: 'issue-1', mediaId: 'DEMO', brand: 'Verbatim', basePitchUm: 0.7401, stamperCode: 'TEST-0001 A', measuredSpanMm: [25.3, 56.9] as [number, number] } as DiscGeometryPreset, ...contributed]
/** The published index: search fields only, the way the workflow splits it. */
export async function index(): Promise<CatalogEntry[]> {
  await wait(300)
  if (globalThis.__mockOffline) throw new Error('offline')
  const [demo, ...mine] = library()
  const entry = (p: DiscGeometryPreset, contributor: string, rating: CatalogEntry['rating']): CatalogEntry => ({
    id: p.id, stamperCode: p.stamperCode, mediaId: p.mediaId, brand: p.brand ?? '', basePitchUm: p.basePitchUm,
    size: p.outerRadiusMm > 45 ? 12 : 8, createdAt: p.createdAt, contributor, verified: true, rating,
  })
  return [entry(demo, 'demo-user', { average: 4.5, count: 6 }), ...mine.map((p) => entry(p, 'you', null))]
}
/** The published folder for one calibration: everything the app needs to burn it. */
export async function preset(id: string): Promise<DiscGeometryPreset> {
  await wait(200)
  if (globalThis.__mockOffline) throw new Error('offline')
  const hit = library().find((p) => p.id === id)
  if (!hit) throw new Error('HTTP 404')
  return hit
}
declare global { var __mockOpened: string | undefined }
export const openUrl = (url: string) => {
  globalThis.__mockOpened = url
  // A contribution is a pre-filled issue form. Keep what it carries so a later search
  // finds it: the point of the browser flow is to walk calibration → contribution.
  try {
    const raw = new URL(url).searchParams.get('data')
    if (!raw) return
    const data = JSON.parse(raw)
    if ('score' in data) return
    if (contributed.some((p) => p.stamperCode === data.stamperCode)) return
    contributed.push({ ...data, id: `issue-mock-${contributed.length + 1}` } as DiscGeometryPreset)
  } catch {
    // A malformed form is the caller's bug; the flow itself still succeeded.
  }
}
export async function call(cmd: string, a: Record<string, any>): Promise<unknown> {
  switch (cmd) {
    case 'list_optical_drives':
      return [{ id: '', vendor: 'HL-DT-ST', product: 'DVDRAM GUD1N', revision: '1.01', mediaPresent: media() !== 'none', mediaType: media() === 'none' ? 'None' : media() === 'cd' ? 'CD-R' : 'DVD-R', mediaId: media() === 'none' ? '' : 'CMC MAG. AM3', totalSectors: 2295104, isBlank: media() === 'blank' || media() === 'cd', simulated: false, supported: media() === 'blank' || media() === 'written' }]
    case 'inspect_disc':
      return ({ none: 'none', cd: 'unsupported', blank: 'blank', written: 'calibration' } as Record<string, string>)[media()] ?? 'none'
    case 'check_calibration_disc':
      return { sectors: 2295104, foreign: false }
    case 'get_presets': return saved
    case 'save_preset': saved = [...saved.filter((p) => p.id !== a.preset.id), a.preset]; return null
    case 'delete_preset': saved = saved.filter((p) => p.id !== a.id); return null
    case 'analyze_artwork':
      return { width: 2048, height: String(a.imagePath).includes('wide') ? 1024 : 2048, darkRatio: 0.41, suggestInvert: true, maxBlackRunPx: 86, thumbnail: disc(256, 30) }
    case 'preview_texture': {
      // An "F" over a gradient, so turns and mirroring are easy to see; "wide" pictures are 2:1.
      const n = 512, d = new Uint8Array(n * n * 2), wide = String(a.imagePath).includes('wide')
      for (let y = 0; y < n; y++) for (let x = 0; x < n; x++) {
        const i = (y * n + x) * 2
        const f = (x > 120 && x < 180 && y > 100 && y < 420) || (x > 120 && x < 400 && y > 100 && y < 160) || (x > 120 && x < 330 && y > 240 && y < 290)
        d[i] = f ? 0 : wide ? 255 - (x / n) * 135 : 120 + (x / n) * 135
        d[i + 1] = wide && Math.hypot(x - 256, y - 256) > 250 ? 0 : 255
      }
      await wait(30)
      return d.buffer
    }
    case 'save_composition': compositions.set(a.path, JSON.stringify(a.layers)); return null
    case 'open_composition': {
      const c = compositions.get(a.path)
      if (!c) throw '不是 Discratcher 构图文件'
      return JSON.parse(c as string)
    }
    case 'simulate_disc_preview':
      await wait(60)
      return disc(a.previewSize, a.config.servoSafeDither ? 90 : 0)
    case 'build_carved_iso': {
      for (let p = 0; p <= 100; p += 5) { emit('carve://progress', { percent: p, mbPerS: 512, maxSectorDarkRatio: 0.652 }); await wait(40) }
      return { summary: { outputIsoPath: a.config.outputIsoPath, totalSectors: 2295104, bytes: 4700372992, elapsedSeconds: 9.2, mbPerS: 511, meanDarkRatio: 0.21, maxSectorDarkRatio: 0.652, maxDarkRunBytes: 2, servoSafe: true, limitOk: true }, audit: { sectors: 2295104, foreignBytes: 0, maxSectorDarkRatio: 0.652, worstLba: 912344 } }
    }
    case 'start_calibration': {
      const rungs = []
      stop = false
      for (let i = 0; i < a.maxTargets; i++) {
        if (stop) { stop = false; throw '已取消' }
        if (globalThis.__mockCalibrationFails && i === 3) throw '主轴不稳定'
        const x = i / (a.maxTargets - 1)
        const e = { rungIndex: i, totalRungs: a.maxTargets, lba: 45888 + x * 2157000 | 0, radiusMm: 25.3 + 31.6 * x, phaseDeg: i * 37 % 360, sigmaDeg: 0.2, unwrappedTurns: i * 300, constPitchResidualDeg: Math.sin(x * 6) * 1.1, cubicPitchResidualDeg: null as number | null, spindlePeriodMs: 25.474 + Math.sin(i) * 0.004, chi2PerDof: 1 }
        rungs.push(e); emit('calib://rung-progress', e); emit('calib://status', `第 ${i + 1} / ${a.maxTargets} 阶`); await wait(40)
      }
      rungs.forEach((r, i) => (r.cubicPitchResidualDeg = Math.sin(i * 2.1) * 0.35))
      return { preset: { ...demo, id: '' }, rungs, constantRmsDeg: 0.715, cubicRmsDeg: 0.402, spindlePeriodMs: 25.474, suspect: false }
    }
    case 'get_calibration_log': return [...log].reverse()
    case 'add_calibration_record': log.push(a.record); return null
    case 'delete_calibration_record': log = log.filter((r) => r.id !== a.id); return null
    case 'cancel_job': stop = true; return null
    case 'cache_iso_path': return `/cache/${a.name}`
    case 'burn_iso_to_drive':
      stop = false
      for (let p = 0; p <= 100; p += 4) {
        if (stop) { stop = false; throw '已取消' }
        emit('burn://progress', { percent: p, message: '正在写入' })
        await wait(250)
      }
      globalThis.__mockDisc = 'written'
      return null
    default: return null
  }
}
