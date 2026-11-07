<template>
  <div class="sl">
    <div class="head">
      <span>{{ label }}</span>
      <span class="mono muted">{{ format ? format(modelValue) : modelValue }}</span>
    </div>
    <input type="range" :aria-label="label" :min="min" :max="max" :step="step" :value="modelValue" :disabled="disabled" :style="{ '--fill': `${((modelValue - min) / (max - min)) * 100}%` }" @input="$emit('update:modelValue', Number(($event.target as HTMLInputElement).value))" />
  </div>
</template>

<script setup lang="ts">
defineProps<{ label: string; modelValue: number; min: number; max: number; step?: number; disabled?: boolean; format?: (v: number) => string }>()
defineEmits<{ 'update:modelValue': [number] }>()
</script>

<style scoped>
.sl { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
.head { display: flex; justify-content: space-between; }
input { appearance: none; width: 100%; height: 16px; margin: 0; background: transparent; }
input::-webkit-slider-runnable-track { height: 4px; border-radius: var(--oc-radius-pill); background: linear-gradient(to right, var(--oc-accent) var(--fill), var(--oc-bg-input) var(--fill)); }
input::-webkit-slider-thumb { appearance: none; width: 12px; height: 12px; margin-top: -4px; border-radius: 50%; background: #fff; box-shadow: var(--oc-shadow-sm); }
input:disabled { opacity: var(--oc-opacity-disabled); }
</style>
