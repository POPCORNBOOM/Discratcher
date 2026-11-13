// A CD drive's along-track constant, kept per drive model: it belongs to how the drive
// reads, not to the disc, so one burned twist chart serves every CD calibrated on it.
const key = (driveModel: string) => `discratcher.cdTwist.${driveModel}`

export function twistOf(driveModel: string): number | null {
  try {
    const v = localStorage.getItem(key(driveModel))
    return v === null || Number.isNaN(Number(v)) ? null : Number(v)
  } catch {
    return null
  }
}

export function rememberTwist(driveModel: string, frames: number) {
  try {
    localStorage.setItem(key(driveModel), String(frames))
  } catch {
    // Without storage the value still lives in the preset.
  }
}
