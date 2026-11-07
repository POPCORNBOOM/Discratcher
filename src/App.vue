<template>
  <div class="shell">
    <TitleBar />
    <div class="body">
      <nav class="rail" :aria-label="t('nav.label')" data-tooltip-placement="right" data-tooltip-group>
        <button v-for="n in nav" :key="n.id" type="button" :class="{ on: n.id === active }" :aria-label="n.label" :aria-current="n.id === active ? 'page' : undefined" :data-tooltip="n.label" @click="s.view = n.id">
          <UiIcon :path="n.icon" :size="20" />
        </button>
      </nav>
      <main class="content">
        <DiscStage />
        <HomeView v-if="s.view === 'home'" />
        <ModelsView v-show="s.view === 'models'" />
        <CalibrateView v-show="s.view === 'calibrate'" />
        <CarveView v-show="s.view === 'carve'" />
        <ProcessView v-if="s.view === 'process'" />
      </main>
    </div>
  </div>
</template>

<script setup lang="ts">
import { mdiCrosshairsGps, mdiDisc, mdiHomeOutline, mdiImageEditOutline } from '@mdi/js'
import { computed, onBeforeUnmount, onMounted } from 'vue'
import { t } from './i18n'
import DiscStage from './components/DiscStage.vue'
import TitleBar from './components/TitleBar.vue'
import UiIcon from './components/UiIcon.vue'
import { useProcess } from './stores/process'
import { useStudio, type View } from './stores/studio'
import { startUpdater } from './updater'
import CalibrateView from './views/CalibrateView.vue'
import CarveView from './views/CarveView.vue'
import HomeView from './views/HomeView.vue'
import ModelsView from './views/ModelsView.vue'
import ProcessView from './views/ProcessView.vue'

/** Often enough that inserting a disc feels noticed, with no refresh button to press. */
const DRIVE_POLL_MS = 3000

const s = useStudio()
const process = useProcess()
const nav = computed<{ id: View; label: string; icon: string }[]>(() => [
  { id: 'home', label: t('nav.home'), icon: mdiHomeOutline },
  { id: 'models', label: t('nav.models'), icon: mdiDisc },
  { id: 'calibrate', label: t('nav.calibrate'), icon: mdiCrosshairsGps },
  { id: 'carve', label: t('nav.carve'), icon: mdiImageEditOutline },
])
// The process page belongs to whichever page started it.
const active = computed(() => (s.view === 'process' ? s.origin : s.view))
let poll = 0
onMounted(async () => {
  await s.refreshPresets()
  await s.refreshDrives()
  process.listen()
  void startUpdater()
  poll = window.setInterval(() => s.job || s.refreshDrives(), DRIVE_POLL_MS)
})
onBeforeUnmount(() => clearInterval(poll))
</script>

<style scoped>
.shell { height: 100%; display: flex; flex-direction: column; }
.body { flex: 1; min-height: 0; display: flex; }
.rail { width: var(--oc-rail-width); flex: none; padding: var(--oc-space-xs); display: flex; flex-direction: column; gap: var(--oc-space-xs); background: var(--oc-bg-block); border-right: 1px solid var(--oc-border-muted); }
.rail button { width: var(--oc-size-lg); height: var(--oc-size-lg); flex: none; border: 0; padding: 0; border-radius: var(--oc-radius-control); background: transparent; color: var(--oc-fg-muted); display: grid; place-items: center; transition: background-color var(--oc-duration) var(--oc-ease), color var(--oc-duration) var(--oc-ease); }
.rail button:hover { background-color: var(--oc-bg-hover); }
.rail button.on { background-color: var(--oc-bg-active); color: var(--oc-fg-accent); }
.content { position: relative; flex: 1; min-width: 0; display: flex; background: #060607; }
.content > :not(canvas, .lens) { position: relative; }
</style>
