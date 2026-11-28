<template>
  <section class="ui-section" :class="{ open }">
    <div class="head">
      <button type="button" class="toggle" :aria-expanded="open" @click="toggle">
        <UiIcon :path="mdiChevronRight" :size="14" class="chevron" :class="{ on: open }" />
        <UiIcon v-if="icon" :path="icon" :size="16" class="icon" />
        <span class="name">{{ title }}</span>
      </button>
      <slot v-if="open" name="actions" />
    </div>
    <!-- Kept in place while closed so opening and closing can slide; inert keeps it out of reach. -->
    <div class="fold" :inert="!open">
      <div class="clip"><div class="body"><slot /></div></div>
    </div>
  </section>
</template>

<script setup lang="ts">
// One group of settings in a `UiAccordion`; opening it closes the others.
import { mdiChevronRight } from '@mdi/js'
import { computed, inject, ref } from 'vue'
import { ACCORDION } from './accordion'
import UiIcon from './UiIcon.vue'

const props = defineProps<{ name: string; title: string; icon?: string }>()
const current = inject(ACCORDION, ref(''))
const open = computed(() => current.value === props.name)

function toggle() {
  current.value = open.value ? '' : props.name
}
</script>

<style scoped>
.ui-section { --fold: 280ms; flex: none; display: flex; flex-direction: column; }
/* The open section takes what room is left, its heading staying put while its body scrolls. */
.ui-section.open { flex: 0 1 auto; min-height: 0; }
.head { display: flex; align-items: center; gap: var(--oc-space-sm); padding: 0 var(--oc-space-md) 0 var(--oc-space-sm); }
.head:hover { background: var(--oc-bg-hover); }
.toggle { flex: 1; min-width: 0; display: inline-flex; align-items: center; gap: var(--oc-space-xs); height: 36px; padding: 0; border: 0; background: transparent; color: var(--oc-fg-default); font-size: var(--oc-text-lg); font-weight: 600; text-align: left; }
.toggle:hover .chevron { color: var(--oc-fg-default); }
.toggle:focus-visible { outline: none; box-shadow: var(--oc-focus-ring); border-radius: var(--oc-radius-control); }
.chevron { color: var(--oc-fg-muted); transition: transform var(--fold) var(--oc-ease); }
.chevron.on { transform: rotate(90deg); }
.name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
/* Child combinators so a section nested in an open one still folds. Grows from no height to the content's own, so the open section pushes the rest down smoothly. */
.fold { flex: 0 1 auto; min-height: 0; display: grid; grid-template-rows: 0fr; transition: grid-template-rows var(--fold) var(--oc-ease); }
.open > .fold { grid-template-rows: 1fr; }
.clip { min-height: 0; overflow: hidden; }
.open > .fold > .clip { overflow-y: auto; overscroll-behavior: contain; }
.body { opacity: 0; transform: translateY(-4px); transition: opacity var(--fold) var(--oc-ease), transform var(--fold) var(--oc-ease); }
.open > .fold > .clip > .body { opacity: 1; transform: none; }
@media (prefers-reduced-motion: reduce) { .fold, .body, .chevron { transition: none; } }
/* Under the heading, in from the chevron. */
.body { display: flex; flex-direction: column; gap: var(--oc-space-md); padding: var(--oc-space-2xs) var(--oc-space-md) var(--oc-space-lg) calc(var(--oc-space-sm) + 14px + var(--oc-space-xs)); }
</style>
