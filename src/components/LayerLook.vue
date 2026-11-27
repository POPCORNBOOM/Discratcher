<template>
  <section class="section">
    <div class="title">{{ t('carve.tone') }}</div>
    <UiSwitch v-model="layer.invert">{{ t('carve.invert') }}</UiSwitch>
    <UiSlider v-model="layer.brightness" :label="t('carve.brightness')" :min="-100" :max="100" :step="1" />
    <UiSlider v-model="layer.contrast" :label="t('carve.contrast')" :min="-100" :max="100" :step="1" />
    <UiSwitch v-model="layer.pixelated">{{ t('carve.pixelated') }}</UiSwitch>
  </section>
  <section class="section">
    <div class="title">{{ t('carve.effects') }}</div>
    <UiSwitch v-model="layer.repeat">{{ t('carve.repeat') }}</UiSwitch>
    <template v-if="layer.repeat">
      <UiSlider v-model="layer.repeatDensity" :label="t('carve.repeatDensity')" :min="20" :max="100" :step="1" :format="(v) => `${Math.round(v)}%`" />
      <UiSlider v-model="layer.repeatAngle" :label="t('carve.repeatAngle')" :min="-90" :max="90" :step="0.5" :format="(v) => `${v.toFixed(1)}°`" />
      <UiSlider v-model="layer.repeatStagger" :label="t('carve.repeatStagger')" :min="0" :max="50" :step="1" :format="(v) => `${Math.round(v)}%`" />
    </template>
  </section>
</template>

<script setup lang="ts">
// How a picture's greys are carved, and its repeats: the carve page's and the hologram's flat
// pictures alike.
import { t } from '../i18n'
import type { Layer } from '../types'
import UiSlider from './UiSlider.vue'
import UiSwitch from './UiSwitch.vue'

defineProps<{ layer: Layer }>()
</script>
