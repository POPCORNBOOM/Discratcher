<template>
  <button class="btn" :class="[variant, size, { icon: iconOnly }]" :type="type" :disabled="disabled" :data-tooltip="iconOnly ? $attrs['aria-label'] : undefined">
    <UiIcon v-if="icon" :path="icon" />
    <span v-if="$slots.default" class="label"><slot /></span>
  </button>
</template>

<script setup lang="ts">
import UiIcon from './UiIcon.vue'

withDefaults(
  defineProps<{ variant?: 'solid' | 'soft' | 'ghost' | 'danger'; size?: 'md' | 'lg'; type?: 'button' | 'submit'; icon?: string; iconOnly?: boolean; disabled?: boolean }>(),
  { variant: 'soft', size: 'md', type: 'button' },
)
</script>

<style scoped>
.btn {
  height: var(--oc-size-md);
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: var(--oc-space-xs);
  padding: 0 var(--oc-space-md);
  border: 0;
  border-radius: var(--oc-radius-control);
  background-color: transparent;
  white-space: nowrap;
  transition: background-color var(--oc-duration-quick) var(--oc-ease);
}
.btn:hover:not(:disabled) { background-image: linear-gradient(var(--oc-bg-hover), var(--oc-bg-hover)); }
.btn:disabled { opacity: var(--oc-opacity-disabled); cursor: default; }
.solid { background-color: var(--oc-bg-accent); color: var(--oc-accent-fg); }
.solid:hover:not(:disabled) { background-image: none; background-color: var(--oc-bg-accent-hover); }
.soft { background-color: var(--oc-bg-raised); }
.danger { background-color: var(--oc-bg-danger-subtle); color: var(--oc-fg-danger); }
.icon { width: var(--oc-size-md); padding: 0; }
.lg { height: var(--oc-size-lg); padding: 0 var(--oc-space-lg); }
</style>
