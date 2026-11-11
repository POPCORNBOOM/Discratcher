<template>
  <div class="page page-flow">
    <div class="flow">
      <h1>{{ t('models.title') }}</h1>
      <p class="small muted">{{ t('models.intro') }}</p>
      <input ref="field" v-model.trim="query" :aria-label="t('models.codeLabel')" :placeholder="t('models.codeHint')" spellcheck="false" autocomplete="off" @focus="s.codeFocus = true" @blur="s.codeFocus = false" />

      <UiAdvanced>
        <div class="field">
          <span class="small muted">{{ t('models.size') }}</span>
          <UiSegmented v-model="size" :options="sizes" />
        </div>
        <div class="field">
          <span class="small muted">{{ t('models.sort') }}</span>
          <UiSegmented v-model="order" :options="orders" />
        </div>
      </UiAdvanced>

      <div v-if="syncing && !index.length" class="small muted">{{ t('models.syncing') }}</div>

      <div v-if="rows.length" class="list">
        <div v-for="r in rows" :key="r.id" class="item" @pointerenter="s.discPreview = r.look" @pointerleave="s.discPreview = null">
          <UiIcon class="mark" :path="r.preset ? mdiHarddisk : mdiCloudOutline" :data-tooltip="t(r.preset ? 'models.onDisk' : 'models.inCloud')" />
          <div class="grow text">
            <div class="ellipsis" :data-tooltip="r.look.name" data-tooltip-overflow>{{ r.look.name }}</div>
            <div v-if="normalizeCode(r.look.name) !== normalizeCode(r.look.stamperCode)" class="small muted ellipsis selectable" :data-tooltip="r.look.stamperCode" data-tooltip-overflow>{{ r.look.stamperCode }}</div>
            <div class="badges">
              <span v-if="r.brand" class="badge">{{ r.brand }}</span>
              <span class="badge">{{ sizeLabel(r.size) }}</span>
              <span v-if="!r.preset && r.contributor" class="small muted ellipsis">{{ t('models.by', { name: r.contributor }) }}</span>
            </div>
          </div>
          <span v-if="r.rating" class="rating small" :data-tooltip="t('models.rated', { n: r.rating.count })"><UiIcon :path="mdiStar" />{{ r.rating.average.toFixed(1) }}</span>
          <UiButton :variant="r.preset ? 'soft' : 'solid'" :icon="r.preset ? undefined : mdiDownload" :disabled="!!saving" @click="use(r)">{{ t(r.preset ? 'models.use' : 'models.download') }}</UiButton>
        </div>
      </div>

      <div v-if="offline" class="notice warn">
        <UiIcon :path="mdiAlertOutline" />
        <div class="grow">{{ t('models.offline') }}</div>
        <UiButton class="retry" variant="ghost" :icon="mdiRefresh" :disabled="syncing" @click="sync(true)">{{ t('models.retry') }}</UiButton>
      </div>

      <div v-else-if="settled && !syncing && !rows.length" class="notice info">
        <UiIcon :path="mdiInformationOutline" />
        <div>{{ t('models.none', { q: settled }) }}</div>
      </div>

      <div class="fallback">
        <span class="grow small muted">{{ t('models.fallback') }}</span>
        <UiButton :variant="settled && !rows.length ? 'solid' : 'soft'" :icon="mdiDiscPlayer" @click="calibrate">{{ t('models.local') }}</UiButton>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { mdiAlertOutline, mdiCloudOutline, mdiDiscPlayer, mdiDownload, mdiHarddisk, mdiInformationOutline, mdiRefresh, mdiStar } from '@mdi/js'
import { computed, nextTick, ref, shallowRef, watch } from 'vue'
import { call, loadIndex, loadPresets } from '../api'
import { byRating, matches, normalizeCode, sizeCm, sizeLabel, type CatalogEntry, type Rating } from '../catalog'
import UiButton from '../components/UiButton.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiIcon from '../components/UiIcon.vue'
import UiSegmented from '../components/UiSegmented.vue'
import { t } from '../i18n'
import { notifyError, notifySuccess } from '../notices'
import { useStudio } from '../stores/studio'
import type { DiscGeometryPreset, PresetLook } from '../types'

/** Typing settles before the list is rebuilt, so it never flickers through half-typed codes. */
const DEBOUNCE_MS = 200

interface Row {
  id: string
  /** What the disc preview draws: a library row carries enough for that. */
  look: PresetLook
  brand: string
  size: 8 | 12
  contributor: string
  rating: Rating | null
  /** Null for a row that only exists in the library. */
  preset: DiscGeometryPreset | null
}

const sizes = computed(() => [{ value: '', label: t('models.sizeAll') }, { value: '12', label: '12 cm' }, { value: '8', label: '8 cm' }])
const orders = computed(() => [{ value: 'rating', label: t('models.sortRating') }, { value: 'new', label: t('models.sortNew') }])
const size = ref('')
const order = ref('rating')

const s = useStudio()
const field = ref<HTMLInputElement | null>(null)
const query = ref('')
/** What the list is built from: the field settles first. */
const settled = ref('')
const index = shallowRef<CatalogEntry[]>([])
const syncing = ref(false)
const offline = ref(false)
const saving = ref('')

let typing = 0
watch(query, (v) => {
  clearTimeout(typing)
  typing = window.setTimeout(() => (settled.value = v), DEBOUNCE_MS)
})

// Local rows and library rows together: an empty query lists both, downloading one moves it here.
const rows = computed<Row[]>(() => {
  const want = Number(size.value) || 0
  const fits = (cm: number) => !want || cm === want
  const local = s.presets.filter((p) => fits(sizeCm(p)) && matches(p, settled.value))
  const known = new Set(local.flatMap((p) => [p.id, normalizeCode(p.stamperCode)]))
  const remote = index.value
    .filter((e) => e.verified && fits(e.size) && matches(e, settled.value) && !known.has(e.id) && !known.has(normalizeCode(e.stamperCode)))
    .sort(order.value === 'rating' ? byRating : (a, b) => b.createdAt.localeCompare(a.createdAt))
  return [
    ...local.map((preset) => ({ id: preset.id, look: preset, brand: preset.brand ?? '', size: sizeCm(preset), contributor: '', rating: null, preset })),
    ...remote.map((entry) => ({ id: entry.id, look: entry, brand: entry.brand, size: entry.size, contributor: entry.contributor, rating: entry.rating, preset: null })),
  ]
})

/** A retried sync reports how it went; the automatic one on entry stays quiet. */
async function sync(announce = false) {
  if (syncing.value) return
  syncing.value = true
  try {
    index.value = await loadIndex()
    offline.value = false
    if (announce) notifySuccess(t('models.synced'))
  } catch {
    offline.value = true
    if (announce) notifyError(t('models.offline'))
  } finally {
    syncing.value = false
  }
}

watch(
  () => s.view,
  async (v) => {
    if (v !== 'models') return
    void sync()
    await nextTick()
    field.value?.focus()
  },
  { immediate: true },
)

async function use(r: Row) {
  saving.value = r.id
  try {
    const preset = r.preset ?? (await download(r.id))
    if (!r.preset) {
      await call('save_preset', { preset })
      await s.refreshPresets()
      notifySuccess(t('models.downloaded'))
    }
    s.presetId = preset.id
    s.discPreview = null
    s.view = 'carve'
  } catch (e) {
    notifyError(String(e))
  } finally {
    saving.value = ''
  }
}

/** The geometry behind a row that is not on this machine yet: only a download needs it. */
async function download(id: string) {
  let all: DiscGeometryPreset[]
  try {
    all = await loadPresets()
  } catch {
    throw t('models.offline')
  }
  const preset = all.find((p) => p.id === id)
  if (!preset) throw t('models.gone')
  return preset
}

function calibrate() {
  s.pendingCode = query.value
  s.view = 'calibrate'
}
</script>

<style scoped>
input { width: 100%; min-width: 0; height: var(--oc-size-md); padding: 0 var(--oc-space-sm); border: 0; border-radius: var(--oc-radius-control); background: var(--oc-bg-raised); }
input::placeholder { color: var(--oc-fg-muted); }
.list { display: flex; flex-direction: column; gap: 2px; }
.item { display: flex; align-items: center; gap: var(--oc-space-sm); padding: var(--oc-space-xs) var(--oc-space-sm); border-radius: var(--oc-radius-control); background: var(--oc-bg-glass); backdrop-filter: blur(18px); }
.mark { flex: none; color: var(--oc-fg-muted); }
.text { min-width: 0; }
.fallback { display: flex; align-items: center; gap: var(--oc-space-sm); padding-top: var(--oc-space-md); border-top: 1px solid var(--oc-border-muted); }
.rating { display: inline-flex; align-items: center; gap: 2px; color: var(--oc-fg-accent); }
.rating svg { width: 14px; height: 14px; }
.field { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
.retry { flex: none; align-self: center; }
.badges { display: flex; align-items: center; flex-wrap: wrap; gap: var(--oc-space-2xs); margin-top: 2px; }
.badge { height: 18px; display: inline-flex; align-items: center; padding: 0 var(--oc-space-xs); border-radius: 9px; background: var(--oc-bg-raised); color: var(--oc-fg-muted); font-size: var(--oc-text-xs); white-space: nowrap; }
.badge.warn { background: var(--oc-bg-warning-subtle); color: var(--oc-fg-warning); }
</style>
