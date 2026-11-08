import { defineStore } from 'pinia'
import { computed, ref, shallowRef, watch } from 'vue'
import { call } from '../api'
import { MAX_LAYERS, type ArtField } from '../discRenderer'
import { notifyError } from '../notices'
import type { ArtworkAnalysis, Layer } from '../types'

export const fileName = (p: string) => p.split(/[\\/]/).pop() ?? p
const snapshot = (layers: Layer[]) => JSON.stringify(layers)

/** The size that lets a picture fill the disc, as one picture usually should. */
export const coverSize = (l: Pick<Layer, 'width' | 'height'>) => Math.max(l.width, l.height) / Math.max(1, Math.min(l.width, l.height))

/** The pictures on the disc, their places, and the file they were saved to. */
export const useComposition = defineStore('composition', () => {
  /** Bottom first. */
  const layers = ref<Layer[]>([])
  const selectedId = ref('')
  const selected = computed(() => layers.value.find((l) => l.id === selectedId.value) ?? null)
  /** Decoded pictures for the disc renderer, by path. */
  const fields = shallowRef(new Map<string, ArtField>())
  /** Thumbnail and tone hints, by path. */
  const pictures = ref<Record<string, ArtworkAnalysis>>({})
  const path = ref('')
  const saved = ref(snapshot([]))
  const dirty = computed(() => layers.value.length > 0 && snapshot(layers.value) !== saved.value)
  const loading = ref(0)
  const full = computed(() => layers.value.length >= MAX_LAYERS)

  // Each picture is read once; moving or toning it never goes back to the native side.
  watch(() => [...new Set(layers.value.map((l) => l.path))].join('\n'), async (joined) => {
    const wanted = joined ? joined.split('\n') : []
    const next = new Map([...fields.value].filter(([p]) => wanted.includes(p)))
    for (const p of wanted) {
      if (next.has(p)) continue
      loading.value++
      try {
        if (!pictures.value[p]) pictures.value[p] = await call<ArtworkAnalysis>('analyze_artwork', { imagePath: p })
        const data = new Uint8Array(await call<ArrayBuffer>('preview_texture', { imagePath: p }))
        next.set(p, { size: Math.round(Math.sqrt(data.length / 2)), data })
      } catch (e) {
        notifyError(String(e))
      } finally {
        loading.value--
      }
    }
    // Another change may have landed while this one was reading.
    const now = new Set(layers.value.map((l) => l.path))
    fields.value = new Map([...fields.value, ...next].filter(([p]) => now.has(p)))
  }, { immediate: true })

  async function add(file: string) {
    if (full.value) return
    try {
      const a = await call<ArtworkAnalysis>('analyze_artwork', { imagePath: file })
      pictures.value[file] = a
      const first = layers.value.length === 0
      const l: Layer = {
        id: crypto.randomUUID(), path: file, name: fileName(file), width: a.width, height: a.height,
        x: 0, y: 0, size: 0, rotationDeg: 0, invert: a.suggestInvert, contrast: 0,
      }
      // The first picture fills the disc; later ones arrive smaller, on top.
      l.size = first ? coverSize(l) : 0.5
      layers.value.push(l)
      selectedId.value = l.id
    } catch (e) {
      notifyError(String(e))
    }
  }

  function remove(id: string) {
    layers.value = layers.value.filter((l) => l.id !== id)
    if (selectedId.value === id) selectedId.value = ''
  }

  /** `by` 1 brings the picture forward, -1 sends it back. */
  function restack(id: string, by: 1 | -1) {
    const i = layers.value.findIndex((l) => l.id === id)
    const j = i + by
    if (i < 0 || j < 0 || j >= layers.value.length) return
    const next = [...layers.value]
    ;[next[i], next[j]] = [next[j], next[i]]
    layers.value = next
  }

  function clear() {
    layers.value = []
    selectedId.value = ''
    path.value = ''
    saved.value = snapshot([])
  }

  async function open(file: string) {
    const opened = await call<Layer[]>('open_composition', { path: file })
    layers.value = opened.slice(0, MAX_LAYERS).map((l) => ({ ...l, id: l.id || crypto.randomUUID() }))
    selectedId.value = ''
    path.value = file
    saved.value = snapshot(layers.value)
  }

  async function save(file: string) {
    await call('save_composition', { path: file, layers: layers.value })
    path.value = file
    saved.value = snapshot(layers.value)
  }

  return { layers, selectedId, selected, fields, pictures, path, dirty, loading, full, add, remove, restack, clear, open, save }
})
