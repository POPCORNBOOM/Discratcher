<template>
  <section class="section">
    <div class="title row">
      <span class="grow">{{ t('carve.position') }}</span>
      <a class="action-link" role="button" tabindex="0" @click="$emit('reset')" @keydown.enter="$emit('reset')">{{ t('carve.reset') }}</a>
    </div>
    <UiSlider v-model="percent.x" :label="t('carve.x')" :min="-50" :max="50" :step="0.5" :format="formatOffset" />
    <UiSlider v-model="percent.y" :label="t('carve.y')" :min="-50" :max="50" :step="0.5" :format="formatOffset" />
    <UiSlider v-model="percent.size" :label="t('carve.size')" :min="5" :max="300" :step="1" :format="(v) => `${Math.round(v)}%`" />
    <UiSlider v-model="percent.stretchX" :label="t('carve.stretchX')" :min="10" :max="400" :step="1" :format="(v) => `${Math.round(v)}%`" />
    <UiSlider v-model="percent.stretchY" :label="t('carve.stretchY')" :min="10" :max="400" :step="1" :format="(v) => `${Math.round(v)}%`" />
    <UiSlider v-model="layer.rotationDeg" :label="t('carve.rotate')" :min="-180" :max="180" :step="0.5" :format="(v) => `${v.toFixed(1)}°`" />
  </section>
</template>

<script setup lang="ts">
// Where a picture sits on the disc, in percent of the diameter: the carve page's and the
// hologram's flat pictures alike.
import { computed, reactive } from 'vue'
import { t } from '../i18n'
import type { Layer } from '../types'
import UiSlider from './UiSlider.vue'

const props = defineProps<{ layer: Layer }>()
defineEmits<{ reset: [] }>()

const formatOffset = (v: number) => (v > 0 ? `+${v.toFixed(1)}%` : `${v.toFixed(1)}%`)
const percent = reactive({
  x: computed({ get: () => props.layer.x * 100, set: (v) => (props.layer.x = v / 100) }),
  y: computed({ get: () => props.layer.y * 100, set: (v) => (props.layer.y = v / 100) }),
  size: computed({ get: () => props.layer.size * 100, set: (v) => (props.layer.size = v / 100) }),
  stretchX: computed({ get: () => (props.layer.stretchX ?? 1) * 100, set: (v) => (props.layer.stretchX = v / 100) }),
  stretchY: computed({ get: () => (props.layer.stretchY ?? 1) * 100, set: (v) => (props.layer.stretchY = v / 100) }),
})
</script>
