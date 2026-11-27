<template>
  <div class="sl">
    <div class="head">
      <span>{{ label }}</span>
      <span v-if="!ends || format" class="mono muted">{{ format ? format(modelValue) : modelValue }}</span>
    </div>
    <input type="range" :aria-label="label" :min="min" :max="max" :step="step" :value="modelValue" :disabled="disabled" :style="{ '--fill': `${((modelValue - min) / (max - min)) * 100}%` }" @input="$emit('update:modelValue', Number(($event.target as HTMLInputElement).value))" />
    <div v-if="ends" class="ends small muted"><span>{{ ends[0] }}</span><span>{{ ends[1] }}</span></div>
  </div>
</template>

<script setup lang="ts">
/**
 * `ends` names what each end gives, for a slider that trades one thing for another. Without a
 * `format` such a slider shows no number: the ends say what it does.
 */
defineProps<{ label: string; modelValue: number; min: number; max: number; step?: number; disabled?: boolean; format?: (v: number) => string; ends?: [string, string] }>()
defineEmits<{ 'update:modelValue': [number] }>()
</script>

<style scoped>
.sl { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
.head { display: flex; justify-content: space-between; }
.ends { display: flex; justify-content: space-between; gap: var(--oc-space-sm); }
input { appearance: none; width: 100%; height: 16px; margin: 0; background: transparent; }
input::-webkit-slider-runnable-track { height: 4px; border-radius: var(--oc-radius-pill); background: linear-gradient(to right, var(--oc-accent) var(--fill), var(--oc-bg-input) var(--fill)); }
input::-webkit-slider-thumb { appearance: none; width: 12px; height: 12px; margin-top: -4px; border-radius: 50%; background: #fff; box-shadow: var(--oc-shadow-sm); }
input:disabled { opacity: var(--oc-opacity-disabled); }
</style>
