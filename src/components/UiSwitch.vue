<template>
  <label class="sw">
    <span class="text"><slot /></span>
    <input type="checkbox" role="switch" :checked="modelValue" :disabled="disabled" @change="$emit('update:modelValue', ($event.target as HTMLInputElement).checked)" />
    <i />
  </label>
</template>

<script setup lang="ts">
defineProps<{ modelValue: boolean; disabled?: boolean }>()
defineEmits<{ 'update:modelValue': [boolean] }>()
</script>

<style scoped>
.sw { display: flex; align-items: center; gap: var(--oc-space-sm); min-height: var(--oc-size-md); }
.text { flex: 1; }
input { position: absolute; opacity: 0; width: 0; height: 0; }
i { position: relative; width: 28px; height: 16px; border-radius: var(--oc-radius-pill); background: var(--oc-bg-input); transition: background var(--oc-duration-quick) var(--oc-ease); }
i::after { content: ''; position: absolute; top: 2px; left: 2px; width: 12px; height: 12px; border-radius: 50%; background: var(--oc-fg-default); box-shadow: var(--oc-shadow-sm); transition: transform var(--oc-duration) var(--oc-ease); }
input:checked + i { background: var(--oc-bg-accent); }
input:checked + i::after { transform: translateX(12px); background: #fff; }
input:focus-visible + i { box-shadow: var(--oc-focus-ring); }
input:disabled + i { opacity: var(--oc-opacity-disabled); }
</style>
