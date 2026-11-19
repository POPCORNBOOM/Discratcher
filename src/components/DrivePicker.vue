<template>
  <UiSelect v-if="s.drives.length > 1" v-model="s.driveKey" :label="t('drive.label')" :options="options" :disabled="!!s.job" />
  <div v-if="!bare" class="small muted">{{ status }}<span v-if="mid" class="mono mid" :data-tooltip="t('drive.midTip')"> · {{ mid }}</span></div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { t, unsupportedText } from '../i18n'
import { isDvdRam } from '../etaModel'
import { useStudio } from '../stores/studio'
import UiSelect from './UiSelect.vue'

/** `bare` leaves the disc's state to the page, which has more to say about it. */
defineProps<{ bare?: boolean }>()
const s = useStudio()
// The model name only matters when it has to tell two drives apart.
const options = computed(() => s.drives.map((d) => ({ value: s.keyOf(d), label: `${d.vendor} ${d.product}` })))
// The disc names itself; the drive only says what it can see of it.
const mid = computed(() => s.drive?.mediaId ?? '')
const status = computed(() => {
  const d = s.drive
  if (!d) return t(s.driveStuck ? 'drive.stuck' : 'drive.none')
  if (!d.mediaPresent) return t('drive.noDisc')
  if (!d.supported) return unsupportedText(d)
  // A DVD-RAM is RAM media: the burn overwrites it, it is not erased first.
  if (isDvdRam(d)) return t('drive.ram')
  if (d.reusable) return t('drive.rewritable')
  return t(d.isBlank ? 'drive.blank' : 'drive.written')
})
</script>

<style scoped>
.mid { margin-left: 0.3em; }
</style>
