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

      <section v-for="g in groups" :key="g.key" class="group">
        <div class="small muted">{{ g.title }}</div>
        <div class="list">
          <div v-for="r in g.rows" :key="r.id" class="item" @pointerenter="s.discPreview = r.look" @pointerleave="s.discPreview = null">
            <UiIcon class="mark" :class="r.state" :path="stateIcon[r.state]" :data-tooltip="stateLabel[r.state]" />
            <div class="grow text">
              <div class="ellipsis" :data-tooltip="labelOf(r.look)" data-tooltip-overflow>{{ labelOf(r.look) }}</div>
              <div v-if="r.look.stamperCode && r.look.mediaId" class="small muted ellipsis selectable" :data-tooltip="r.look.mediaId" data-tooltip-overflow>{{ r.look.mediaId }}</div>
              <div class="badges">
                <span v-if="r.brand" class="badge">{{ r.brand }}</span>
                <span class="badge">{{ sizeLabel(r.size) }}</span>
                <span v-if="r.preset && r.state !== 'downloaded'" class="badge" :class="r.state">{{ stateLabel[r.state] }}</span>
                <span v-if="!r.preset && r.contributor" class="small muted ellipsis">{{ t('models.by', { name: r.contributor }) }}</span>
              </div>
            </div>
            <span v-if="r.rating" class="rating small" :data-tooltip="t('models.rated', { n: r.rating.count })"><UiIcon :path="mdiStar" />{{ r.rating.average.toFixed(1) }}</span>
            <template v-if="r.preset">
              <UiButton v-if="r.shared" icon-only variant="ghost" :icon="mdiOpenInNew" :aria-label="t('models.viewShared')" @click="viewShared(r.shared)" />
              <UiButton v-else-if="r.state !== 'downloaded'" icon-only variant="ghost" :icon="mdiShareVariantOutline" :aria-label="t('models.share')" :disabled="!r.preset.burnedAt" :data-tooltip="r.preset.burnedAt ? t('models.share') : t('models.shareTip')" @click="shareTarget = r.preset" />
              <UiButton v-if="r.state === 'unverified' || r.state === 'tested'" icon-only variant="ghost" :icon="mdiPencilOutline" :aria-label="t('models.edit')" @click="startEdit(r.preset)" />
              <UiButton icon-only variant="ghost" :icon="mdiExportVariant" :aria-label="t('models.export')" @click="exportPreset(r.preset)" />
              <UiButton icon-only variant="ghost" :icon="mdiTrashCanOutline" :aria-label="t('presets.delete', { name: labelOf(r.look) })" :disabled="!!s.job" @click="deleteTarget = r.preset" />
            </template>
            <UiButton :variant="r.preset ? 'soft' : 'solid'" :icon="r.preset ? undefined : mdiDownload" :disabled="!!saving" @click="use(r)">{{ t(r.preset ? 'models.use' : 'models.download') }}</UiButton>
          </div>
        </div>
      </section>

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

    <ContributeDialog v-if="shareTarget" :preset="shareTarget" @close="shareTarget = null" />
    <UiDialog v-if="editTarget" :title="t('models.edit')" @close="editTarget = null">
      <label class="field">
        <span class="small muted">{{ t('share.code') }}</span>
        <input v-model.trim="editCode" :placeholder="t('share.codeHint')" spellcheck="false" autocomplete="off" @keydown.enter="saveEdit" />
      </label>
      <label class="field">
        <span class="small muted">{{ t('share.brand') }}</span>
        <input v-model.trim="editBrand" :placeholder="t('share.brandHint')" spellcheck="false" autocomplete="off" @keydown.enter="saveEdit" />
      </label>
      <template #actions>
        <UiButton variant="ghost" @click="editTarget = null">{{ t('common.cancel') }}</UiButton>
        <UiButton variant="solid" @click="saveEdit">{{ t('models.save') }}</UiButton>
      </template>
    </UiDialog>
    <UiDialog v-if="deleteTarget" :title="t('presets.deleteTitle')" @close="deleteTarget = null">
      <p>{{ t('presets.deleteBody', { name: labelOf(deleteTarget) }) }}</p>
      <template #actions>
        <UiButton variant="ghost" @click="deleteTarget = null">{{ t('common.cancel') }}</UiButton>
        <UiButton variant="danger" @click="remove">{{ t('common.delete') }}</UiButton>
      </template>
    </UiDialog>
  </div>
</template>

<script setup lang="ts">
import { mdiAlertOutline, mdiCheckCircleOutline, mdiCloudOutline, mdiDiscPlayer, mdiDownload, mdiExportVariant, mdiHarddisk, mdiInformationOutline, mdiOpenInNew, mdiPencilOutline, mdiRefresh, mdiShareVariantOutline, mdiStar, mdiTrashCanOutline } from '@mdi/js'
import { computed, nextTick, ref, shallowRef, watch } from 'vue'
import { call, loadIndex, loadPreset, openInBrowser, pickDataPath } from '../api'
import { byRating, issueOf, issueUrl, labelOf, matches, normalizeCode, sizeCm, sizeLabel, type CatalogEntry, type Rating } from '../catalog'
import ContributeDialog from '../components/ContributeDialog.vue'
import UiDialog from '../components/UiDialog.vue'
import UiButton from '../components/UiButton.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiIcon from '../components/UiIcon.vue'
import UiSegmented from '../components/UiSegmented.vue'
import { t } from '../i18n'
import { isStandardPreset } from '../etaModel'
import { notifyError, notifySuccess } from '../notices'
import { useStudio } from '../stores/studio'
import type { DiscGeometryPreset, PresetLook } from '../types'

/** Typing settles before the list is rebuilt, so it never flickers through half-typed codes. */
const DEBOUNCE_MS = 200

/** How far a calibration has been taken. Ours go measured, burned, shared; a download is only held. */
type RowState = 'unverified' | 'tested' | 'contributed' | 'downloaded' | 'online'

interface Row {
  id: string
  state: RowState
  /** The library entry a calibration made here was shared as. */
  shared?: CatalogEntry
  /** What the disc preview draws: a library row carries enough for that. */
  look: PresetLook
  brand: string
  size: 8 | 12
  contributor: string
  rating: Rating | null
  /** Null for a row that only exists in the library. */
  preset: DiscGeometryPreset | null
}

const stateLabel = computed<Record<RowState, string>>(() => ({
  unverified: t('models.state.unverified'),
  tested: t('models.state.tested'),
  contributed: t('models.state.contributed'),
  downloaded: t('models.state.downloaded'),
  online: t('models.inCloud'),
}))
const stateIcon: Record<RowState, string> = { unverified: mdiAlertOutline, tested: mdiInformationOutline, contributed: mdiCheckCircleOutline, downloaded: mdiHarddisk, online: mdiCloudOutline }

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
const shareTarget = ref<DiscGeometryPreset | null>(null)
const deleteTarget = ref<DiscGeometryPreset | null>(null)
const editTarget = ref<DiscGeometryPreset | null>(null)
const editCode = ref('')
const editBrand = ref('')

let typing = 0
watch(query, (v) => {
  clearTimeout(typing)
  typing = window.setTimeout(() => (settled.value = v), DEBOUNCE_MS)
})

/** A calibration made here that is already in the library: same printed code and same measured pitch. Unverified entries count, so a fresh contribution shows. */
const sharedAs = (p: DiscGeometryPreset) =>
  issueOf(p.id) == null ? index.value.find((e) => normalizeCode(e.stamperCode) === normalizeCode(p.stamperCode) && e.basePitchUm === p.basePitchUm) : undefined

function localRow(preset: DiscGeometryPreset): Row {
  const shared = sharedAs(preset)
  const state: RowState = issueOf(preset.id) != null ? 'downloaded' : shared ? 'contributed' : preset.burnedAt ? 'tested' : 'unverified'
  return { id: preset.id, state, shared, look: preset, brand: preset.brand ?? '', size: sizeCm(preset), contributor: '', rating: shared?.rating ?? null, preset }
}

// Local rows and library rows together: an empty query lists both, downloading one moves it up.
const split = computed(() => {
  const want = Number(size.value) || 0
  const fits = (cm: number) => !want || cm === want
  // A standard geometry is not a calibration of this machine: the library page
  // lists what was measured here.
  const local = s.presets.filter((p) => !isStandardPreset(p) && fits(sizeCm(p)) && matches(p, settled.value))
  const known = new Set(local.flatMap((p) => [p.id, normalizeCode(p.stamperCode)]))
  const remote = index.value
    .filter((e) => e.verified && fits(e.size) && matches(e, settled.value) && !known.has(e.id) && !known.has(normalizeCode(e.stamperCode)))
    .sort(order.value === 'rating' ? byRating : (a, b) => b.createdAt.localeCompare(a.createdAt))
  return {
    local: local.map(localRow),
    remote: remote.map((entry): Row => ({ id: entry.id, state: 'online', look: entry, brand: entry.brand, size: entry.size, contributor: entry.contributor, rating: entry.rating, preset: null })),
  }
})
const rows = computed(() => [...split.value.local, ...split.value.remote])
const groups = computed(() =>
  [
    { key: 'local', title: t('models.mine'), rows: split.value.local },
    { key: 'remote', title: t('models.online'), rows: split.value.remote },
  ].filter((g) => g.rows.length),
)

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

// The drive names the disc it holds, so the library can be searched for exactly
// that disc without typing anything. What the user typed is never overwritten.
watch([() => s.view, () => s.drive?.mediaId], ([view, mid]) => {
  if (view !== 'models' || !mid || query.value) return
  query.value = mid
})

/** A download saves the geometry and stays put; the row turns local and can then be used. */
async function use(r: Row) {
  saving.value = r.id
  try {
    if (!r.preset) {
      await call('save_preset', { preset: await download(r.id) })
      await s.refreshPresets()
      notifySuccess(t('models.downloaded'))
      return
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

/** The geometry behind a row that is not on this machine yet: one folder, one request. */
async function download(id: string) {
  try {
    return await loadPreset(id)
  } catch (e) {
    throw String(e).includes('404') ? t('models.gone') : t('models.offline')
  }
}

/** A local calibration goes out as the same JSON the library publishes for it. */
async function exportPreset(preset: DiscGeometryPreset) {
  const name = (labelOf(preset) || preset.id).replace(/[\\/:*?"<>|]/g, '-')
  const path = await pickDataPath(`${name}.json`)
  if (!path) return
  try {
    await call('export_preset', { path, preset })
    notifySuccess(t('models.exported'))
  } catch (e) {
    notifyError(String(e))
  }
}

function viewShared(entry: CatalogEntry) {
  openInBrowser(issueUrl(entry)).catch((e) => notifyError(String(e)))
}

/**
 * A calibration is named by the code printed on the disc, so renaming one means correcting that code.
 * Only before it is shared: the library recognises a contribution by its code.
 */
function startEdit(preset: DiscGeometryPreset) {
  editTarget.value = preset
  editCode.value = preset.stamperCode
  editBrand.value = preset.brand ?? ''
}

async function saveEdit() {
  const preset = editTarget.value
  if (!preset) return
  editTarget.value = null
  try {
    await call('save_preset', { preset: { ...preset, stamperCode: editCode.value, brand: editBrand.value } })
    await s.refreshPresets()
  } catch (e) {
    notifyError(String(e))
  }
}

async function remove() {
  const id = deleteTarget.value?.id
  deleteTarget.value = null
  if (!id) return
  try {
    await call('delete_preset', { id })
    if (s.presetId === id) s.presetId = ''
    await s.refreshPresets()
    notifySuccess(t('presets.deleted'))
  } catch (e) {
    notifyError(String(e))
  }
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
.group { display: flex; flex-direction: column; gap: var(--oc-space-xs); }
.mark { flex: none; color: var(--oc-fg-muted); }
.mark.contributed { color: var(--oc-fg-success); }
.mark.tested { color: var(--oc-fg-accent); }
.mark.unverified { color: var(--oc-fg-warning); }
.badge.unverified { background: var(--oc-bg-warning-subtle); color: var(--oc-fg-warning); }
.text { min-width: 0; }
.fallback { display: flex; align-items: center; gap: var(--oc-space-sm); padding-top: var(--oc-space-md); border-top: 1px solid var(--oc-border-muted); }
.rating { display: inline-flex; align-items: center; gap: 2px; color: var(--oc-fg-accent); }
.rating svg { width: 14px; height: 14px; }
.field { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
.retry { flex: none; align-self: center; }
.badges { display: flex; align-items: center; flex-wrap: wrap; gap: var(--oc-space-2xs); margin-top: 2px; }
.badge { height: 18px; display: inline-flex; align-items: center; padding: 0 var(--oc-space-xs); border-radius: 9px; background: var(--oc-bg-raised); color: var(--oc-fg-muted); font-size: var(--oc-text-xs); white-space: nowrap; }
</style>
