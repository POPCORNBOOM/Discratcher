<template>
  <UiSelect v-if="s.drives.length > 1" v-model="s.driveKey" :label="t('drive.label')" :options="options" :disabled="!!s.job" />
  <div class="small muted">{{ status }}</div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { t } from '../i18n'
import { useStudio } from '../stores/studio'
import UiSelect from './UiSelect.vue'

const s = useStudio()
// The model name only matters when it has to tell two drives apart.
const options = computed(() => s.drives.map((d) => ({ value: s.keyOf(d), label: d.simulated ? d.product : `${d.vendor} ${d.product}` })))
const status = computed(() => {
  const d = s.drive
  if (!d) return t('drive.none')
  if (!d.mediaPresent) return t('drive.noDisc')
  // A closed DVD-R answers as DVD-ROM, so the drive's report alone would call a disc
  // it can measure unsupported. The inspection knows better.
  if (!d.supported && s.discKind !== 'calibration') return t('drive.unsupported', { type: d.mediaType })
  return t(d.isBlank ? 'drive.blank' : 'drive.written')
})
</script>
