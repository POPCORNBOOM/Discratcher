<template>
  <div class="sl" :class="{ trades: ends && !disabled, off: disabled }">
    <div class="head">
      <span>{{ label }}</span>
      <span v-if="!ends || format" class="mono muted">{{ format ? format(modelValue) : modelValue }}</span>
    </div>
    <div ref="row" class="track" :style="{ '--at': `calc(${THUMB_HALF}px + ${fraction} * (100% - ${2 * THUMB_HALF}px))` }">
      <div class="rail" />
      <template v-if="ends">
        <span ref="lo" class="end lo" :class="{ squeezed: place.loHidden }" :style="{ '--to': `${place.lo}px` }">{{ ends[0] }}</span>
        <span ref="hi" class="end hi" :class="{ squeezed: place.hiHidden }" :style="{ '--to': `${place.hi}px` }">{{ ends[1] }}</span>
      </template>
      <input type="range" :aria-label="label" :min="min" :max="max" :step="step" :value="modelValue" :disabled="disabled" @input="$emit('update:modelValue', Number(($event.target as HTMLInputElement).value))" />
    </div>
  </div>
</template>

<script setup lang="ts">
/**
 * `ends` names what each end gives, for a slider that trades one thing for another. Without a
 * `format` such a slider shows no number. The ends take no room: while the slider is hovered,
 * focused from the keyboard or dragged, its track swells into a bar and they come out of the
 * handle inside it, one either side, with a highlight flowing away from it.
 */
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue'

const props = defineProps<{ label: string; modelValue: number; min: number; max: number; step?: number; disabled?: boolean; format?: (v: number) => string; ends?: [string, string] }>()
defineEmits<{ 'update:modelValue': [number] }>()

/** Half the thumb's width, px (the thumb's centre runs from this to the width less this). */
const THUMB_HALF = 6
/** Space between the handle's centre and each end's words, px. */
const GAP = 12
/** Space kept clear at each rounded end of the bar, px. */
const INSET = 6

const fraction = computed(() => (props.max > props.min ? (props.modelValue - props.min) / (props.max - props.min) : 0))

const row = ref<HTMLElement | null>(null)
const lo = ref<HTMLElement | null>(null)
const hi = ref<HTMLElement | null>(null)
const size = reactive({ row: 0, lo: 0, hi: 0 })
let observer: ResizeObserver | null = null

function measure() {
  size.row = row.value?.clientWidth ?? 0
  size.lo = lo.value?.offsetWidth ?? 0
  size.hi = hi.value?.offsetWidth ?? 0
}
onMounted(() => {
  observer = new ResizeObserver(measure)
  for (const el of [row.value, lo.value, hi.value]) if (el) observer.observe(el)
  measure()
})
watch(() => props.ends, () => requestAnimationFrame(measure))
onBeforeUnmount(() => observer?.disconnect())

/** How far each end moves to flank the handle, px, and whether the bar has room for it there. */
const place = computed(() => {
  const x = THUMB_HALF + fraction.value * (size.row - 2 * THUMB_HALF)
  const loAt = x - GAP - size.lo
  const hiAt = x + GAP
  return { lo: loAt, hi: hiAt - (size.row - size.hi), loHidden: loAt < INSET, hiHidden: hiAt + size.hi > size.row - INSET }
})
</script>

<style scoped>
.sl { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
.head { display: flex; justify-content: space-between; }

/* The native track is left clear; the rail under it is drawn here so it can swell. */
.track { position: relative; height: 16px; }
.sl.off .track { opacity: var(--oc-opacity-disabled); }
.rail {
  position: absolute; left: 0; right: 0; top: 50%; height: 4px; transform: translateY(-50%);
  border-radius: var(--oc-radius-pill);
  background: linear-gradient(to right, var(--oc-accent) var(--at), var(--oc-bg-input) var(--at));
  transition: height 240ms var(--oc-ease);
}
input { position: relative; display: block; appearance: none; width: 100%; height: 16px; margin: 0; background: transparent; }
input::-webkit-slider-runnable-track { height: 16px; background: transparent; }
input::-webkit-slider-thumb { appearance: none; width: 12px; height: 12px; margin-top: 2px; border-radius: 50%; background: #fff; box-shadow: var(--oc-shadow-sm); transition: transform 240ms var(--oc-ease); }
/* The handle grows a little when the slider is in use, still inside the swollen bar. */
.sl:not(.off):is(:hover, :has(input:focus-visible), :has(input:active)) input::-webkit-slider-thumb { transform: scale(1.25); }

/* In use (hovered, focused from the keyboard or dragged) `--in` is 1, and 0 otherwise. */
.sl.trades:is(:hover, :has(input:focus-visible), :has(input:active)) { --in: 1; --flow: running; }
.sl.trades:is(:hover, :has(input:focus-visible), :has(input:active)) .rail { height: 16px; }
.end {
  position: absolute; top: 0; line-height: 16px; font-size: var(--oc-text-xs); font-weight: 500; white-space: nowrap; pointer-events: none;
  /* At rest each end is tucked against the handle; in use it moves out to its side. */
  opacity: var(--in, 0);
  transform: translateX(calc(var(--to) + var(--tuck) * (1 - var(--in, 0))));
  color: transparent;
  background: linear-gradient(90deg, var(--base) 0 38%, var(--glint) 50%, var(--base) 62% 100%) 0 0 / 300% 100%;
  -webkit-background-clip: text;
  background-clip: text;
  animation: 1.8s linear infinite var(--flow, paused);
  transition: transform 280ms var(--oc-ease), opacity 160ms var(--oc-ease);
}
/* The left end sits on the filled part, the right on the empty part. */
.lo { left: 0; --tuck: 8px; --base: rgba(245, 242, 255, 0.78); --glint: #fff; animation-name: flow-left; }
.hi { right: 0; --tuck: -8px; --base: var(--oc-fg-default); --glint: var(--oc-fg-accent); animation-name: flow-right; }
.end.squeezed { opacity: 0; }
/* While dragging they keep up with the handle instead of easing after it. */
.sl:has(input:active) .end { transition-property: opacity; }
@keyframes flow-left { from { background-position: 0% 0; } to { background-position: 100% 0; } }
@keyframes flow-right { from { background-position: 100% 0; } to { background-position: 0% 0; } }
@media (prefers-reduced-motion: reduce) {
  .rail, input::-webkit-slider-thumb { transition: none; }
  .end { animation: none; transition: opacity 160ms; }
}
</style>
