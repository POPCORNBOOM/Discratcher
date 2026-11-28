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
    <UiSwitch :model-value="layer.ring" @update:model-value="setRing">{{ t('carve.ring') }}</UiSwitch>
    <UiSlider v-if="layer.ring" v-model="layer.ringTurns" :label="t('carve.ringTurns')" :min="1" :max="MAX_RING_TURNS" :step="1" :format="(v) => t('carve.turns', { n: Math.round(v) })" />
    <UiSwitch v-model="layer.repeat">{{ t('carve.repeat') }}</UiSwitch>
    <template v-if="layer.repeat">
      <UiSlider v-model="layer.repeatDensity" :label="t('carve.repeatDensity')" :min="20" :max="100" :step="1" :format="(v) => `${Math.round(v)}%`" />
      <UiSlider v-model="layer.repeatAngle" :label="t('carve.repeatAngle')" :min="-90" :max="90" :step="0.5" :format="(v) => `${v.toFixed(1)}°`" />
      <UiSlider v-model="layer.repeatStagger" :label="t('carve.repeatStagger')" :min="0" :max="50" :step="1" :format="(v) => `${Math.round(v)}%`" />
    </template>
  </section>
</template>

<script setup lang="ts">
// How a picture's greys are carved, and its ring and repeats: the carve page's and the hologram's flat
// pictures alike.
import { t } from '../i18n'
import { MAX_RING_TURNS, ringFit } from '../ringMap'
import type { Layer } from '../types'
import UiSlider from './UiSlider.vue'
import UiSwitch from './UiSwitch.vue'

const props = defineProps<{ layer: Layer }>()

/** Wrapping round the ring starts the picture centred on the band, over the turns that suit it. */
function setRing(on: boolean) {
  Object.assign(props.layer, on ? { ...ringFit(props.layer), ring: true } : { ring: false })
}
</script>
