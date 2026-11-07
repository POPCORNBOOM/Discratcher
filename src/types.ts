export interface OpticalDriveInfo {
  id: string; vendor: string; product: string; revision: string
  mediaPresent: boolean; mediaType: string; mediaId: string
  totalSectors: number; isBlank: boolean; simulated: boolean
  writeSpeeds?: number[]; maxWriteSpeed?: number
}
export interface DiscGeometryPreset {
  id: string; name: string; driveModel: string; mediaId: string; stamperCode: string
  /** Disc brand, filled in by whoever shares the calibration. */
  brand?: string
  /** Set locally when a real burn with this geometry has finished; sharing needs it. */
  burnedAt?: string
  totalSectors: number; channelBitLengthNm: number; innerRadiusMm: number; outerRadiusMm: number
  pitchLinear: number; pitchQuadratic: number; pitchCubic: number; basePitchUm: number
  rmsResidualDeg: number; measuredSpanMm: [number, number]; createdAt: string
}
export interface CalibRungEvent {
  rungIndex: number; totalRungs: number; lba: number; radiusMm: number; phaseDeg: number
  sigmaDeg: number; unwrappedTurns: number; constPitchResidualDeg: number
  cubicPitchResidualDeg: number | null; spindlePeriodMs: number; chi2PerDof: number
}
export interface CalibrationResult {
  preset: DiscGeometryPreset; rungs: CalibRungEvent[]; constantRmsDeg: number
  cubicRmsDeg: number; spindlePeriodMs: number; suspect: boolean
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
export type CropMode = 'square-center' | 'stretch' | 'fit' | 'custom'
export interface CarveJobConfig {
  sourceImagePath: string; outputIsoPath: string; preset: DiscGeometryPreset
  invertImage: boolean; rotationDeg: number; cropMode: CropMode; contrast: number
  servoSafeDither: boolean; maxDarkDutyCycle: number
  offsetX: number; offsetY: number
  scale: number
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
