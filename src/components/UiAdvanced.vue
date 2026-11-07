<template>
  <div class="adv">
    <button type="button" class="toggle small" :aria-expanded="open" @click="open = !open">
      <UiIcon :path="mdiChevronRight" :class="{ on: open }" />{{ t('common.advanced') }}
    </button>
    <div v-if="open" class="body"><slot /></div>
  </div>
</template>

<script setup lang="ts">
import { mdiChevronRight } from '@mdi/js'
import { ref } from 'vue'
import { t } from '../i18n'
import UiIcon from './UiIcon.vue'

const open = ref(false)
</script>

<style scoped>
.adv { display: flex; flex-direction: column; gap: var(--oc-space-sm); }
.toggle { align-self: flex-start; display: inline-flex; align-items: center; gap: 2px; height: var(--oc-size-sm); padding: 0 var(--oc-space-xs) 0 0; border: 0; border-radius: var(--oc-radius-control); background: transparent; color: var(--oc-fg-muted); }
.toggle:hover { color: var(--oc-fg-default); }
.toggle svg { transition: transform var(--oc-duration-quick) var(--oc-ease); }
.toggle svg.on { transform: rotate(90deg); }
.body { display: flex; flex-direction: column; gap: var(--oc-space-sm); }
</style>
