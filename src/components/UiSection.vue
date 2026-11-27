<template>
  <section class="ui-section">
    <div class="head">
      <button type="button" class="toggle" :aria-expanded="open" @click="open = !open">
        <UiIcon :path="mdiChevronRight" :size="14" class="chevron" :class="{ on: open }" />
        <UiIcon v-if="icon" :path="icon" :size="16" class="icon" />
        <span class="name">{{ title }}</span>
      </button>
      <slot name="actions" />
    </div>
    <div v-if="open" class="body"><slot /></div>
  </section>
</template>

<script setup lang="ts">
// A group of settings under a heading that folds away.
import { mdiChevronRight } from '@mdi/js'
import { ref } from 'vue'
import UiIcon from './UiIcon.vue'

const props = defineProps<{ title: string; icon?: string; folded?: boolean }>()
const open = ref(!props.folded)
</script>

<style scoped>
.ui-section { display: flex; flex-direction: column; gap: var(--oc-space-sm); }
.head { display: flex; align-items: center; gap: var(--oc-space-sm); }
.toggle { flex: 1; min-width: 0; display: inline-flex; align-items: center; gap: var(--oc-space-xs); height: var(--oc-size-md); padding: 0; border: 0; background: transparent; color: var(--oc-fg-default); font-size: var(--oc-text-lg); font-weight: 600; text-align: left; }
.toggle:hover .chevron { color: var(--oc-fg-default); }
.toggle:focus-visible { outline: none; box-shadow: var(--oc-focus-ring); border-radius: var(--oc-radius-control); }
.chevron { color: var(--oc-fg-muted); transition: transform var(--oc-duration-quick) var(--oc-ease); }
.chevron.on { transform: rotate(90deg); }
.name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
/* Under the heading, in from the chevron. */
.body { display: flex; flex-direction: column; gap: var(--oc-space-sm); padding-left: calc(14px + var(--oc-space-xs)); }
</style>
