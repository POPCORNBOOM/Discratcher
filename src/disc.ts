import type { OpticalDriveInfo } from './types'

/** A burn can go ahead: the disc is blank, or rewritable and erased by the burn itself. */
export const canBurn = (d: OpticalDriveInfo | null | undefined) => !d?.probing && (d?.disc === 'blank' || d?.disc === 'erasable')
