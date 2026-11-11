<template>
  <header class="titlebar" :class="{ mac }" data-tauri-drag-region="deep">
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

      <UiButton v-if="update" variant="soft" :disabled="p.status === 'running'" @click="updateDialog = true">{{ t('update.available', { version: update.version }) }}</UiButton>

      <div class="preset" data-tauri-drag-region="false">
        <UiSelect class="picker" filterable :label="t('presets.label')" :placeholder="t('presets.none')" :model-value="s.preset?.id ?? ''" :options="presetOptions" :disabled="!!s.job" @update:model-value="s.presetId = $event" @preview="preview" />
        <UiButton v-if="s.preset" icon-only variant="ghost" :icon="mdiTrashCanOutline" :aria-label="t('presets.delete', { name: s.preset.name })" :disabled="!!s.job" @click="confirmDelete = true" />
      </div>

      <div v-if="!mac" class="controls" data-tauri-drag-region="false">
        <button class="wc" type="button" :aria-label="t('titlebar.minimize')" :data-tooltip="t('titlebar.minimize')" @click="windowControl('minimize')">
          <UiIcon :path="mdiWindowMinimize" :size="14" />
        </button>
        <button class="wc" type="button" :aria-label="maxed ? t('titlebar.restore') : t('titlebar.maximize')" :data-tooltip="maxed ? t('titlebar.restore') : t('titlebar.maximize')" @click="toggleMaximized">
          <UiIcon :path="maxed ? mdiWindowRestore : mdiWindowMaximize" :size="14" />
        </button>
        <button class="wc close" type="button" :aria-label="t('titlebar.close')" :data-tooltip="t('titlebar.close')" @click="windowControl('close')">
          <UiIcon :path="mdiClose" :size="14" />
        </button>
      </div>
    </div>

    <UiDialog v-if="confirmDelete && s.preset" :title="t('presets.deleteTitle')" data-tauri-drag-region="false" @close="confirmDelete = false">
      <p>{{ t('presets.deleteBody', { name: s.preset.name }) }}</p>
      <template #actions>
        <UiButton variant="ghost" @click="confirmDelete = false">{{ t('common.cancel') }}</UiButton>
        <UiButton variant="danger" @click="removePreset">{{ t('common.delete') }}</UiButton>
      </template>
    </UiDialog>

    <UiDialog v-if="updateDialog && update" :title="t('update.available', { version: update.version })" data-tauri-drag-region="false" @close="updateDialog = false">
      <p class="notes selectable">{{ update.body || t('update.noNotes') }}</p>
      <template #actions>
        <UiButton variant="ghost" @click="updateDialog = false">{{ t('update.later') }}</UiButton>
        <UiButton variant="solid" @click="confirmUpdate">{{ t('update.now') }}</UiButton>
      </template>
    </UiDialog>

    <div v-if="overallProgress != null" class="titlebar-progress" role="progressbar" :aria-label="t('titlebar.progress')" aria-valuemin="0" aria-valuemax="100" :aria-valuenow="Math.round(overallProgress * 100)"><i :style="{ width: `${overallProgress * 100}%` }" /></div>
  </header>
</template>

<script setup lang="ts">
import { mdiUpdate, mdiArrowLeft, mdiCheck, mdiClose, mdiContentSaveOutline, mdiImagePlusOutline, mdiTranslate, mdiCrosshairsGps, mdiExportVariant, mdiFire, mdiFolderOpenOutline, mdiGithub, mdiHomeOutline, mdiImageEditOutline, mdiMagnify, mdiMessageAlertOutline, mdiTrashCanOutline, mdiWindowMaximize, mdiWindowMinimize, mdiWindowRestore } from '@mdi/js'
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { call, isWindowMaximized, openInBrowser, windowControl } from '../api'
import { REPOSITORY_URL } from '../catalog'
import { locale, locales, setLocale, t } from '../i18n'
import { notices, notifyError, notifySuccess } from '../notices'
import { useProcess } from '../stores/process'
import { useComposition } from '../stores/composition'
import { useStudio, type CarveCommands } from '../stores/studio'
import { overallProgress, tasks } from '../tasks'
import { appVersion, checkForUpdate, checking, installUpdate, update } from '../updater'
import BrandMark from './BrandMark.vue'
import UiButton from './UiButton.vue'
import UiDialog from './UiDialog.vue'
import UiIcon from './UiIcon.vue'
import UiMenubar, { type Menu } from './UiMenubar.vue'
import UiSelect from './UiSelect.vue'

const FADE_MS = 2200
const ERROR_FADE_MS = 6000
const PANEL_CLOSE_DELAY_MS = 120

/** macOS draws its own traffic lights over the window; elsewhere the page draws the buttons. */
const mac = navigator.userAgent.includes('Mac')

const s = useStudio()
const comp = useComposition()
const p = useProcess()
const updateDialog = ref(false)
const confirmDelete = ref(false)
const taskPanel = ref(false)
const noticePanel = ref(false)
const faded = ref(false)
let fadeTimer = 0
let closeTimer = 0

/** The disc the next burn will use, chosen from every calibration on this machine. */
const presetOptions = computed(() => s.presets.map((p) => ({ value: p.id, label: p.name, keywords: `${p.stamperCode} ${p.mediaId} ${p.brand ?? ''}` })))
const preview = (id: string | null) => (s.discPreview = s.presets.find((p) => p.id === id) ?? null)

async function removePreset() {
  const id = s.preset?.id
  confirmDelete.value = false
  if (!id) return
  try {
    await call('delete_preset', { id })
    if (s.presetId === id) s.presetId = ''
    await s.refreshPresets()
    notifySuccess(t('presets.deleted'))
  } catch (e) {
    notifyError(String(e))
  }
}

/** The window is frameless, so its buttons are ours and the icon has to follow it. */
const maxed = ref(false)
async function syncMaximized() {
  maxed.value = await isWindowMaximized()
}
async function toggleMaximized() {
  await windowControl('toggleMaximize')
  await syncMaximized()
}

const latest = computed(() => notices.value[notices.value.length - 1] ?? null)
watch(latest, (n) => {
  clearTimeout(fadeTimer)
  faded.value = false
  if (n) fadeTimer = window.setTimeout(() => (faded.value = true), n.tone === 'danger' ? ERROR_FADE_MS : FADE_MS)
})

function confirmUpdate() {
  updateDialog.value = false
  installUpdate()
}

function openTasks() {
  clearTimeout(closeTimer)
  taskPanel.value = tasks.value.length > 0
}
function closeTasks() {
  clearTimeout(closeTimer)
  closeTimer = window.setTimeout(() => (taskPanel.value = false), PANEL_CLOSE_DELAY_MS)
}

/** Menu actions that belong to the carve page open it first. */
async function onCarve(run: (c: CarveCommands) => void) {
  s.view = 'carve'
  await nextTick()
  if (s.carve) run(s.carve)
}
const modKey = mac ? '⌘' : 'Ctrl+'

const menus = computed<Menu[]>(() => [
  {
    key: 'file',
    label: t('menu.file'),
    actions: [
      { key: 'home', title: t('menu.home'), icon: mdiHomeOutline, run: () => (s.view = 'home') },
      { key: 'd1', divider: true },
      { key: 'new', title: t('menu.new'), icon: mdiImageEditOutline, run: () => onCarve((c) => c.newComposition()) },
      { key: 'open', title: t('menu.open'), icon: mdiFolderOpenOutline, run: () => onCarve((c) => c.openComposition()) },
      { key: 'save', title: t('menu.save'), icon: mdiContentSaveOutline, shortcut: `${modKey}S`, disabled: !comp.layers.length, run: () => onCarve((c) => c.save()) },
      { key: 'saveAs', title: t('menu.saveAs'), disabled: !comp.layers.length, run: () => onCarve((c) => c.saveAs()) },
      { key: 'd2', divider: true },
      { key: 'add', title: t('menu.addImage'), icon: mdiImagePlusOutline, disabled: comp.full, run: () => onCarve((c) => c.choose()) },
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
      { key: 'update', title: t(checking.value ? 'update.checking' : 'update.check'), icon: mdiUpdate, disabled: checking.value, run: () => void checkForUpdate(true) },
      { key: 'd1', divider: true },
      {
        key: 'language',
        title: t('menu.language'),
        icon: mdiTranslate,
        children: locales.map((l) => ({ key: l.code, title: l.name, icon: l.code === locale.value ? mdiCheck : undefined, run: () => setLocale(l.code) })),
      },
      ...(appVersion.value ? [{ key: 'version', title: t('update.version', { version: appVersion.value }), disabled: true }] : []),
    ],
  },
])

function onShortcut(e: KeyboardEvent) {
  if (e.metaKey && e.key === '[') {
    e.preventDefault()
    s.back()
  }
}
onMounted(() => {
  window.addEventListener('keydown', onShortcut)
  // Maximising can also come from a double click on the drag region, so follow the window.
  window.addEventListener('resize', syncMaximized)
  void syncMaximized()
})
onBeforeUnmount(() => {
  window.removeEventListener('keydown', onShortcut)
  window.removeEventListener('resize', syncMaximized)
})
</script>

<style scoped>
.titlebar { position: relative; height: var(--oc-shell-titlebar-height); flex: none; display: grid; grid-template-columns: 1fr auto 1fr; align-items: center; gap: var(--oc-space-sm); padding: 0 0 0 var(--oc-space-md); background: var(--oc-bg-block); border-bottom: 1px solid var(--oc-border-muted); z-index: 10; }
.titlebar.mac { padding: 0 var(--oc-space-md) 0 84px; }
.notes { max-height: 320px; overflow-y: auto; white-space: pre-wrap; }
.left { display: flex; align-items: center; gap: var(--oc-space-2xs); }
.middle { display: flex; justify-content: center; }
.brand { position: relative; display: inline-flex; align-items: center; gap: var(--oc-space-xs); height: var(--oc-size-md); padding: 0 var(--oc-space-sm); border-radius: var(--oc-radius-control); color: var(--oc-fg-muted); font-size: var(--oc-text-sm); }
.mark { width: 11px; height: 14px; color: var(--oc-accent); }
.brand.busy { color: var(--oc-fg-default); cursor: pointer; }
.brand.busy:hover { background-color: var(--oc-bg-hover); }
.right { display: flex; justify-content: flex-end; align-items: stretch; align-self: stretch; min-width: 0; gap: var(--oc-space-xs); }
.preset { display: flex; align-items: center; align-self: center; gap: var(--oc-space-2xs); width: clamp(160px, 22vw, 260px); }
.picker { flex: 1; min-width: 0; }
.controls { display: flex; align-items: stretch; }
.wc { width: 36px; display: grid; place-items: center; border: 0; padding: 0; background: transparent; color: var(--oc-fg-muted); transition: background-color var(--oc-duration) var(--oc-ease), color var(--oc-duration) var(--oc-ease); }
.wc:hover { background: var(--oc-bg-hover); color: var(--oc-fg-default); }
.wc.close:hover { background: #c42b1c; color: #fff; }
.notice-host { position: relative; align-self: center; min-width: var(--oc-size-lg); min-height: var(--oc-size-sm); display: flex; align-items: center; justify-content: flex-end; }
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
