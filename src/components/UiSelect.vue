<template>
  <div class="sel" :class="{ open }">
    <div v-if="filterable" ref="trigger" class="trigger typing" :class="{ disabled }">
      <input
        ref="input"
        class="field"
        role="combobox"
        aria-autocomplete="list"
        aria-haspopup="listbox"
        autocomplete="off"
        spellcheck="false"
        :aria-label="label"
        :aria-expanded="open"
        :aria-controls="open ? listId : undefined"
        :aria-activedescendant="activeOptionId"
        :disabled="disabled"
        :placeholder="text"
        :value="open ? query : (selected?.label ?? '')"
        @focus="onFocus"
        @click="open || onFocus()"
        @input="onInput"
        @keydown="onKey"
        @blur="onBlur"
      />
      <UiIcon class="chevron" :path="mdiChevronDown" @mousedown.prevent="open ? close(false) : input?.focus()" />
    </div>
    <button
      v-else
      ref="trigger"
      type="button"
      class="trigger"
      role="combobox"
      aria-haspopup="listbox"
      :aria-label="label"
      :aria-expanded="open"
      :aria-controls="open ? listId : undefined"
      :aria-activedescendant="activeOptionId"
      :disabled="disabled"
      @click="open ? close(false) : show()"
      @keydown="onKey"
    >
      <span class="value ellipsis" :class="{ empty: !selected }" :data-tooltip="text" data-tooltip-overflow>{{ text }}</span>
      <UiIcon class="chevron" :path="mdiChevronDown" />
    </button>
    <Teleport to="body">
      <div v-if="open" :id="listId" ref="list" class="menu" role="listbox" :aria-label="label" data-overlay :style="placement">
        <button
          v-for="(o, i) in shown"
          :id="`${listId}-${i}`"
          :key="o.value"
          type="button"
          class="option"
          :class="{ active: i === activeIndex, selected: o.value === modelValue }"
          role="option"
          tabindex="-1"
          :aria-selected="o.value === modelValue"
          :disabled="o.disabled"
          @pointerenter="o.disabled || (activeIndex = i)"
          @mousedown.prevent
          @click="pick(o)"
        >
          <span class="grow ellipsis" :data-tooltip="o.label" data-tooltip-overflow>{{ o.label }}</span>
          <UiIcon v-if="o.value === modelValue" :path="mdiCheck" />
        </button>
        <div v-if="!shown.length" class="none small muted">{{ query ? t('select.noMatch') : (emptyText ?? t('select.empty')) }}</div>
      </div>
    </Teleport>
  </div>
</template>

<script setup lang="ts">
import { mdiCheck, mdiChevronDown } from '@mdi/js'
import { computed, nextTick, onBeforeUnmount, ref, useId, watch, type CSSProperties } from 'vue'
import { t } from '../i18n'
import UiIcon from './UiIcon.vue'

export interface UiSelectOption {
  value: string
  label: string
  disabled?: boolean
  /** Extra words the typed filter also matches, such as a code the label leaves out. */
  keywords?: string
}

const props = withDefaults(
  defineProps<{ modelValue: string; options: readonly UiSelectOption[]; label: string; placeholder?: string; emptyText?: string; disabled?: boolean; filterable?: boolean }>(),
  { placeholder: undefined, emptyText: undefined },
)
const emit = defineEmits<{ 'update:modelValue': [string]; preview: [string | null] }>()

const MENU_MAX_HEIGHT = 240
const MENU_GAP = 4
const EDGE = 8
const OPTION_HEIGHT = 28

const listId = `select-${useId()}`
const trigger = ref<HTMLElement | null>(null)
const input = ref<HTMLInputElement | null>(null)
const query = ref('')
const list = ref<HTMLElement | null>(null)
const open = ref(false)
const activeIndex = ref(-1)
const placement = ref<CSSProperties>({})
let typed = ''
let typedTimer = 0

/** Codes print with uneven spacing, so matching ignores case and punctuation. */
const squash = (s: string) => s.toLocaleLowerCase().replace(/[^\p{L}\p{N}]/gu, '')
const shown = computed(() => {
  const q = squash(query.value)
  if (!props.filterable || !q) return props.options
  return props.options.filter((o) => squash(`${o.label} ${o.keywords ?? ''}`).includes(q))
})
const selected = computed(() => props.options.find((o) => o.value === props.modelValue))
const text = computed(() => selected.value?.label ?? props.placeholder ?? t('select.placeholder'))
const activeOptionId = computed(() => (open.value && activeIndex.value >= 0 ? `${listId}-${activeIndex.value}` : undefined))

function place() {
  const box = trigger.value!.getBoundingClientRect()
  const below = window.innerHeight - box.bottom - MENU_GAP - EDGE
  const above = box.top - MENU_GAP - EDGE
  const wanted = Math.min(MENU_MAX_HEIGHT, shown.value.length * OPTION_HEIGHT + 10)
  const flip = below < wanted && above > below
  placement.value = {
    left: `${box.left}px`,
    width: `${box.width}px`,
    maxHeight: `${Math.min(MENU_MAX_HEIGHT, flip ? above : below)}px`,
    ...(flip ? { bottom: `${window.innerHeight - box.top + MENU_GAP}px` } : { top: `${box.bottom + MENU_GAP}px` }),
  }
}

function onFocus() {
  query.value = ''
  show()
  input.value?.select()
}
function onInput(e: Event) {
  query.value = (e.target as HTMLInputElement).value
  if (!open.value) show()
  activeIndex.value = shown.value.findIndex((o) => !o.disabled)
  nextTick(place)
}
function onBlur() {
  close(false)
}

function show() {
  if (props.disabled) return
  const current = shown.value.findIndex((o) => o.value === props.modelValue && !o.disabled)
  activeIndex.value = current >= 0 ? current : shown.value.findIndex((o) => !o.disabled)
  place()
  open.value = true
}

function close(refocus: boolean) {
  open.value = false
  query.value = ''
  clearTyped()
  if (refocus) (input.value ?? (trigger.value as HTMLElement | null))?.focus()
}

function pick(o: UiSelectOption) {
  if (o.disabled) return
  emit('update:modelValue', o.value)
  close(true)
}

function move(step: 1 | -1) {
  const n = shown.value.length
  let i = activeIndex.value
  for (let k = 0; k < n; k++) {
    i = (i + step + n) % n
    if (!shown.value[i].disabled) {
      activeIndex.value = i
      return
    }
  }
}

function clearTyped() {
  typed = ''
  window.clearTimeout(typedTimer)
}

function onKey(e: KeyboardEvent) {
  // In the typing field, letters and the space bar belong to the text.
  if (props.filterable && (e.key === ' ' || (e.key.length === 1 && !e.ctrlKey && !e.metaKey))) return
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault()
    if (open.value) move(e.key === 'ArrowDown' ? 1 : -1)
    else show()
  } else if (e.key === 'Enter' || e.key === ' ') {
    e.preventDefault()
    const o = shown.value[activeIndex.value]
    if (!open.value) show()
    else if (o) pick(o)
  } else if (e.key === 'Home' || e.key === 'End') {
    if (!open.value) return
    e.preventDefault()
    activeIndex.value = e.key === 'Home' ? -1 : shown.value.length
    move(e.key === 'Home' ? 1 : -1)
  } else if (e.key === 'Escape' && open.value) {
    e.preventDefault()
    e.stopPropagation()
    close(true)
  } else if (e.key === 'Tab') {
    close(false)
  } else if (e.key.length === 1 && !e.ctrlKey && !e.metaKey && !e.altKey) {
    typed += e.key.toLocaleLowerCase()
    if (!open.value) show()
    const hit = shown.value.findIndex((o) => !o.disabled && o.label.toLocaleLowerCase().startsWith(typed))
    if (hit >= 0) activeIndex.value = hit
    window.clearTimeout(typedTimer)
    typedTimer = window.setTimeout(clearTyped, 500)
  }
}

function onOutside(e: Event) {
  const t = e.target as Node
  if (!trigger.value?.contains(t) && !list.value?.contains(t)) close(false)
}
function onResize() {
  close(false)
}

function listen(on: boolean) {
  const act = on ? 'addEventListener' : 'removeEventListener'
  document[act]('pointerdown', onOutside, true)
  window[act]('scroll', onOutside, true)
  window[act]('resize', onResize)
}

watch(open, listen)
watch([activeIndex, open], ([i, isOpen]) => emit('preview', isOpen ? (shown.value[i]?.value ?? null) : null))
watch(activeIndex, async (i) => {
  await nextTick()
  list.value?.children[i]?.scrollIntoView({ block: 'nearest' })
})
watch(() => props.disabled, (d) => d && close(false))
onBeforeUnmount(() => {
  listen(false)
  clearTyped()
})
</script>

<style scoped>
.sel { display: flex; min-width: 0; }
.trigger { width: 100%; min-width: 0; height: var(--oc-size-md); display: flex; align-items: center; gap: var(--oc-space-2xs); padding: 0 var(--oc-space-2xs) 0 var(--oc-space-sm); border: 0; border-radius: var(--oc-radius-control); background-color: var(--oc-bg-raised); text-align: left; }
.trigger:hover:not(:disabled) { background-image: linear-gradient(var(--oc-bg-hover), var(--oc-bg-hover)); }
.typing { cursor: text; }
.typing.disabled { opacity: var(--oc-opacity-disabled); }
.field { flex: 1; min-width: 0; height: 100%; padding: 0; border: 0; outline: 0; background: transparent; }
.field:focus-visible { box-shadow: none; }
.field::placeholder { color: var(--oc-fg-default); }
.typing:focus-within { box-shadow: var(--oc-focus-ring); }
.typing:focus-within .field::placeholder { color: var(--oc-fg-muted); }
.trigger:disabled { opacity: var(--oc-opacity-disabled); }
.open .trigger { box-shadow: var(--oc-focus-ring); }
.value { flex: 1; min-width: 0; }
.empty { color: var(--oc-fg-muted); }
.chevron { flex: none; color: var(--oc-fg-muted); transition: transform var(--oc-duration-quick) var(--oc-ease); }
.open .chevron { transform: rotate(180deg); }
.menu { position: fixed; z-index: var(--oc-z-overlay); padding: var(--oc-space-2xs); overflow-y: auto; background: var(--oc-bg-surface); border: 1px solid var(--oc-border-default); border-radius: var(--oc-radius-control); box-shadow: var(--oc-shadow-md); }
.option { width: 100%; min-height: var(--oc-size-md); display: flex; align-items: center; gap: var(--oc-space-xs); padding: 0 var(--oc-space-sm); border: 0; border-radius: 4px; background: transparent; text-align: left; }
.option:disabled { opacity: var(--oc-opacity-disabled); }
.option.selected { background-color: var(--oc-bg-selected); }
.option.active:not(:disabled) { background-image: linear-gradient(var(--oc-bg-hover), var(--oc-bg-hover)); }
.option svg { flex: none; color: var(--oc-fg-accent); }
.none { padding: var(--oc-space-sm); text-align: center; }
</style>
