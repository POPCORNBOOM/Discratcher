<template>
  <section class="section">
    <div class="title">{{ t('presets.title') }}</div>
    <div class="row">
      <UiSelect class="grow" filterable :label="t('presets.label')" :placeholder="t('presets.none')" :model-value="s.preset?.id ?? ''" :options="options" :disabled="!!s.job" @update:model-value="s.presetId = $event" @preview="preview" />
      <UiButton v-if="s.preset" icon-only variant="ghost" :icon="mdiTrashCanOutline" :aria-label="t('presets.delete', { name: s.preset.name })" :disabled="!!s.job" @click="confirmId = s.preset.id" />
    </div>
    <div v-if="matched" class="small muted">{{ t('presets.matched') }}</div>
    <a role="button" tabindex="0" class="small" @click="s.view = 'calibrate'" @keydown.enter="s.view = 'calibrate'">{{ t('presets.missing') }}</a>

    <UiDialog v-if="confirmId" :title="t('presets.deleteTitle')" @close="confirmId = ''">
      <p>{{ t('presets.deleteBody', { name: s.presets.find((p) => p.id === confirmId)?.name ?? '' }) }}</p>
      <template #actions>
        <UiButton variant="ghost" @click="confirmId = ''">{{ t('common.cancel') }}</UiButton>
        <UiButton variant="danger" @click="remove">{{ t('common.delete') }}</UiButton>
      </template>
    </UiDialog>
  </section>
</template>

<script setup lang="ts">
import { mdiTrashCanOutline } from '@mdi/js'
import { computed, ref } from 'vue'
import { call } from '../api'
import { t } from '../i18n'
import { notifyError, notifySuccess } from '../notices'
import { useStudio } from '../stores/studio'
import UiButton from './UiButton.vue'
import UiDialog from './UiDialog.vue'
import UiSelect from './UiSelect.vue'

const s = useStudio()
const confirmId = ref('')
const options = computed(() => s.presets.map((p) => ({ value: p.id, label: p.name, keywords: `${p.stamperCode} ${p.mediaId} ${p.brand ?? ''}` })))
const preview = (id: string | null) => (s.discPreview = s.presets.find((p) => p.id === id) ?? null)
const matched = computed(() => !!s.drive?.mediaId && s.drive.mediaId === s.preset?.mediaId)

async function remove() {
  const id = confirmId.value
  confirmId.value = ''
  try {
    await call('delete_preset', { id })
    if (s.presetId === id) s.presetId = ''
    await s.refreshPresets()
    notifySuccess(t('presets.deleted'))
  } catch (e) {
    notifyError(String(e))
  }
}
</script>

<style scoped>
a { color: var(--oc-fg-accent); cursor: pointer; align-self: flex-start; }
</style>
