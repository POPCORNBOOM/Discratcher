<template>
  <UiSwitch v-model="c.servoSafeDither">{{ t('carve.servoSafe') }}</UiSwitch>
  <UiSlider v-model="c.maxDarkDutyCycle" :label="t('carve.maxDuty')" :min="0.5" :max="0.85" :step="0.01" :disabled="!c.servoSafeDither" :format="(v) => `${Math.round(v * 100)}%`" :ends="[t('carve.maxDutyLow'), t('carve.maxDutyHigh')]" />
  <div v-if="c.servoSafeDither && c.maxDarkDutyCycle > MAX_SECTOR_DARK" class="notice danger">
    <UiIcon :path="mdiAlertOctagonOutline" />
    <div>{{ t('carve.dutyWarn') }}</div>
  </div>
  <div v-if="!c.servoSafeDither" class="notice danger">
    <UiIcon :path="mdiAlertOctagonOutline" />
    <div>{{ t('carve.servoWarn') }}</div>
  </div>
</template>

<script setup lang="ts">
// How dark carved pictures may get before the drive could lose its track: one setting for the
// carve page and the hologram's flat pictures.
import { mdiAlertOctagonOutline } from '@mdi/js'
import { t } from '../i18n'
import { useStudio } from '../stores/studio'
import UiIcon from './UiIcon.vue'
import UiSlider from './UiSlider.vue'
import UiSwitch from './UiSwitch.vue'

/** The servo safety line the build measures against; going past it is allowed but warned. */
const MAX_SECTOR_DARK = 0.7
const c = useStudio().config
</script>
