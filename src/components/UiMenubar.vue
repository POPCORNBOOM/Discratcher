<template>
  <nav ref="root" class="menubar" :aria-label="t('menu.label')">
    <div v-for="m in menus" :key="m.key" class="entry">
      <button type="button" class="top" :class="{ on: openKey === m.key }" aria-haspopup="menu" :aria-expanded="openKey === m.key" @click="toggle(m.key)" @pointerenter="openKey && (openKey = m.key)" @keydown="onKey($event, m.key)">{{ m.label }}</button>
      <div v-if="openKey === m.key" class="menu" role="menu" :aria-label="m.label" data-tauri-drag-region="false" @keydown="onKey($event, m.key)">
        <template v-for="a in m.actions" :key="a.key">
          <div v-if="a.divider" class="divider" />
          <div v-else-if="a.children" class="sub" @pointerleave="sub = ''">
            <button type="button" class="item" role="menuitem" aria-haspopup="menu" :aria-expanded="sub === a.key" @click="sub = sub === a.key ? '' : a.key" @pointerenter="sub = a.key">
              <UiIcon v-if="a.icon" :path="a.icon" />
              <span v-else class="spacer" />
              <span class="grow ellipsis">{{ a.title }}</span>
              <UiIcon class="more" :path="mdiChevronRight" />
            </button>
            <div v-if="sub === a.key" class="menu submenu" role="menu" :aria-label="a.title">
              <button v-for="c in a.children" :key="c.key" type="button" class="item" role="menuitem" @click="select(c)">
                <UiIcon v-if="c.icon" :path="c.icon" />
                <span v-else class="spacer" />
                <span class="grow ellipsis">{{ c.title }}</span>
              </button>
            </div>
          </div>
          <button v-else type="button" class="item" role="menuitem" :disabled="a.disabled" @click="select(a)">
            <UiIcon v-if="a.icon" :path="a.icon" />
            <span v-else class="spacer" />
            <span class="grow ellipsis">{{ a.title }}</span>
            <span v-if="a.shortcut" class="shortcut small">{{ a.shortcut }}</span>
          </button>
        </template>
      </div>
    </div>
  </nav>
</template>

<script setup lang="ts">
// Behaviour follows OpenCard's useMenubar: click toggles, and once one menu is open,
// hovering a neighbour switches to it.
import { mdiChevronRight } from '@mdi/js'
import { nextTick, onBeforeUnmount, onMounted, ref } from 'vue'
import { t } from '../i18n'
import UiIcon from './UiIcon.vue'

export interface MenuAction {
  key: string
  title?: string
  icon?: string
  shortcut?: string
  disabled?: boolean
  divider?: boolean
  run?: () => void
  /** A submenu: the item only opens it. */
  children?: MenuAction[]
}
export interface Menu {
  key: string
  label: string
  actions: MenuAction[]
}

const props = defineProps<{ menus: Menu[] }>()
const root = ref<HTMLElement | null>(null)
const openKey = ref<string | null>(null)
const sub = ref('')

function toggle(key: string) {
  openKey.value = openKey.value === key ? null : key
}

function select(a: MenuAction) {
  openKey.value = null
  sub.value = ''
  a.run?.()
}

function items() {
  return [...(root.value?.querySelectorAll<HTMLElement>('.item:not(:disabled)') ?? [])]
}

async function onKey(e: KeyboardEvent, key: string) {
  const menus = props.menus
  const at = menus.findIndex((m) => m.key === key)
  if (e.key === 'Escape' && openKey.value) {
    e.stopPropagation()
    const top = root.value?.querySelectorAll<HTMLElement>('.top')[at]
    openKey.value = null
    top?.focus()
  } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault()
    openKey.value = key
    await nextTick()
    const list = items()
    const here = list.indexOf(document.activeElement as HTMLElement)
    const step = e.key === 'ArrowDown' ? 1 : -1
    list[here < 0 ? (step > 0 ? 0 : list.length - 1) : (here + step + list.length) % list.length]?.focus()
  } else if ((e.key === 'ArrowRight' || e.key === 'ArrowLeft') && openKey.value) {
    e.preventDefault()
    const next = (at + (e.key === 'ArrowRight' ? 1 : -1) + menus.length) % menus.length
    openKey.value = menus[next].key
    await nextTick()
    items()[0]?.focus()
  }
}

function onOutside(e: PointerEvent) {
  if (!root.value?.contains(e.target as Node)) openKey.value = null
}
onMounted(() => document.addEventListener('pointerdown', onOutside, true))
onBeforeUnmount(() => document.removeEventListener('pointerdown', onOutside, true))
</script>

<style scoped>
.menubar { display: flex; align-items: center; }
.entry { position: relative; }
.top { height: var(--oc-size-md); padding: 0 var(--oc-space-sm); border: 0; border-radius: var(--oc-radius-control); background: transparent; color: var(--oc-fg-muted); }
.top:hover, .top.on { background-color: var(--oc-bg-hover); color: var(--oc-fg-default); }
.menu { position: absolute; left: 0; top: calc(100% + 4px); z-index: var(--oc-z-overlay); min-width: 200px; padding: 3px; background: var(--oc-bg-surface); border: 1px solid var(--oc-border-default); border-radius: var(--oc-radius-control); box-shadow: var(--oc-shadow-md); }
.item { width: 100%; height: var(--oc-size-md); display: flex; align-items: center; gap: var(--oc-space-xs); padding: 0 var(--oc-space-sm); border: 0; border-radius: 4px; background: transparent; text-align: left; }
.item:hover:not(:disabled), .item:focus-visible { background-color: var(--oc-bg-hover); box-shadow: none; }
.item:disabled { opacity: var(--oc-opacity-disabled); }
.item svg { flex: none; color: var(--oc-fg-muted); }
.spacer { width: 16px; flex: none; }
.shortcut { color: var(--oc-fg-subtle); }
.sub { position: relative; }
.submenu { left: calc(100% + 2px); top: -4px; min-width: 140px; }
.more { margin-left: auto; }
.divider { height: 1px; margin: 3px var(--oc-space-xs); background: var(--oc-border-muted); }
</style>
