<template>
  <div class="ui-accordion"><slot /></div>
</template>

<script setup lang="ts">
// Groups of settings stacked as one card, with one group open at a time.
import { provide } from 'vue'
import { ACCORDION } from './accordion'

/** The `name` of the open `UiSection`, or '' when all are closed. */
const open = defineModel<string>({ default: '' })
provide(ACCORDION, open)
</script>

<style scoped>
/* Never taller than the room it is given: the open section scrolls inside instead. */
.ui-accordion { flex: 0 1 auto; min-height: 0; display: flex; flex-direction: column; border: 1px solid var(--oc-border-muted); border-radius: var(--oc-radius-surface); background: var(--oc-bg-block); overflow: hidden; }
.ui-accordion > :deep(.ui-section + .ui-section) { border-top: 1px solid var(--oc-border-muted); }
</style>
