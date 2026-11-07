<template>
  <header class="titlebar" data-tauri-drag-region="deep">
    <div class="left">
      <UiButton icon-only variant="ghost" :icon="mdiArrowLeft" :aria-label="t('titlebar.back')" :disabled="!s.canBack" @click="s.back()" />
      <UiMenubar :menus="menus" />
    </div>

    <div class="middle">
      <div class="brand" :class="{ busy: tasks.length }" :tabindex="tasks.length ? 0 : undefined" @pointerenter="openTasks" @pointerleave="closeTasks" @focusin="openTasks" @focusout="closeTasks" @click="tasks.length && (s.view = 'process')">
        <BrandMark class="mark" />
        <span>Discratcher</span>
        <section v-if="taskPanel && tasks.length" class="panel tasks" aria-live="polite" data-tauri-drag-region="false">
          <div v-for="t in tasks" :key="t.key" class="task">
            <div class="row small"><span class="grow ellipsis">{{ t.title }}</span><span class="mono muted">{{ Math.round(t.progress * 100) }}%</span></div>
            <div v-if="t.detail" class="small muted ellipsis">{{ t.detail }}</div>
            <div class="track"><i :style="{ width: `${t.progress * 100}%` }" /></div>
          </div>
        </section>
      </div>
    </div>

    <div class="right">
      <div class="notice-host" @pointerenter="noticePanel = notices.length > 0" @pointerleave="noticePanel = false">
        <div v-if="latest" :key="latest.id" class="flash" :class="[latest.tone, { faded }]" role="status" aria-live="polite">
          <UiIcon :path="latest.icon" /><span class="ellipsis">{{ latest.message }}</span>
        </div>
        <section v-if="noticePanel" class="panel history selectable" :aria-label="t('titlebar.history')" data-tauri-drag-region="false">
          <div v-for="n in [...notices].reverse()" :key="n.id" class="line small" :class="n.tone"><UiIcon :path="n.icon" /><span>{{ n.message }}</span></div>
        </section>
      </div>
    </div>

    <div v-if="overallProgress != null" class="titlebar-progress" role="progressbar" :aria-label="t('titlebar.progress')" aria-valuemin="0" aria-valuemax="100" :aria-valuenow="Math.round(overallProgress * 100)"><i :style="{ width: `${overallProgress * 100}%` }" /></div>
  </header>
</template>

<script setup lang="ts">
import { mdiArrowLeft, mdiCheck, mdiTranslate, mdiCrosshairsGps, mdiExportVariant, mdiFire, mdiFolderOpenOutline, mdiGithub, mdiHomeOutline, mdiImageEditOutline, mdiMagnify, mdiMessageAlertOutline } from '@mdi/js'
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { openInBrowser } from '../api'
import { REPOSITORY_URL } from '../catalog'
import { locale, locales, setLocale, t } from '../i18n'
import { notices } from '../notices'
import { useStudio } from '../stores/studio'
import { overallProgress, tasks } from '../tasks'
import BrandMark from './BrandMark.vue'
import UiButton from './UiButton.vue'
import UiIcon from './UiIcon.vue'
import UiMenubar, { type Menu } from './UiMenubar.vue'

const FADE_MS = 2200
const ERROR_FADE_MS = 6000
const PANEL_CLOSE_DELAY_MS = 120

const s = useStudio()
const taskPanel = ref(false)
const noticePanel = ref(false)
const faded = ref(false)
let fadeTimer = 0
let closeTimer = 0

const latest = computed(() => notices.value[notices.value.length - 1] ?? null)
watch(latest, (n) => {
  clearTimeout(fadeTimer)
  faded.value = false
  if (n) fadeTimer = window.setTimeout(() => (faded.value = true), n.tone === 'danger' ? ERROR_FADE_MS : FADE_MS)
})

function openTasks() {
  clearTimeout(closeTimer)
  taskPanel.value = tasks.value.length > 0
}
function closeTasks() {
  clearTimeout(closeTimer)
  closeTimer = window.setTimeout(() => (taskPanel.value = false), PANEL_CLOSE_DELAY_MS)
}

const menus = computed<Menu[]>(() => [
  {
    key: 'file',
    label: t('menu.file'),
    actions: [
      { key: 'home', title: t('menu.home'), icon: mdiHomeOutline, run: () => (s.view = 'home') },
      { key: 'd1', divider: true },
      { key: 'new', title: t('menu.new'), icon: mdiImageEditOutline, run: () => (s.view = 'carve') },
      { key: 'open', title: t('menu.open'), icon: mdiFolderOpenOutline, disabled: !s.carve, run: () => { s.view = 'carve'; s.carve?.choose() } },
      { key: 'export', title: t('menu.export'), icon: mdiExportVariant, disabled: !s.carveReady, run: () => s.carve?.exportIso() },
    ],
  },
  {
    key: 'disc',
    label: t('menu.disc'),
    actions: [
      { key: 'models', title: t('menu.models'), icon: mdiMagnify, run: () => (s.view = 'models') },
      { key: 'calibrate', title: t('menu.calibrate'), icon: mdiCrosshairsGps, run: () => (s.view = 'calibrate') },
      { key: 'd1', divider: true },
      { key: 'burn', title: t('menu.burn'), icon: mdiFire, disabled: !s.carveReady, run: () => s.carve?.burn() },
    ],
  },
  {
    key: 'help',
    label: t('menu.help'),
    actions: [
      { key: 'repo', title: t('menu.repo'), icon: mdiGithub, run: () => void openInBrowser(`${REPOSITORY_URL}/issues?q=label%3Acalibration`) },
      { key: 'issue', title: t('menu.issue'), icon: mdiMessageAlertOutline, run: () => void openInBrowser(`${REPOSITORY_URL}/issues/new`) },
      { key: 'd1', divider: true },
      {
        key: 'language',
        title: t('menu.language'),
        icon: mdiTranslate,
        children: locales.map((l) => ({ key: l.code, title: l.name, icon: l.code === locale.value ? mdiCheck : undefined, run: () => setLocale(l.code) })),
      },
    ],
  },
])

function onShortcut(e: KeyboardEvent) {
  if (e.metaKey && e.key === '[') {
    e.preventDefault()
    s.back()
  }
}
onMounted(() => window.addEventListener('keydown', onShortcut))
onBeforeUnmount(() => window.removeEventListener('keydown', onShortcut))
</script>

<style scoped>
.titlebar { position: relative; height: var(--oc-shell-titlebar-height); flex: none; display: grid; grid-template-columns: 1fr auto 1fr; align-items: center; gap: var(--oc-space-sm); padding: 0 var(--oc-space-md) 0 84px; background: var(--oc-bg-block); border-bottom: 1px solid var(--oc-border-muted); z-index: 10; }
.left { display: flex; align-items: center; gap: var(--oc-space-2xs); }
.middle { display: flex; justify-content: center; }
.brand { position: relative; display: inline-flex; align-items: center; gap: var(--oc-space-xs); height: var(--oc-size-md); padding: 0 var(--oc-space-sm); border-radius: var(--oc-radius-control); color: var(--oc-fg-muted); font-size: var(--oc-text-sm); }
.mark { width: 11px; height: 14px; color: var(--oc-accent); }
.brand.busy { color: var(--oc-fg-default); cursor: pointer; }
.brand.busy:hover { background-color: var(--oc-bg-hover); }
.right { display: flex; justify-content: flex-end; min-width: 0; }
.notice-host { position: relative; min-width: var(--oc-size-lg); min-height: var(--oc-size-sm); display: flex; align-items: center; justify-content: flex-end; }
.flash { display: inline-flex; align-items: center; gap: var(--oc-space-2xs); max-width: min(36vw, 420px); font-size: var(--oc-text-xs); color: var(--oc-fg-muted); opacity: 1; transition: opacity var(--oc-duration) var(--oc-ease); }
.flash.faded { opacity: 0; }
.notice-host:hover .flash, .titlebar:hover .flash { opacity: 1; }
.flash svg, .line svg { flex: none; }
.success svg { color: var(--oc-fg-success); }
.warning svg { color: var(--oc-fg-warning); }
.danger svg { color: var(--oc-fg-danger); }
.panel { position: absolute; top: calc(100% + 4px); z-index: var(--oc-z-overlay); background: var(--oc-bg-surface); border: 1px solid var(--oc-border-default); border-radius: var(--oc-radius-control); box-shadow: var(--oc-shadow-md); cursor: default; }
.tasks { left: 50%; transform: translateX(-50%); width: 320px; padding: var(--oc-space-sm); display: grid; gap: var(--oc-space-sm); }
.task { display: grid; gap: var(--oc-space-xs); min-width: 0; color: var(--oc-fg-default); }
.track { height: 3px; overflow: hidden; border-radius: var(--oc-radius-pill); background: var(--oc-bg-hover); }
.track i { display: block; height: 100%; background: var(--oc-accent); transition: width var(--oc-duration) linear; }
.history { right: 0; width: max-content; max-width: 420px; max-height: 360px; overflow-y: auto; padding: var(--oc-space-sm); display: grid; gap: var(--oc-space-sm); }
.line { display: flex; gap: var(--oc-space-2xs); overflow-wrap: anywhere; }
.titlebar-progress { position: absolute; left: 0; right: 0; bottom: -1px; height: 2px; }
.titlebar-progress i { display: block; height: 100%; background: var(--oc-accent); transition: width var(--oc-duration) linear; }
</style>
