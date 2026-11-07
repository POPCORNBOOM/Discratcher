<template>
  <section class="home">
    <div class="hero">
      <p class="welcome">{{ t('home.welcome') }}</p>
      <h1 aria-label="Discratcher">
        <span class="di" aria-hidden="true"><BrandMark class="mark" />i</span><span class="sc" aria-hidden="true">sc</span><span class="rest" aria-hidden="true">ratcher</span>
      </h1>
      <p class="lede">{{ t('home.lede') }}</p>
      <div class="actions">
        <UiButton variant="solid" size="lg" :icon="mdiMagnify" @click="s.view = 'models'">{{ t('home.search') }}</UiButton>
        <UiButton size="lg" :icon="mdiImageEditOutline" @click="s.view = 'carve'">{{ t('home.create') }}</UiButton>
      </div>
    </div>
    <dl class="specs mono">
      <div><dt>{{ t('home.pitch') }}</dt><dd>0.74 µm</dd></div>
      <div><dt>{{ t('home.sectors') }}</dt><dd>2,295,104</dd></div>
      <div><dt>{{ t('home.accuracy') }}</dt><dd>0.34°</dd></div>
      <div v-if="s.preset?.stamperCode"><dt>{{ t('home.code') }}</dt><dd class="selectable">{{ s.preset.stamperCode }}</dd></div>
    </dl>
  </section>
</template>

<script setup lang="ts">
import { mdiImageEditOutline, mdiMagnify } from '@mdi/js'
import { t } from '../i18n'
import BrandMark from '../components/BrandMark.vue'
import UiButton from '../components/UiButton.vue'
import { useStudio } from '../stores/studio'

const s = useStudio()
</script>

<style scoped>
.home { --big: clamp(58px, 7.4vw, 104px); --small: calc(var(--big) * 0.34); position: relative; flex: 1; min-width: 0; overflow: hidden; display: flex; align-items: center; }
.hero { position: relative; display: flex; flex-direction: column; align-items: flex-start; padding-left: var(--oc-page-gutter); pointer-events: none; }
.hero > * { pointer-events: auto; }
.welcome { color: var(--oc-fg-muted); font-size: var(--oc-text-lg); letter-spacing: 0.14em; }
h1 { display: flex; align-items: baseline; height: calc(var(--big) * 1.24); margin: var(--oc-space-xs) 0 var(--oc-space-md) -0.04em; font-family: 'SF Pro Display', var(--oc-font-sans); font-size: var(--big); font-weight: 600; line-height: 1.24; letter-spacing: -0.035em; color: #f5f5f7; text-shadow: 0 2px 30px rgba(0, 0, 0, 0.75); }
.mark { width: 0.55em; height: 0.71em; margin-right: 0.02em; }
.sc { background: linear-gradient(100deg, #c9a6ff, #8fb7ff 55%, #8ff0dc); -webkit-background-clip: text; background-clip: text; color: transparent; }
.di, .rest { animation: 8s cubic-bezier(0.65, 0, 0.35, 1) infinite; }
.di { animation-name: grow-first; }
.rest { animation-name: grow-last; }
/* "Disc" and "scratcher" share their "sc", so one word gives way to the other around it. */
@keyframes grow-first {
  0%, 42%, 100% { font-size: var(--big); opacity: 1; }
  50%, 92% { font-size: var(--small); opacity: 0.38; }
}
@keyframes grow-last {
  0%, 42%, 100% { font-size: var(--small); opacity: 0.38; }
  50%, 92% { font-size: var(--big); opacity: 1; }
}
.lede { max-width: 26em; color: var(--oc-fg-default); font-size: var(--oc-text-lg); line-height: 1.6; text-shadow: 0 1px 14px rgba(0, 0, 0, 0.8); }
.actions { display: flex; gap: var(--oc-space-sm); margin-top: 28px; }
.specs { position: absolute; left: var(--oc-page-gutter); bottom: 28px; display: flex; margin: 0; }
.specs div { display: flex; flex-direction: column; gap: 2px; padding: 0 var(--oc-space-lg); border-left: 1px solid var(--oc-border-muted); }
.specs div:first-child { padding-left: 0; border-left: 0; }
.specs dt { color: var(--oc-fg-subtle); font-size: var(--oc-text-xs); letter-spacing: 0.1em; }
.specs dd { margin: 0; color: var(--oc-fg-muted); }
@media (prefers-reduced-motion: reduce) {
  .di, .rest { animation: none; }
}
</style>
