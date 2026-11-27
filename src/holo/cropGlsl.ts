// Multi-shape crop evaluation and GPU CanvasTexture rasterization, matching `disc_core::holo::flat::PositionCrop`.
import {
  ClampToEdgeWrapping,
  DataTexture,
  LinearFilter,
  RGBAFormat,
  UnsignedByteType,
  CanvasTexture,
  type Texture,
} from 'three'
import type { HoloCrop, HoloMaskCrop, HoloPosition, MaskChannel } from './scene'

const MASK_TEX_SIZE = 512

export interface DecodedMaskData {
  width: number
  height: number
  data: Uint8ClampedArray
}

export type MaskPixelLookup = Readonly<Record<string, DecodedMaskData>>

/** Whether `pos` has at least one visible crop that does not already cover 360°. */
export function hasActiveCrop(pos: HoloPosition | undefined): boolean {
  if (!pos) return false
  const visible = pos.crops.filter((c) => c.visible)
  if (!visible.length) return false
  return !visible.some((c) => c.kind === 'sector' && ((c.side === 'both' && c.deg >= 90) || c.deg >= 180))
}

function inSector(nx: number, ny: number, deg: number, side: 'both' | 'up' | 'down', offsetDeg: number): boolean {
  if ((side === 'both' && deg >= 90) || deg >= 180) return true
  if (nx === 0 && ny === 0) return true
  const axis = (offsetDeg * Math.PI) / 180
  const ux = Math.sin(axis)
  const uy = -Math.cos(axis)
  const a = Math.atan2(Math.abs(nx * uy - ny * ux), nx * ux + ny * uy)
  if (side === 'both') {
    const c = Math.min(Math.max(deg, 0), 90) * (Math.PI / 180)
    return a <= c || a >= Math.PI - c
  }
  if (side === 'up') {
    return a <= Math.min(Math.max(deg, 0), 180) * (Math.PI / 180)
  }
  return a >= Math.PI - Math.min(Math.max(deg, 0), 180) * (Math.PI / 180)
}

function inCircle(nx: number, ny: number, cx: number, cy: number, size: number, stretchX: number, stretchY: number, rotationDeg: number): boolean {
  const rx = Math.max(0.5 * size * Math.max(stretchX, 1e-4), 1e-6)
  const ry = Math.max(0.5 * size * Math.max(stretchY, 1e-4), 1e-6)
  const dx = nx - cx
  const dy = ny - cy
  const rad = (rotationDeg * Math.PI) / 180
  const cos = Math.cos(rad)
  const sin = Math.sin(rad)
  const lx = (cos * dx + sin * dy) / rx
  const ly = (-sin * dx + cos * dy) / ry
  return lx * lx + ly * ly <= 1
}

function inPolygon(nx: number, ny: number, sides: number, cx: number, cy: number, size: number, stretchX: number, stretchY: number, rotationDeg: number): boolean {
  const rx = Math.max(0.5 * size * Math.max(stretchX, 1e-4), 1e-6)
  const ry = Math.max(0.5 * size * Math.max(stretchY, 1e-4), 1e-6)
  const dx = nx - cx
  const dy = ny - cy
  const rad = (rotationDeg * Math.PI) / 180
  const cos = Math.cos(rad)
  const sin = Math.sin(rad)
  const lx = (cos * dx + sin * dy) / rx
  const ly = (-sin * dx + cos * dy) / ry
  const r = Math.hypot(lx, ly)
  if (r === 0) return true
  const n = Math.min(12, Math.max(3, Math.round(sides)))
  const sector = (Math.PI * 2) / n
  const ang = (((Math.atan2(lx, -ly) % sector) + sector) % sector) - 0.5 * sector
  return r * Math.cos(ang) <= Math.cos(0.5 * sector)
}

function sampleMaskScalar(d: DecodedMaskData, u: number, v: number, channel: MaskChannel, invert: boolean): number {
  const { width: w, height: h, data } = d
  const x = Math.min(Math.max(u * w - 0.5, 0), w - 1)
  const y = Math.min(Math.max(v * h - 0.5, 0), h - 1)
  const ix = Math.min(Math.floor(x), Math.max(0, w - 2))
  const iy = Math.min(Math.floor(y), Math.max(0, h - 2))
  const fx = x - ix
  const fy = y - iy
  const dx = w > 1 ? 4 : 0
  const dy = h > 1 ? w * 4 : 0
  const i = (iy * w + ix) * 4
  const val = (off: number): number => {
    const a = data[off + 3]
    if (channel === 'alpha') {
      return invert ? 255 - a : a
    }
    if (a <= 0) return 0
    const luma = 0.299 * data[off] + 0.587 * data[off + 1] + 0.114 * data[off + 2]
    const base = invert ? 255 - luma : luma
    return base * (a / 255)
  }
  const a = val(i)
  const b = val(i + dx)
  const c = val(i + dy)
  const e = val(i + dy + dx)
  const top = a + (b - a) * fx
  return top + (c + (e - c) * fx - top) * fy
}

function inMask(nx: number, ny: number, crop: HoloMaskCrop, masks?: MaskPixelLookup): boolean {
  const long = Math.max(crop.width, crop.height, 1)
  const w = Math.max((crop.size * Math.max(crop.stretchX, 1e-4) * Math.max(crop.width, 1)) / long, 1e-6)
  const h = Math.max((crop.size * Math.max(crop.stretchY, 1e-4) * Math.max(crop.height, 1)) / long, 1e-6)
  const dx = nx - crop.x
  const dy = ny - crop.y
  const rad = (crop.rotationDeg * Math.PI) / 180
  const cos = Math.cos(rad)
  const sin = Math.sin(rad)
  const u = (cos * dx + sin * dy) / w + 0.5
  const v = (-sin * dx + cos * dy) / h + 0.5
  if (u < 0 || u > 1 || v < 0 || v > 1) return false
  const decoded = masks?.[crop.path]
  if (!decoded) return true
  return sampleMaskScalar(decoded, u, v, crop.channel, crop.invert) >= 127.5
}

/** Whether normalized position-frame point `(nx, ny)` (`-0.5..0.5`, `+y` down) lies inside `crop`. */
export function inSingleCrop(nx: number, ny: number, crop: HoloCrop, masks?: MaskPixelLookup): boolean {
  if (crop.kind === 'sector') return inSector(nx, ny, crop.deg, crop.side, crop.offsetDeg)
  if (crop.kind === 'circle') return inCircle(nx, ny, crop.x, crop.y, crop.size, crop.stretchX, crop.stretchY, crop.rotationDeg)
  if (crop.kind === 'polygon') return inPolygon(nx, ny, crop.sides, crop.x, crop.y, crop.size, crop.stretchX, crop.stretchY, crop.rotationDeg)
  return inMask(nx, ny, crop, masks)
}

/** Whether disc point `(xMm, yMm)` (Three.js position group frame, `+y` up) lies inside `pos`'s combined crop. */
export function inPositionCrop(
  xMm: number,
  yMm: number,
  outerRadiusMm: number,
  pos: HoloPosition | undefined,
  masks?: MaskPixelLookup,
): boolean {
  if (!pos || !hasActiveCrop(pos)) return true
  const across = Math.max(2 * outerRadiusMm, 1e-6)
  const nx = xMm / across
  const ny = -yMm / across
  for (const crop of pos.crops) {
    if (crop.visible && inSingleCrop(nx, ny, crop, masks)) return true
  }
  return false
}

interface CachedBitmap {
  url: string
  decoded: DecodedMaskData | null
  variants: Map<string, HTMLCanvasElement>
}

interface PositionTextureEntry {
  signature: string
  hasCrop: boolean
  canvas: HTMLCanvasElement
  texture: CanvasTexture
}

/** Rasterizes each position's multi-shape crop list into a 512x512 linear mask texture. */
export function createPositionCropMasks(redraw: () => void) {
  const whiteFallback = new DataTexture(new Uint8Array([255, 255, 255, 255]), 1, 1, RGBAFormat, UnsignedByteType)
  whiteFallback.needsUpdate = true

  const bitmaps = new Map<string, CachedBitmap>()
  const masks: Record<string, DecodedMaskData> = {}
  const entries = new Map<string, PositionTextureEntry>()
  let lastPositions: readonly HoloPosition[] = []
  let lastThumbnails: Readonly<Record<string, string>> = {}

  function ensureBitmap(path: string, url: string) {
    if (!url) return
    const existing = bitmaps.get(path)
    if (existing && existing.url === url) return
    const entry: CachedBitmap = { url, decoded: null, variants: new Map() }
    bitmaps.set(path, entry)
    const img = new Image()
    img.onload = () => {
      const current = bitmaps.get(path)
      if (!current || current.url !== url) return
      const c = document.createElement('canvas')
      c.width = Math.max(1, img.naturalWidth || img.width)
      c.height = Math.max(1, img.naturalHeight || img.height)
      const ctx = c.getContext('2d')
      if (!ctx) return
      ctx.drawImage(img, 0, 0, c.width, c.height)
      const imgData = ctx.getImageData(0, 0, c.width, c.height)
      current.decoded = { width: c.width, height: c.height, data: imgData.data }
      masks[path] = current.decoded
      sync(lastPositions, lastThumbnails)
      redraw()
    }
    img.src = url
  }

  function binaryMaskCanvas(path: string, channel: MaskChannel, invert: boolean): HTMLCanvasElement | null {
    const bm = bitmaps.get(path)
    const d = bm?.decoded
    if (!bm || !d) return null
    const key = `${channel}:${invert ? 1 : 0}`
    const cached = bm.variants.get(key)
    if (cached) return cached
    const c = document.createElement('canvas')
    c.width = d.width
    c.height = d.height
    const ctx = c.getContext('2d')!
    const out = ctx.createImageData(d.width, d.height)
    const src = d.data
    const dst = out.data
    for (let i = 0; i < src.length; i += 4) {
      const a = src[i + 3]
      let val = 0
      if (channel === 'alpha') {
        val = invert ? 255 - a : a
      } else if (a > 0) {
        const luma = 0.299 * src[i] + 0.587 * src[i + 1] + 0.114 * src[i + 2]
        val = (invert ? 255 - luma : luma) * (a / 255)
      }
      const on = val >= 127.5 ? 255 : 0
      dst[i] = 255
      dst[i + 1] = 255
      dst[i + 2] = 255
      dst[i + 3] = on
    }
    ctx.putImageData(out, 0, 0)
    bm.variants.set(key, c)
    return c
  }

  function drawShape(ctx: CanvasRenderingContext2D, crop: HoloCrop) {
    const S = MASK_TEX_SIZE
    const cx = S / 2
    const cy = S / 2
    ctx.fillStyle = '#ffffff'

    if (crop.kind === 'sector') {
      const axis = (crop.offsetDeg * Math.PI) / 180
      if (crop.side === 'both' || crop.side === 'up') {
        const c = Math.min(Math.max(crop.deg, 0), crop.side === 'both' ? 90 : 180) * (Math.PI / 180)
        const mid = axis - Math.PI / 2
        ctx.beginPath()
        ctx.moveTo(cx, cy)
        ctx.arc(cx, cy, S, mid - c, mid + c)
        ctx.closePath()
        ctx.fill()
      }
      if (crop.side === 'both' || crop.side === 'down') {
        const c = Math.min(Math.max(crop.deg, 0), crop.side === 'both' ? 90 : 180) * (Math.PI / 180)
        const mid = axis + Math.PI / 2
        ctx.beginPath()
        ctx.moveTo(cx, cy)
        ctx.arc(cx, cy, S, mid - c, mid + c)
        ctx.closePath()
        ctx.fill()
      }
      return
    }

    if (crop.kind === 'circle') {
      const rx = Math.max(0.5 * crop.size * Math.max(crop.stretchX, 1e-4) * S, 0.5)
      const ry = Math.max(0.5 * crop.size * Math.max(crop.stretchY, 1e-4) * S, 0.5)
      ctx.save()
      ctx.translate((crop.x + 0.5) * S, (crop.y + 0.5) * S)
      ctx.rotate((crop.rotationDeg * Math.PI) / 180)
      ctx.scale(rx, ry)
      ctx.beginPath()
      ctx.arc(0, 0, 1, 0, Math.PI * 2)
      ctx.fill()
      ctx.restore()
      return
    }

    if (crop.kind === 'polygon') {
      const rx = Math.max(0.5 * crop.size * Math.max(crop.stretchX, 1e-4) * S, 0.5)
      const ry = Math.max(0.5 * crop.size * Math.max(crop.stretchY, 1e-4) * S, 0.5)
      const n = Math.min(12, Math.max(3, Math.round(crop.sides)))
      const sector = (Math.PI * 2) / n
      ctx.save()
      ctx.translate((crop.x + 0.5) * S, (crop.y + 0.5) * S)
      ctx.rotate((crop.rotationDeg * Math.PI) / 180)
      ctx.beginPath()
      for (let k = 0; k < n; k++) {
        const a = k * sector
        const vx = Math.sin(a) * rx
        const vy = -Math.cos(a) * ry
        if (k === 0) ctx.moveTo(vx, vy)
        else ctx.lineTo(vx, vy)
      }
      ctx.closePath()
      ctx.fill()
      ctx.restore()
      return
    }

    const maskCanvas = binaryMaskCanvas(crop.path, crop.channel, crop.invert)
    if (!maskCanvas) return
    const long = Math.max(crop.width, crop.height, 1)
    const wPx = ((crop.size * Math.max(crop.stretchX, 1e-4) * Math.max(crop.width, 1)) / long) * S
    const hPx = ((crop.size * Math.max(crop.stretchY, 1e-4) * Math.max(crop.height, 1)) / long) * S
    ctx.save()
    ctx.translate((crop.x + 0.5) * S, (crop.y + 0.5) * S)
    ctx.rotate((crop.rotationDeg * Math.PI) / 180)
    ctx.drawImage(maskCanvas, -wPx / 2, -hPx / 2, wPx, hPx)
    ctx.restore()
  }

  function sync(positions: readonly HoloPosition[], thumbnails: Readonly<Record<string, string>>) {
    lastPositions = positions
    lastThumbnails = thumbnails

    const neededMaskPaths = new Set<string>()
    for (const pos of positions) {
      for (const crop of pos.crops) {
        if (crop.kind === 'mask' && crop.visible) {
          neededMaskPaths.add(crop.path)
          ensureBitmap(crop.path, thumbnails[crop.path] ?? '')
        }
      }
    }

    const activeIds = new Set(positions.map((p) => p.id))
    for (const [id, e] of entries) {
      if (!activeIds.has(id)) {
        e.texture.dispose()
        entries.delete(id)
      }
    }

    for (const pos of positions) {
      const hasCrop = hasActiveCrop(pos)
      const visibleCrops = pos.crops.filter((c) => c.visible)
      const sigParts = visibleCrops.map((c) => {
        if (c.kind !== 'mask') return c
        const bm = bitmaps.get(c.path)
        return { ...c, _url: bm?.url ?? '', _ready: bm?.decoded ? 1 : 0 }
      })
      const sig = `${hasCrop ? 1 : 0}|${JSON.stringify(sigParts)}`

      let e = entries.get(pos.id)
      if (!e) {
        const canvas = document.createElement('canvas')
        canvas.width = MASK_TEX_SIZE
        canvas.height = MASK_TEX_SIZE
        const texture = new CanvasTexture(canvas)
        texture.flipY = false
        texture.minFilter = LinearFilter
        texture.magFilter = LinearFilter
        texture.wrapS = ClampToEdgeWrapping
        texture.wrapT = ClampToEdgeWrapping
        e = { signature: '', hasCrop: false, canvas, texture }
        entries.set(pos.id, e)
      }

      if (e.signature !== sig) {
        e.signature = sig
        e.hasCrop = hasCrop
        if (hasCrop) {
          const ctx = e.canvas.getContext('2d')!
          ctx.fillStyle = '#000000'
          ctx.fillRect(0, 0, MASK_TEX_SIZE, MASK_TEX_SIZE)
          for (const crop of visibleCrops) {
            drawShape(ctx, crop)
          }
          e.texture.needsUpdate = true
        }
      }
    }
  }

  function get(positionId: string): { texture: Texture; hasCrop: boolean } {
    const e = entries.get(positionId)
    if (!e || !e.hasCrop) return { texture: whiteFallback, hasCrop: false }
    return { texture: e.texture, hasCrop: true }
  }

  function dispose() {
    for (const e of entries.values()) e.texture.dispose()
    entries.clear()
    bitmaps.clear()
    whiteFallback.dispose()
  }

  return { sync, get, masks: masks as MaskPixelLookup, dispose }
}
