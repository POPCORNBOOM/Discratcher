<template>
  <div class="back" @mousedown.self="$emit('close')">
    <div class="dlg" role="dialog" aria-modal="true" :aria-label="title">
      <div class="title">{{ title }}</div>
      <div class="body"><slot /></div>
      <div class="foot"><slot name="actions" /></div>
    </div>
  </div>
</template>

<script setup lang="ts">
defineProps<{ title: string }>()
defineEmits<{ close: [] }>()
</script>

<style scoped>
.back { position: fixed; inset: 0; z-index: 1000; display: grid; place-items: center; background: var(--oc-bg-modal-backdrop); }
.dlg { width: 520px; background: var(--oc-bg-surface); border: 1px solid var(--oc-border-default); border-radius: var(--oc-radius-surface); box-shadow: var(--oc-shadow-lg); padding: var(--oc-space-lg); display: flex; flex-direction: column; gap: var(--oc-space-md); }
.body { display: flex; flex-direction: column; gap: var(--oc-space-sm); }
.foot { display: flex; justify-content: flex-end; gap: var(--oc-space-sm); }
</style>
