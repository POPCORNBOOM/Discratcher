<template>
  <UiSelect v-if="s.drives.length > 1" v-model="s.driveKey" :label="t('drive.label')" :options="options" :disabled="!!s.job" />
  <div v-if="!bare" class="small muted">{{ status }}<span v-if="mid" class="mono mid" :data-tooltip="t('drive.midTip')"> · {{ mid }}</span></div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { t } from '../i18n'
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
  if (!d) return t(s.driveStuck ? 'drive.stuck' : s.driveBusy ? 'drive.busy' : 'drive.none')
  switch (d.disc) {
    case 'none': return t('drive.noDisc')
    case 'unsupported': return t('drive.unsupported', { type: d.mediaType })
    case 'blank': return t('drive.blank')
    case 'erasable': return t('drive.rewritable')
    default: return t('drive.written')
  }
})
</script>

<style scoped>
.mid { margin-left: 0.3em; }
</style>
