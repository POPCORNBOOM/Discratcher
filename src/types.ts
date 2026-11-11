export interface OpticalDriveInfo {
  id: string; vendor: string; product: string; revision: string
  mediaPresent: boolean; mediaType: string; mediaId: string
  totalSectors: number; isBlank: boolean; simulated: boolean
  writeSpeeds?: number[]; maxWriteSpeed?: number; supported?: boolean
}
export type DiscKind = 'none' | 'unsupported' | 'blank' | 'calibration' | 'other'
export interface DiscCheck { sectors: number; foreign: boolean }
export interface DiscGeometryPreset {
  id: string; driveModel: string; mediaId: string; stamperCode: string
  /** Disc brand, filled in by whoever shares the calibration. */
  brand?: string
  /** Set locally when a real burn with this geometry has finished; sharing needs it. */
  burnedAt?: string
  totalSectors: number; channelBitLengthNm: number; innerRadiusMm: number; outerRadiusMm: number
  pitchLinear: number; pitchQuadratic: number; pitchCubic: number; basePitchUm: number
  rmsResidualDeg: number; measuredSpanMm: [number, number]; createdAt: string
}
/** What previewing a disc needs; the library index carries it, so a row can be previewed before it is downloaded. */
export type PresetLook = Pick<DiscGeometryPreset, 'stamperCode' | 'mediaId' | 'basePitchUm'>
export interface CalibRungEvent {
  rungIndex: number; totalRungs: number; lba: number; radiusMm: number; phaseDeg: number
  sigmaDeg: number; unwrappedTurns: number; constPitchResidualDeg: number
  cubicPitchResidualDeg: number | null; spindlePeriodMs: number; chi2PerDof: number
}
export interface CalibrationResult {
  preset: DiscGeometryPreset; rungs: CalibRungEvent[]; constantRmsDeg: number
  cubicRmsDeg: number; spindlePeriodMs: number; suspect: boolean; specProblem?: string | null
}
export type CalibrationStatus = 'success' | 'suspect' | 'failed' | 'cancelled'
export interface CalibrationRungRecord {
  lba: number; radiusMm: number; phaseDeg: number; sigmaDeg: number; residualDeg: number; seconds: number
}
/** One attempt, kept whatever its outcome. The rung timings can refit the ETA model. */
export interface CalibrationRecord {
  id: string; startedAt: string; durationSeconds: number; status: CalibrationStatus; message: string
  driveModel: string; mediaId: string; stamperCode: string
  rungs: number; reads: number; rangePct: [number, number]; rungsMeasured: number
  rmsDeg: number | null; preset: DiscGeometryPreset | null; points: CalibrationRungRecord[]
}
/** One picture on the disc; the native side reads it as artwork::Layer. */
export interface Layer {
  id: string; path: string; name: string
  /** The picture's own pixel size. */
  width: number; height: number
  /** Centre as a share of the disc's diameter from its centre, y down. */
  x: number; y: number
  /** Long edge as a share of the diameter. */
  size: number
  /** Clockwise as seen. */
  rotationDeg: number
  invert: boolean; contrast: number
}
export interface CarveJobConfig {
  /** Bottom first. */
  layers: Layer[]; outputIsoPath: string; preset: DiscGeometryPreset
  servoSafeDither: boolean; maxDarkDutyCycle: number
}
export interface ArtworkAnalysis {
  width: number; height: number; darkRatio: number; suggestInvert: boolean
  maxBlackRunPx: number; thumbnail: string
}
export interface CarveProgress { percent: number; mbPerS: number; maxSectorDarkRatio: number }
export interface CarveJobSummary {
  outputIsoPath: string; totalSectors: number; bytes: number; elapsedSeconds: number
  mbPerS: number; meanDarkRatio: number; maxSectorDarkRatio: number
  maxDarkRunBytes: number; servoSafe: boolean; limitOk: boolean
}
export interface IsoAudit { sectors: number; foreignBytes: number; maxSectorDarkRatio: number; worstLba: number }
export interface BuiltIso { summary: CarveJobSummary; audit: IsoAudit }
export interface BurnProgress { percent: number | null; message: string }
