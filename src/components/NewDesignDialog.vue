<template>
  <UiDialog :title="t('wizard.title')" @close="$emit('close')">
    <div class="steps small" aria-hidden="true">
      <span :class="{ on: step === 1 }">{{ t('wizard.stepLamp') }}</span>
      <UiIcon :path="mdiChevronRight" :size="16" class="muted" />
      <span :class="{ on: step === 2 }">{{ t('wizard.stepStart') }}</span>
    </div>

    <template v-if="step === 1">
      <div class="cards three" role="radiogroup" :aria-label="t('wizard.stepLamp')">
        <button
          v-for="l in LAMPS"
          :key="l.id"
          type="button"
          role="radio"
          class="card"
          :aria-checked="lamp === l.id"
          :class="{ on: lamp === l.id }"
          @click="lamp = l.id"
          @dblclick="next"
        >
          <span class="figure">
            <img v-if="pictures[l.id]" class="disc" :src="pictures[l.id]" alt="" />
            <svg class="marks" viewBox="0 0 100 100" aria-hidden="true">
              <line v-bind="ray(l.azimuthDeg)" class="ray" />
              <g :transform="lampAt(l.azimuthDeg)"><path :d="mdiLightbulbOn" class="bulb" /></g>
            </svg>
          </span>
          <span class="name">{{ t(l.name) }}</span>
          <span class="small" :class="l.id === 'farRight' ? 'muted' : 'tag'">{{ t(l.fits) }}</span>
        </button>
      </div>
      <p class="small muted">{{ t('wizard.lampHelp') }}</p>
    </template>

    <template v-else>
      <div class="cards two" role="radiogroup" :aria-label="t('wizard.stepStart')">
        <button
          v-for="tpl in DESIGN_TEMPLATES"
          :key="tpl.id"
          type="button"
          role="radio"
          class="card left"
          :aria-checked="template === tpl.id"
          :class="{ on: template === tpl.id }"
          @click="template = tpl.id"
          @dblclick="create"
        >
          <UiIcon :path="tpl.icon" :size="24" />
          <span class="name">{{ t(tpl.name) }}</span>
          <span class="small muted">{{ t(tpl.description) }}</span>
        </button>
      </div>
      <div v-if="template === 'spin' && lamp === 'held'" class="notice warn">
        <UiIcon :path="mdiAlertOutline" />
        <div class="grow">
          {{ t('wizard.spinSideLight') }}
          <a role="button" tabindex="0" @click="lamp = 'far'" @keydown.enter="lamp = 'far'">{{ t('wizard.useTopLight') }}</a>
        </div>
      </div>
      <p class="small muted">{{ t('wizard.lampLater') }}</p>
    </template>

    <template #actions>
      <UiButton v-if="step === 1" variant="ghost" @click="$emit('close')">{{ t('common.cancel') }}</UiButton>
      <UiButton v-else variant="ghost" @click="step = 1">{{ t('common.back') }}</UiButton>
      <UiButton v-if="step === 1" variant="solid" @click="next">{{ t('wizard.next') }}</UiButton>
      <UiButton v-else variant="solid" @click="create">{{ t('wizard.create') }}</UiButton>
    </template>
  </UiDialog>
</template>

<script setup lang="ts">
import { mdiAlertOutline, mdiChevronRight, mdiLightbulbOn } from '@mdi/js'
import { onMounted, ref } from 'vue'
import { LAMP_PRESETS, lampMmToCylindrical, type LampPresetId } from '../holo/constants'
import { lampPictures } from '../holo/lampPictures'
import { DESIGN_TEMPLATES, type DesignTemplate } from '../holo/templates'
import { t } from '../i18n'
import UiButton from './UiButton.vue'
import UiDialog from './UiDialog.vue'
import UiIcon from './UiIcon.vue'

const emit = defineEmits<{ close: []; create: [lamp: LampPresetId, template: DesignTemplate] }>()

/** Side light suits models, light from above suits colour pictures; the corner light is between (see `usability.ts`). */
const LAMPS: { id: LampPresetId; name: string; fits: string; azimuthDeg: number }[] = (
  [
    ['held', 'holo.lampHeld', 'wizard.fitsModels'],
    ['far', 'holo.lampFar', 'wizard.fitsPictures'],
    ['farRight', 'holo.lampFarRight', 'wizard.fitsBoth'],
  ] as const
).map(([id, name, fits]) => ({ id, name, fits, azimuthDeg: lampMmToCylindrical(LAMP_PRESETS[id]).azimuthDeg }))

const step = ref<1 | 2>(1)
const lamp = ref<LampPresetId>('held')
const template = ref(DESIGN_TEMPLATES[0].id)
const pictures = ref<Partial<Record<LampPresetId, string>>>({})

onMounted(() => {
  try {
    pictures.value = lampPictures(LAMP_PRESETS, Math.round(112 * 2 * (window.devicePixelRatio || 1)))
  } catch {
    // Without WebGL the cards still show where each lamp stands.
  }
})

/** The disc picture spans 14 to 86 of the 100-unit figure; the lamp stands just outside it. */
const polar = (deg: number, r: number) => [50 + r * Math.sin((deg * Math.PI) / 180), 50 - r * Math.cos((deg * Math.PI) / 180)]
function ray(deg: number) {
  const [x1, y1] = polar(deg, 40)
  const [x2, y2] = polar(deg, 30)
  return { x1, y1, x2, y2 }
}
function lampAt(deg: number) {
  const [x, y] = polar(deg, 44)
  return `translate(${x - 6} ${y - 6}) scale(0.5)`
}

function next() {
  step.value = 2
}
function create() {
  const tpl = DESIGN_TEMPLATES.find((x) => x.id === template.value) ?? DESIGN_TEMPLATES[0]
  emit('create', lamp.value, tpl)
}
</script>

<style scoped>
.steps { display: flex; align-items: center; gap: var(--oc-space-2xs); color: var(--oc-fg-muted); }
.steps .on { color: var(--oc-fg-default); font-weight: 600; }
.cards { display: grid; gap: var(--oc-space-sm); }
.cards.three { grid-template-columns: repeat(3, 1fr); }
.cards.two { grid-template-columns: repeat(3, 1fr); }
.card { display: flex; flex-direction: column; align-items: center; gap: var(--oc-space-2xs); padding: var(--oc-space-sm); border: 1px solid var(--oc-border-muted); border-radius: var(--oc-radius-control); background: var(--oc-bg-base); color: var(--oc-fg-default); text-align: center; }
.card.left { align-items: flex-start; text-align: left; padding: var(--oc-space-md); }
.card:hover { border-color: var(--oc-border-strong); }
.card.on { border-color: var(--oc-accent); background: var(--oc-bg-accent-subtle); }
.name { font-weight: 600; }
.tag { color: var(--oc-fg-accent); }
.figure { position: relative; width: 112px; height: 112px; }
.disc { position: absolute; left: 14%; top: 14%; width: 72%; height: 72%; }
.marks { position: absolute; inset: 0; width: 100%; height: 100%; overflow: visible; }
.ray { stroke: var(--oc-accent); stroke-width: 1.2; stroke-dasharray: 2 2; }
.bulb { fill: var(--oc-accent); }
</style>
