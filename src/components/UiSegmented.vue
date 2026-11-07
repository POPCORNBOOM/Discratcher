<template>
  <div class="seg" role="radiogroup">
    <button v-for="o in options" :key="o.value" type="button" role="radio" :aria-checked="o.value === modelValue" :class="{ on: o.value === modelValue }" @click="$emit('update:modelValue', o.value)">{{ o.label }}</button>
  </div>
</template>

<script setup lang="ts">
defineProps<{ modelValue: string; options: { value: string; label: string }[] }>()
defineEmits<{ 'update:modelValue': [string] }>()
</script>

<style scoped>
.seg { display: flex; padding: 2px; gap: 2px; background: var(--oc-bg-base); border-radius: var(--oc-radius-control); }
button { flex: 1; height: 24px; white-space: nowrap; border: 0; border-radius: 4px; background: transparent; color: var(--oc-fg-muted); font-size: var(--oc-text-sm); }
button:hover { background-image: linear-gradient(var(--oc-bg-hover), var(--oc-bg-hover)); }
.on { background: var(--oc-bg-raised); color: var(--oc-fg-default); }
</style>
