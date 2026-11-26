/**
 * Binary display-mesh decoder (matches `disc_core::holo::model3d::mesh_to_display_bytes`)
 * plus a lightweight bundled sample mesh generator for browser mock mode.
 */

export interface DisplayMeshData {
  positions: Float32Array
  normals: Float32Array
  indices: Uint32Array
  triangles: number
}

/**
 * Decode binary payload from `holo_load_mesh`:
 * `[vCount:u32][iCount:u32][positions:f32*3V][normals:f32*3V][indices:u32*I]`
 */
export function decodeDisplayMesh(raw: ArrayBuffer | Uint8Array): DisplayMeshData {
  const bytes = raw instanceof Uint8Array ? raw : new Uint8Array(raw)
  if (bytes.byteLength < 8) {
    throw new Error('Display mesh buffer is truncated')
  }
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength)
  const vCount = view.getUint32(0, true)
  const iCount = view.getUint32(4, true)
  const posBytes = vCount * 12
  const nrmBytes = vCount * 12
  const idxBytes = iCount * 4
  const expected = 8 + posBytes + nrmBytes + idxBytes
  if (bytes.byteLength < expected || vCount === 0 || iCount === 0) {
    throw new Error('Display mesh header does not match buffer size')
  }
  const buf = bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + expected)
  const positions = new Float32Array(buf, 8, vCount * 3)
  const normals = new Float32Array(buf, 8 + posBytes, vCount * 3)
  const indices = new Uint32Array(buf, 8 + posBytes + nrmBytes, iCount)
  return { positions, normals, indices, triangles: Math.floor(iCount / 3) }
}

/** Pack normalized flat-shaded triangles into the exact binary format returned by `holo_load_mesh`. */
export function packDisplayMesh(rawVerts: [number, number, number][], faces: [number, number, number][]): ArrayBuffer {
  let minX = Infinity, minY = Infinity, minZ = Infinity
  let maxX = -Infinity, maxY = -Infinity, maxZ = -Infinity
  for (const [x, y, z] of rawVerts) {
    if (x < minX) minX = x; if (x > maxX) maxX = x
    if (y < minY) minY = y; if (y > maxY) maxY = y
    if (z < minZ) minZ = z; if (z > maxZ) maxZ = z
  }
  const cx = 0.5 * (minX + maxX)
  const cy = 0.5 * (minY + maxY)
  const cz = 0.5 * (minZ + maxZ)
  const extent = Math.max(maxX - minX, maxY - minY, maxZ - minZ, 1e-6)
  const norm = rawVerts.map(([x, y, z]): [number, number, number] => [
    (x - cx) / extent,
    (y - cy) / extent,
    (z - cz) / extent,
  ])

  const vCount = faces.length * 3
  const iCount = vCount
  const buf = new ArrayBuffer(8 + vCount * 12 + vCount * 12 + iCount * 4)
  const dv = new DataView(buf)
  dv.setUint32(0, vCount, true)
  dv.setUint32(4, iCount, true)
  const pos = new Float32Array(buf, 8, vCount * 3)
  const nrm = new Float32Array(buf, 8 + vCount * 12, vCount * 3)
  const idx = new Uint32Array(buf, 8 + vCount * 24, iCount)

  let vi = 0
  for (const [a, b, c] of faces) {
    const p0 = norm[a], p1 = norm[b], p2 = norm[c]
    const ux = p1[0] - p0[0], uy = p1[1] - p0[1], uz = p1[2] - p0[2]
    const vx = p2[0] - p0[0], vy = p2[1] - p0[1], vz = p2[2] - p0[2]
    let nx = uy * vz - uz * vy
    let ny = uz * vx - ux * vz
    let nz = ux * vy - uy * vx
    const nl = Math.hypot(nx, ny, nz) || 1
    nx /= nl; ny /= nl; nz /= nl

    for (const p of [p0, p1, p2]) {
      pos[vi * 3] = p[0]
      pos[vi * 3 + 1] = p[1]
      pos[vi * 3 + 2] = p[2]
      nrm[vi * 3] = nx
      nrm[vi * 3 + 1] = ny
      nrm[vi * 3 + 2] = nz
      idx[vi] = vi
      vi++
    }
  }
  return buf
}

/** Bundled 3D five-pointed star matching `BUILTIN_STAR_OBJ` in Rust `model3d.rs`. */
export function buildSampleStarBuffer(): ArrayBuffer {
  const verts: [number, number, number][] = [
    [0.0, 1.0, -0.3],
    [-0.2586, 0.356, 0.05],
    [-0.9511, 0.309, -0.3],
    [-0.4185, -0.136, 0.05],
    [-0.5878, -0.809, -0.3],
    [0.0, -0.44, 0.05],
    [0.5878, -0.809, -0.3],
    [0.4185, -0.136, 0.05],
    [0.9511, 0.309, -0.3],
    [0.2586, 0.356, 0.05],
    [0.0, 0.0, 1.05],
    [0.0, 0.0, -0.45],
  ]
  const faces: [number, number, number][] = [
    [10, 0, 1], [10, 1, 2], [10, 2, 3], [10, 3, 4], [10, 4, 5],
    [10, 5, 6], [10, 6, 7], [10, 7, 8], [10, 8, 9], [10, 9, 0],
    [11, 1, 0], [11, 2, 1], [11, 3, 2], [11, 4, 3], [11, 5, 4],
    [11, 6, 5], [11, 7, 6], [11, 8, 7], [11, 9, 8], [11, 0, 9],
  ]
  return packDisplayMesh(verts, faces)
}

/** Faceted crystal / octahedron for secondary mock models so multiple objects are visually distinct. */
export function buildMockGemBuffer(): ArrayBuffer {
  const verts: [number, number, number][] = [
    [0, 1, 0],
    [0.75, 0, 0.45],
    [-0.75, 0, 0.45],
    [-0.75, 0, -0.45],
    [0.75, 0, -0.45],
    [0, -1, 0],
  ]
  const faces: [number, number, number][] = [
    [0, 2, 1], [0, 3, 2], [0, 4, 3], [0, 1, 4],
    [5, 1, 2], [5, 2, 3], [5, 3, 4], [5, 4, 1],
  ]
  return packDisplayMesh(verts, faces)
}
