import { defineStore } from 'pinia'
import { computed, ref, shallowRef, watch } from 'vue'
import { call } from '../api'
import { MAX_LAYERS, type ArtField } from '../discRenderer'
import { notifyError } from '../notices'
import type { ArtworkAnalysis, Layer } from '../types'

import { copyLayer, makeLayer, readLayer, restacked } from '../layers'

export { coverSize, fileName } from '../layers'
const snapshot = (layers: Layer[]) => JSON.stringify(layers)

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
      const l = makeLayer(file, a, layers.value.length === 0)
      layers.value.push(l)
      selectedId.value = l.id
    } catch (e) {
      notifyError(String(e))
    }
  }

  /** A copy just above the original, nudged aside so it can be told apart, and selected. */
  function duplicate(id: string) {
    const i = layers.value.findIndex((l) => l.id === id)
    if (i < 0 || full.value) return
    const copy = copyLayer(layers.value[i])
    const next = [...layers.value]
    next.splice(i + 1, 0, copy)
    layers.value = next
    selectedId.value = copy.id
  }

  function remove(id: string) {
    layers.value = layers.value.filter((l) => l.id !== id)
    if (selectedId.value === id) selectedId.value = ''
  }

  /** `by` 1 brings the picture forward, -1 sends it back. */
  function restack(id: string, by: 1 | -1) {
    layers.value = restacked(layers.value, id, by) ?? layers.value
  }

  function clear() {
    layers.value = []
    selectedId.value = ''
    path.value = ''
    saved.value = snapshot([])
  }

  async function open(file: string) {
    const opened = await call<Layer[]>('open_composition', { path: file })
    layers.value = opened.slice(0, MAX_LAYERS).flatMap((l) => readLayer(l) ?? [])
    selectedId.value = ''
    path.value = file
    saved.value = snapshot(layers.value)
  }

  async function save(file: string) {
    await call('save_composition', { path: file, layers: layers.value })
    path.value = file
    saved.value = snapshot(layers.value)
  }

  return { layers, selectedId, selected, fields, pictures, path, dirty, loading, full, add, duplicate, remove, restack, clear, open, save }
})
