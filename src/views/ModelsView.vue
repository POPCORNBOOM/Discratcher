<template>
  <div class="page page-flow">
    <div class="flow">
      <h1>{{ t('models.title') }}</h1>
      <p class="small muted">{{ t('models.intro') }}</p>
      <form class="row" @submit.prevent="search">
        <input ref="field" v-model.trim="query" class="grow" :aria-label="t('models.codeLabel')" :placeholder="t('models.codeHint')" spellcheck="false" autocomplete="off" @focus="s.codeFocus = true" @blur="s.codeFocus = false" />
        <UiButton variant="solid" type="submit" :icon="mdiMagnify" :disabled="!searchable || busy">{{ t('models.search') }}</UiButton>
      </form>

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

      <div v-if="busy" class="small muted">{{ t('models.searching') }}</div>

      <div v-if="results.length" class="list">
        <div v-for="r in results" :key="r.preset.id" class="item" @pointerenter="s.discPreview = r.preset" @pointerleave="s.discPreview = null">
          <div class="grow text">
            <div class="ellipsis" :data-tooltip="r.preset.name" data-tooltip-overflow>{{ r.preset.name }}</div>
            <div class="small muted ellipsis selectable" :data-tooltip="r.preset.stamperCode" data-tooltip-overflow>{{ r.preset.stamperCode }}</div>
            <div class="badges">
              <span v-if="r.preset.brand" class="badge">{{ r.preset.brand }}</span>
              <span class="badge">{{ sizeLabel(r.preset) }}</span>
              <span v-if="!r.local && !r.verified" class="badge warn">{{ t('models.unverified') }}</span>
              <span v-if="!r.local && r.contributor" class="small muted ellipsis">{{ t('models.by', { name: r.contributor }) }}</span>
            </div>
          </div>
          <span v-if="r.rating" class="rating small" :data-tooltip="t('models.rated', { n: r.rating.count })"><UiIcon :path="mdiStar" />{{ r.rating.average.toFixed(1) }}</span>
          <UiButton :variant="r.local ? 'soft' : 'solid'" :icon="r.local ? undefined : mdiDownload" :disabled="!!saving" @click="use(r)">{{ t(r.local ? 'models.use' : 'models.download') }}</UiButton>
        </div>
      </div>

      <div v-if="unreachable" class="notice warn">
        <UiIcon :path="mdiAlertOutline" />
        <div>{{ t(results.length ? 'models.offlineSome' : 'models.offlineNone') }}</div>
      </div>

      <div v-if="searched && !busy && !results.length && !unreachable" class="notice info">
        <UiIcon :path="mdiInformationOutline" />
        <div>{{ t('models.none', { q: searched }) }}</div>
      </div>

      <div class="fallback">
        <span class="grow small muted">{{ t('models.fallback') }}</span>
        <UiButton :variant="searched && !busy && !results.length ? 'solid' : 'soft'" :icon="mdiDiscPlayer" @click="calibrate">{{ t('models.local') }}</UiButton>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { mdiAlertOutline, mdiDiscPlayer, mdiDownload, mdiInformationOutline, mdiMagnify, mdiStar } from '@mdi/js'
import { computed, nextTick, ref, shallowRef, watch } from 'vue'
import { call, loadCatalog } from '../api'
import { MIN_QUERY, byRating, matches, normalizeCode, sizeCm, sizeLabel, type CatalogEntry, type Rating } from '../catalog'
import UiButton from '../components/UiButton.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiIcon from '../components/UiIcon.vue'
import UiSegmented from '../components/UiSegmented.vue'
import { t } from '../i18n'
import { notifyError, notifySuccess } from '../notices'
import { useStudio } from '../stores/studio'
import type { DiscGeometryPreset } from '../types'

interface Result {
  preset: DiscGeometryPreset
  local: boolean
  contributor?: string
  verified?: boolean
  rating?: Rating | null
}

const sizes = computed(() => [{ value: '', label: t('models.sizeAll') }, { value: '12', label: '12 cm' }, { value: '8', label: '8 cm' }])
const orders = computed(() => [{ value: 'rating', label: t('models.sortRating') }, { value: 'new', label: t('models.sortNew') }])
const size = ref('')
const order = ref('rating')


const s = useStudio()
const field = ref<HTMLInputElement | null>(null)
const query = ref('')
const searched = ref('')
const busy = ref(false)
const saving = ref('')
const unreachable = ref(false)
const results = shallowRef<Result[]>([])
const searchable = computed(() => normalizeCode(query.value).length >= MIN_QUERY)

watch(
  () => s.view,
  async (v) => {
    if (v !== 'models') return
    await nextTick()
    field.value?.focus()
  },
  { immediate: true },
)

async function search() {
  if (!searchable.value || busy.value) return
  const q = query.value
  busy.value = true
  unreachable.value = false
  s.discPreview = null
  results.value = []
  searched.value = q
  const fits = (p: DiscGeometryPreset) => !size.value || sizeCm(p) === Number(size.value)
  const local = s.presets.filter((p) => matches(p, q) && fits(p))
  let remote: CatalogEntry[] = []
  try {
    remote = (await loadCatalog()).filter((e) => matches(e.preset, q) && fits(e.preset))
  } catch {
    unreachable.value = true
  }
  const known = new Set(local.flatMap((p) => [p.id, normalizeCode(p.stamperCode)]))
  results.value = [
    ...local.map((preset) => ({ preset, local: true })),
    ...remote
      .filter((e) => !known.has(e.preset.id) && !known.has(normalizeCode(e.preset.stamperCode)))
      .sort((a, b) => (order.value === 'rating' ? byRating(a, b) : b.preset.createdAt.localeCompare(a.preset.createdAt)))
      .map((e) => ({ ...e, local: false })),
  ]
  busy.value = false
}

watch([size, order], () => searched.value && search())

async function use(r: Result) {
  saving.value = r.preset.id
  try {
    if (!r.local) {
      await call('save_preset', { preset: r.preset })
      await s.refreshPresets()
      notifySuccess(t('models.downloaded'))
    }
    s.presetId = r.preset.id
    s.discPreview = null
    s.view = 'carve'
  } catch (e) {
    notifyError(String(e))
  } finally {
    saving.value = ''
  }
}

function calibrate() {
  s.pendingCode = searched.value
  s.view = 'calibrate'
}
</script>

<style scoped>
input { min-width: 0; height: var(--oc-size-md); padding: 0 var(--oc-space-sm); border: 0; border-radius: var(--oc-radius-control); background: var(--oc-bg-raised); }
input::placeholder { color: var(--oc-fg-muted); }
.list { display: flex; flex-direction: column; gap: 2px; }
.item { display: flex; align-items: center; gap: var(--oc-space-sm); padding: var(--oc-space-xs) var(--oc-space-sm); border-radius: var(--oc-radius-control); background: var(--oc-bg-glass); backdrop-filter: blur(18px); }
.text { min-width: 0; }
.fallback { display: flex; align-items: center; gap: var(--oc-space-sm); padding-top: var(--oc-space-md); border-top: 1px solid var(--oc-border-muted); }
.rating { display: inline-flex; align-items: center; gap: 2px; color: var(--oc-fg-accent); }
.rating svg { width: 14px; height: 14px; }
.field { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
.badges { display: flex; align-items: center; flex-wrap: wrap; gap: var(--oc-space-2xs); margin-top: 2px; }
.badge { height: 18px; display: inline-flex; align-items: center; padding: 0 var(--oc-space-xs); border-radius: 9px; background: var(--oc-bg-raised); color: var(--oc-fg-muted); font-size: var(--oc-text-xs); white-space: nowrap; }
.badge.warn { background: var(--oc-bg-warning-subtle); color: var(--oc-fg-warning); }
</style>
