<template>
  <UiDialog :title="t('share.title')" @close="$emit('close')">
    <p>{{ t('share.intro') }}</p>
    <label class="field">
      <span class="small muted">{{ t('share.code') }}</span>
      <input v-model.trim="code" :placeholder="t('share.codeHint')" spellcheck="false" autocomplete="off" />
    </label>
    <label class="field">
      <span class="small muted">{{ t('share.brand') }}</span>
      <input v-model.trim="brand" :placeholder="t('share.brandHint')" spellcheck="false" autocomplete="off" />
    </label>
    <label class="pledge small">
      <input v-model="pledged" type="checkbox" />
      <span>{{ t('share.pledge') }}</span>
    </label>
    <p class="small muted">{{ t('share.note') }}</p>
    <template #actions>
      <UiButton variant="ghost" @click="$emit('close')">{{ t('share.later') }}</UiButton>
      <UiButton variant="solid" :disabled="!code || !pledged" @click="contribute">{{ t('share.submit') }}</UiButton>
    </template>
  </UiDialog>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { isMock, openInBrowser } from '../api'
import { contributionUrl } from '../catalog'
import { t } from '../i18n'
import { notifyError, notifySuccess } from '../notices'
import type { DiscGeometryPreset } from '../types'
import UiButton from './UiButton.vue'
import UiDialog from './UiDialog.vue'

const BRAND_KEY = 'discratcher.brand'

const props = defineProps<{ preset: DiscGeometryPreset }>()
const emit = defineEmits<{ close: [] }>()

const code = ref(props.preset.stamperCode)
const brand = ref(props.preset.brand || readBrand())
const pledged = ref(false)

function readBrand() {
  try {
    return localStorage.getItem(BRAND_KEY) ?? ''
  } catch {
    return ''
  }
}

async function contribute() {
  const shared = { ...props.preset, brand: brand.value, stamperCode: code.value }
  try {
    localStorage.setItem(BRAND_KEY, brand.value)
  } catch {
    // Only a convenience for next time.
  }
  try {
    await openInBrowser(contributionUrl(shared))
    if (isMock) notifySuccess(t('share.mockDone'))
    emit('close')
  } catch (e) {
    notifyError(String(e))
  }
}
</script>

<style scoped>
.field { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
.field input { height: var(--oc-size-md); padding: 0 var(--oc-space-sm); border: 0; border-radius: var(--oc-radius-control); background: var(--oc-bg-raised); }
.field input::placeholder { color: var(--oc-fg-muted); }
.pledge { display: flex; align-items: flex-start; gap: var(--oc-space-sm); cursor: pointer; line-height: 1.5; }
.pledge input { flex: none; width: 14px; height: 14px; margin: 3px 0 0; accent-color: var(--oc-bg-accent); cursor: pointer; }
</style>
