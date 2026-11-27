<template>
  <div
    class="page holo"
    @dragover.prevent="onDragOver"
    @dragleave="onDragLeave"
    @drop.prevent="onDrop"
  >
    <aside class="side">
      <div class="flow fields">
        <div class="design-title">
          <h1>{{ t('holo.title') }}</h1>
          <div class="small muted ellipsis" :data-tooltip="designDir ?? undefined">{{ designName }}<template v-if="dirty"> · {{ t('carve.unsaved') }}</template></div>
        </div>

        <div v-if="missingFiles.length" class="notice warn">
          <UiIcon :path="mdiAlertOutline" />
          <div class="grow">{{ t('holo.missingFiles', { files: missingFiles.join(', ') }) }}</div>
          <UiButton icon-only variant="ghost" :icon="mdiClose" :aria-label="t('common.cancel')" @click="missingFiles = []" />
        </div>

        <div v-if="loadError" class="notice danger">
          <UiIcon :path="mdiAlertOctagonOutline" />
          <div class="grow">{{ loadError }}</div>
          <UiButton icon-only variant="ghost" :icon="mdiClose" :aria-label="t('common.cancel')" @click="loadError = ''" />
        </div>

        <div v-if="expViews.length" class="notice info">
          <UiIcon :path="mdiFlaskOutline" />
          <div class="grow">{{ t('holo.expActive', { n: expViews.length }) }}</div>
          <UiButton icon-only variant="ghost" :icon="mdiClose" :aria-label="t('holo.clearExp')" @click="clearExpViews" />
        </div>

        <!-- Empty state: one drop area + "试试示例" -->
        <section v-if="!scene.objects.length && !scene.images.length && scene.positions.length < 2 && !expViews.length" class="section empty-section">
          <button
            class="drop"
            :class="{ over: dropHover }"
            type="button"
            :disabled="loadingMesh || !!s.job"
            @click="addModelFromPicker"
          >
            <span class="drop-inner">
              <UiIcon :path="mdiCubeOutline" :size="24" />
              <span class="title">{{ t('holo.addModel') }}</span>
              <span class="small muted">{{ t('holo.dropHint') }}</span>
            </span>
          </button>
          <UiButton
            variant="soft"
            :icon="mdiStarFourPointsOutline"
            :disabled="loadingMesh || !!s.job"
            @click="addSample"
          >
            {{ t('holo.trySample') }}
          </UiButton>
          <UiButton variant="ghost" :icon="mdiImagePlusOutline" :disabled="!!s.job" @click="addImage">{{ t('holo.orImage') }}</UiButton>
        </section>

        <UiAccordion v-model="openGroup">
        <template v-if="scene.objects.length || scene.images.length || scene.positions.length > 1">
          <!-- Where on the disc this position is, and how it is solved. -->
          <UiSection name="positions" :icon="mdiCrosshairsGps" :title="t('holo.positions')">
            <template #actions><a v-if="scene.positions.length > 1 && !manyPicked" class="action-link" role="button" tabindex="0" @click="removePosition" @keydown.enter="removePosition">{{ t('holo.removePosition') }}</a></template>
            <div class="positions" role="tablist" :aria-label="t('holo.positions')">
              <UiButton v-for="(q, i) in scene.positions" :key="q.id" role="tab" class="pos" :aria-selected="pickedIds.includes(q.id)" :variant="pickedIds.includes(q.id) ? 'soft' : 'ghost'" :class="{ clash: overlapping.includes(q.id), lead: manyPicked && q.id === positionId }" :data-tooltip="overlapping.includes(q.id) ? t('holo.overlap') : t('holo.pickMore')" @click="pickPosition($event, q.id)">{{ i + 1 }}</UiButton>
              <UiButton icon-only variant="ghost" :icon="mdiPlus" :disabled="scene.positions.length >= MAX_POSITIONS" :aria-label="t('holo.addPosition')" :data-tooltip="t('holo.addPosition')" @click="addPosition" />
            </div>
            <div v-if="manyPicked" class="small muted">{{ t('holo.pickedMany', { n: pickedIds.length }) }}</div>
            <UiSlider v-if="scene.positions.length > 1" :model-value="position.phiDeg" :label="t('holo.phi')" :min="0" :max="359" :step="1" :format="degInt" :disabled="manyPicked" @pointerdown="beginControlEdit" @update:model-value="setPhi" />
            <UiSlider :model-value="position.cropDeg" :label="t('holo.crop')" :min="5" :max="90" :step="1" :format="cropWords" @pointerdown="beginControlEdit" @update:model-value="setCrop" />
            <UiSlider
              :model-value="neutralNow"
              :label="t('holo.chroma')"
              :min="0"
              :max="3"
              :step="0.05"
              :ends="[t('holo.chromaLow'), t('holo.chromaHigh')]"
              @pointerdown="beginControlEdit"
              @update:model-value="setNeutral"
            />
            <div v-if="overlapping.length" class="notice warn">
              <UiIcon :path="mdiAlertOutline" />
              <div class="grow">{{ t('holo.overlap') }}</div>
            </div>
          </UiSection>

          <!-- What the position shows, top first: where two want the same disc, the higher wins. -->
          <UiSection name="models" :icon="mdiLayersOutline" :title="t('holo.contents')">
            <div class="obj-list" role="listbox" :aria-label="t('holo.contents')">
              <div
                v-for="(entry, i) in items"
                :key="entry.item.id"
                class="obj-row"
                role="option"
                tabindex="0"
                :aria-selected="isPicked(entry)"
                :class="{ on: isPicked(entry), hidden: entry.kind === 'model' && !entry.item.visible }"
                @click="pickItem(entry)"
                @keydown.enter.prevent="pickItem(entry)"
                @keydown.space.prevent="pickItem(entry)"
              >
                <img v-if="entry.kind === 'picture' && pictures[entry.item.path]" class="thumb" :src="pictures[entry.item.path].thumbnail" alt="" />
                <span v-else class="thumb"><UiIcon :path="entry.kind === 'model' ? mdiCubeOutline : mdiImageOutline" :size="16" class="muted" /></span>
                <span class="grow ellipsis" :data-tooltip="entry.item.name" data-tooltip-overflow>{{ entry.item.name }}</span>
                <span class="obj-tools">
                  <UiButton icon-only variant="ghost" :icon="mdiArrowUp" :aria-label="t('carve.raise')" :data-tooltip="t('carve.raise')" :disabled="i === 0" @click.stop="restackItem(entry.item.id, -1)" />
                  <UiButton icon-only variant="ghost" :icon="mdiArrowDown" :aria-label="t('carve.lower')" :data-tooltip="t('carve.lower')" :disabled="i === items.length - 1" @click.stop="restackItem(entry.item.id, 1)" />
                  <UiButton
                    v-if="entry.kind === 'model'"
                    icon-only
                    variant="ghost"
                    :icon="entry.item.visible ? mdiEyeOutline : mdiEyeOffOutline"
                    :aria-label="entry.item.visible ? t('holo.hide') : t('holo.show')"
                    :data-tooltip="entry.item.visible ? t('holo.hide') : t('holo.show')"
                    @click.stop="toggleVisibility(entry.item.id)"
                  />
                  <UiButton v-else icon-only variant="ghost" :icon="mdiContentCopy" :aria-label="t('carve.duplicate')" :data-tooltip="t('carve.duplicate')" :disabled="imagesFull" @click.stop="duplicateImage(entry.item.id)" />
                  <UiButton
                    icon-only
                    variant="ghost"
                    :icon="mdiTrashCanOutline"
                    :aria-label="t('holo.delete')"
                    :data-tooltip="t('holo.delete')"
                    @click.stop="entry.kind === 'model' ? deleteObject(entry.item.id) : removeImage(entry.item.id)"
                  />
                </span>
              </div>
            </div>
            <div class="add-row">
              <UiButton variant="ghost" :icon="mdiPlus" :disabled="loadingMesh || !!s.job" @click="addModelFromPicker">{{ t('holo.addModel') }}</UiButton>
              <UiButton variant="ghost" :icon="mdiImagePlusOutline" :disabled="imagesFull || !!s.job" :data-tooltip="imagesFull ? t('carve.full') : undefined" @click="addImage">{{ t('holo.addImage') }}</UiButton>
            </div>

            <template v-if="selectedImage">
              <div v-if="pictures[selectedImage.path]?.suggestInvert && !selectedImage.invert" class="notice warn">
                <UiIcon :path="mdiAlertOutline" />
                <div class="grow">
                  {{ t('carve.darkWarn') }}
                  <a role="button" tabindex="0" @click="invertSelectedImage" @keydown.enter="invertSelectedImage">{{ t('carve.invertWhite') }}</a>
                </div>
              </div>
              <div class="image-controls" @pointerdown.capture="beginControlEdit">
                <LayerPlace :layer="selectedImage" @reset="resetImage" />
                <UiAdvanced :label="t('holo.toneEffects')">
                  <LayerLook :layer="selectedImage" />
                </UiAdvanced>
              </div>
            </template>

          <section v-if="selectedObj" class="section">
            <div class="title row">
              <span class="grow ellipsis">{{ selectedObj.name }}</span>
              <a class="action-link" role="button" tabindex="0" @click="resetSelected" @keydown.enter="resetSelected">{{ t('holo.reset') }}</a>
            </div>
            <UiSegmented :model-value="selectedObj.mode" :options="calcModeOptions" @update:model-value="setSelectedMode($event as HoloMode)" />
            <UiSlider
              v-if="selectedObj.mode === 'real'"
              :model-value="selectedObj.parallax"
              :label="t('holo.parallax')"
              :min="1"
              :max="6"
              :step="0.1"
              :format="(x) => t('holo.times', { n: x.toFixed(1) })"
              :ends="[t('holo.parallaxLow'), t('holo.parallaxHigh')]"
              @pointerdown="beginControlEdit"
              @update:model-value="setSelectedParallax"
            />
            <UiSlider
              v-else
              :model-value="selectedTilt"
              :label="t('holo.tiltRange')"
              :min="1"
              :max="MAX_TRANSFORM_TILT_DEG"
              :step="0.5"
              :format="(x) => t('holo.tiltRangeWords', { n: x })"
              :ends="[t('holo.tiltLow'), t('holo.tiltHigh')]"
              @pointerdown="beginControlEdit"
              @update:model-value="setSelectedTilt"
            />
            <UiSwitch v-model="modelExact">{{ t('holo.exactEdit') }}</UiSwitch>
            <template v-if="!modelExact">
              <UiSlider
                :model-value="selectedObj.positionMm[2]"
                :label="t('holo.depth')"
                :min="-30"
                :max="30"
                :step="0.5"
                :format="(v) => formatDepthWords(v, t)"
                @pointerdown="beginControlEdit"
                @update:model-value="setSelectedDepth"
              />
              <UiSlider
                v-for="(axis, idx) in SIZE_AXES"
                :key="axis"
                :model-value="selectedObj.scaleMm[idx]"
                :label="t(axis)"
                :min="2"
                :max="60"
                :step="1"
                :format="mm"
                @pointerdown="beginControlEdit"
                @update:model-value="(v) => setSelectedScale(idx, v)"
              />
            </template>
            <template v-else>
              <div class="field">
                <span class="small muted">{{ t('holo.pos') }}</span>
                <div class="num-grid">
                  <label v-for="(axis, idx) in ['X', 'Y', 'Z']" :key="axis" class="num-field">
                    <span class="small muted">{{ axis }}</span>
                    <input
                      type="number"
                      class="num-input mono"
                      :value="selectedObj.positionMm[idx]"
                      step="0.5"
                      @focus="beginControlEdit"
                      @change="setSelectedPosAxis(idx, $event)"
                    />
                  </label>
                </div>
              </div>
              <div class="field">
                <span class="small muted">{{ t('holo.rot') }}</span>
                <div class="num-grid">
                  <label v-for="(axis, idx) in ['X', 'Y', 'Z']" :key="axis" class="num-field">
                    <span class="small muted">{{ axis }}</span>
                    <input
                      type="number"
                      class="num-input mono"
                      :value="selectedObj.rotationDeg[idx]"
                      step="5"
                      @focus="beginControlEdit"
                      @change="setSelectedRotAxis(idx, $event)"
                    />
                  </label>
                </div>
              </div>
              <div class="field">
                <span class="small muted">{{ t('holo.size') }} (mm)</span>
                <div class="num-grid">
                  <label v-for="(axis, idx) in ['X', 'Y', 'Z']" :key="axis" class="num-field">
                    <span class="small muted">{{ axis }}</span>
                    <input
                      type="number"
                      class="num-input mono"
                      :value="selectedObj.scaleMm[idx]"
                      min="2"
                      max="120"
                      step="1"
                      @focus="beginControlEdit"
                      @change="setSelectedScaleInput(idx, $event)"
                    />
                  </label>
                </div>
              </div>
            </template>
            <template v-if="selectedObj.mode === 'transform'">
              <div class="field">
                <span class="small muted">{{ t('holo.motionPreset') }}</span>
                <UiSelect
                  :model-value="selectedObj.motion.preset"
                  :label="t('holo.motionPreset')"
                  :options="motionPresetOptions"
                  @update:model-value="setMotionPreset($event as MotionPresetId)"
                />
              </div>
              <div v-if="selectedObj.motion.vars.length || selectedObj.motion.preset === 'custom'" class="field">
                <div class="title row">
                  <span class="grow small muted">{{ t('holo.motionVars') }}</span>
                  <a v-if="selectedObj.motion.preset === 'custom'" class="action-link" role="button" tabindex="0" @click="addMotionVar" @keydown.enter="addMotionVar">{{ t('holo.motionAddVar') }}</a>
                </div>
                <div v-for="(mv, i) in selectedObj.motion.vars" :key="i" class="var-row">
                  <input
                    v-if="selectedObj.motion.preset === 'custom'"
                    type="text"
                    class="num-input mono"
                    spellcheck="false"
                    autocomplete="off"
                    :value="mv.name"
                    :aria-label="t('holo.motionVarName')"
                    @change="setMotionVarName(i, $event)"
                  />
                  <span v-else class="var-name mono small">{{ mv.name }}</span>
                  <input
                    type="number"
                    class="num-input mono"
                    step="any"
                    :value="mv.value"
                    :aria-label="mv.name"
                    @focus="beginControlEdit"
                    @change="setMotionVarValue(i, $event)"
                  />
                  <UiButton v-if="selectedObj.motion.preset === 'custom'" icon-only variant="ghost" :icon="mdiTrashCanOutline" :aria-label="t('holo.delete')" :data-tooltip="t('holo.delete')" @click="removeMotionVar(i)" />
                </div>
              </div>
              <div v-if="motionError" class="notice warn">
                <UiIcon :path="mdiAlertOutline" />
                <div class="grow">{{ motionError }}</div>
              </div>
            </template>
            <UiAdvanced v-if="selectedObj.mode === 'transform'" :label="t('holo.motionMatrix')" :open="selectedObj.motion.preset === 'custom' || matrixOpen" @update:open="matrixOpen = $event">
              <div class="field">
                <div class="mat-grid">
                  <input
                    v-for="(expr, idx) in motionDraft"
                    :key="idx"
                    type="text"
                    class="mat-input mono"
                    spellcheck="false"
                    autocomplete="off"
                    :value="expr"
                    :readonly="selectedObj.motion.preset !== 'custom'"
                    :aria-label="`${t('holo.motionMatrix')} ${Math.floor(idx / 4) + 1},${(idx % 4) + 1}`"
                    :data-tooltip="expr"
                    data-tooltip-overflow
                    @input="onMotionCellInput(idx, $event)"
                    @blur="commitMotionMatrix"
                    @change="commitMotionMatrix"
                    @keydown.enter.prevent="commitMotionMatrix"
                  />
                </div>
              </div>
            </UiAdvanced>
            <UiAdvanced :label="t('holo.drawing')">
              <UiSlider
                :model-value="selectedObj.style.lines"
                :label="t('holo.lines')"
                :min="0"
                :max="1"
                :step="0.05"
                :ends="[t('holo.linesLow'), t('holo.linesHigh')]"
                @pointerdown="beginControlEdit"
                @update:model-value="(v) => setSelectedStyle('lines', v)"
              />
              <UiSlider
                :model-value="selectedObj.style.shading"
                :label="t('holo.shading')"
                :min="0"
                :max="1"
                :step="0.05"
                :ends="[t('holo.shadingLow'), t('holo.shadingHigh')]"
                @pointerdown="beginControlEdit"
                @update:model-value="(v) => setSelectedStyle('shading', v)"
              />
              <UiSlider
                :model-value="selectedObj.style.texture"
                :label="t('holo.texture')"
                :min="0"
                :max="1"
                :step="0.05"
                :ends="[t('holo.textureLow'), t('holo.textureHigh')]"
                @pointerdown="beginControlEdit"
                @update:model-value="(v) => setSelectedStyle('texture', v)"
              />
            </UiAdvanced>
          </section>
          </UiSection>

          <!-- How dark carved pictures may burn: one setting with the carve page. -->
          <UiSection name="safety" :icon="mdiShieldCheckOutline" :title="t('holo.carveSafety')">
            <CarveSafety />
          </UiSection>

          <!-- The lamp and where you look from, shared by every position. -->
          <UiSection name="lighting" :icon="mdiLightbulbOutline" :title="t('holo.lighting')">
            <div class="lamp-row">
              <UiSelect class="grow" :model-value="opts.lampPreset" :label="t('holo.lampPosition')" :options="lampPresetOptions" @update:model-value="setLampPreset" />
              <UiButton
                v-if="opts.lampPreset === 'custom'"
                icon-only
                variant="ghost"
                :icon="mdiContentSavePlusOutline"
                :aria-label="t('holo.lampSave')"
                :data-tooltip="t('holo.lampSave')"
                @click="openSaveLampPreset"
              />
              <UiButton
                v-else-if="isUserLampPreset"
                icon-only
                variant="ghost"
                :icon="mdiTrashCanOutline"
                :aria-label="t('holo.lampRemove')"
                :data-tooltip="t('holo.lampRemove')"
                @click="removeLampPreset(opts.lampPreset)"
              />
            </div>
            <UiSlider :model-value="opts.lampMm[0]" :label="t('holo.lampSide')" :min="-LAMP_REACH_MM" :max="LAMP_REACH_MM" :step="5" :format="lampSideWords" @update:model-value="(v) => setLampAxis(0, v)" />
            <UiSlider :model-value="opts.lampMm[1]" :label="t('holo.lampAway')" :min="-LAMP_REACH_MM" :max="LAMP_REACH_MM" :step="5" :format="lampAwayWords" @update:model-value="(v) => setLampAxis(1, v)" />
            <UiSlider v-if="opts.lampPreset !== 'ceiling'" :model-value="opts.lampMm[2]" :label="t('holo.lampUp')" :min="LAMP_MIN_UP_MM" :max="LAMP_MAX_UP_MM" :step="5" :format="cm" @update:model-value="(v) => setLampAxis(2, v)" />
            <UiSlider
              :model-value="opts.distanceMm"
              :label="t('holo.distance')"
              :min="150"
              :max="600"
              :step="10"
              :format="mm"
              @update:model-value="setDistance"
            />
          </UiSection>
        </template>

        <!-- Experimental tools -->
        <UiSection name="experimental" :icon="mdiFlaskOutline" :title="t('holo.expTools')">
          <div class="exp-actions">
            <UiButton :icon="mdiFire" :disabled="!ready" @click="burnSwatch">{{ t('holo.swatchBurn') }}</UiButton>
            <UiButton :icon="mdiCubeOutline" :disabled="loadingTest3d || !!s.job" @click="loadTest3d">{{ t('holo.test3dLoad') }}</UiButton>
          </div>

          <section class="section">
            <div class="title row">
              <span class="grow">{{ t('holo.views') }}</span>
              <UiButton variant="ghost" :icon="mdiPlus" :disabled="expViews.length >= MAX_VIEWS" @click="addExpView">{{ t('holo.addView') }}</UiButton>
            </div>
            <div class="field">
              <div class="field-head"><span>{{ t('holo.lightMode') }}</span></div>
              <UiSelect
                :model-value="expOpts.lightMode"
                :label="t('holo.lightMode')"
                :options="lightModeOptions"
                @update:model-value="expOpts.lightMode = $event as LightMode"
              />
            </div>
            <UiSlider v-model="opts.exposure" :label="t('holo.exposure')" :min="0.1" :max="1.5" :step="0.05" :format="(x) => x.toFixed(2)" />
            <UiSlider v-if="expOpts.lightMode === 'far'" v-model="expOpts.lightDeg" :label="t('holo.light')" :min="-30" :max="30" :step="0.5" :format="deg" />

            <div v-for="(v, i) in expViews" :key="v.id" class="exp-view">
              <div class="row">
                <span class="grow ellipsis small" :data-tooltip="v.imagePath" data-tooltip-overflow>{{ fileName(v.imagePath) }}</span>
                <UiButton icon-only variant="ghost" :icon="mdiClose" :aria-label="t('holo.remove')" @click="removeExpView(i)" />
              </div>
              <UiSlider v-if="expOpts.lightMode === 'lamp'" v-model="v.headXMm" :label="t('holo.headX')" :min="-150" :max="150" :step="5" :format="signedMm" />
              <UiSlider v-model="v.angleDeg" :label="t('holo.tilt')" :min="5" :max="60" :step="0.5" :format="deg" />
              <UiSlider v-if="expOpts.lightMode !== 'lamp'" v-model="v.dirDeg" :label="t('holo.azimuth')" :min="-180" :max="180" :step="1" :format="degInt" />
            </div>
            <UiButton v-if="expViews.length" variant="ghost" @click="clearExpViews">{{ t('holo.clearExp') }}</UiButton>
          </section>
        </UiSection>
        </UiAccordion>
      </div>

      <div v-if="!isDvd" class="notice warn">
        <UiIcon :path="mdiAlertOutline" />
        <div class="grow">{{ t('holo.needDvd') }}</div>
      </div>

      <div class="actions">
        <UiButton
          variant="solid"
          size="lg"
          :icon="mdiFire"
          :disabled="!canBurn"
          @click="burnDesign"
        >
          {{ t('holo.burn') }}
        </UiButton>
      </div>
    </aside>

    <!-- Centre stage: Edit | Preview -->
    <div class="stage">
      <header class="stage-bar">
        <UiSegmented v-model="tab" class="mode-seg" :options="tabOptions" />
        <span class="grow" />
        <div class="toolbar">
          <UiButton icon-only variant="ghost" :icon="mdiFilePlusOutline" :aria-label="t('holo.newDesign')" :data-tooltip="t('holo.newDesign')" @click="guardUnsaved(() => (templatePicker = true))" />
          <UiButton icon-only variant="ghost" :icon="mdiFolderOpenOutline" :aria-label="t('holo.openDesign')" :data-tooltip="`${t('holo.openDesign')} (${modKey}O)`" @click="guardUnsaved(openDesign)" />
          <UiButton icon-only variant="ghost" :icon="mdiContentSaveOutline" :aria-label="t('holo.saveDesign')" :data-tooltip="`${t('holo.saveDesign')} (${modKey}S)`" @click="saveDesign()" />
        </div>

        <Transition name="bar" mode="out-in">
        <div v-if="shownTab === 'edit'" class="toolbar" role="toolbar" :aria-label="t('holo.modeEdit')">
          <UiButton
            icon-only
            :variant="gizmoMode === 'all' ? 'soft' : 'ghost'"
            :icon="mdiAxisArrow"
            :aria-label="t('holo.transformAll')"
            :aria-pressed="gizmoMode === 'all'"
            :data-tooltip="t('holo.transformAll')"
            @click="gizmoMode = 'all'"
          />
          <UiButton
            icon-only
            :variant="gizmoMode === 'translate' ? 'soft' : 'ghost'"
            :icon="mdiCursorMove"
            :aria-label="t('holo.move')"
            :aria-pressed="gizmoMode === 'translate'"
            :data-tooltip="t('holo.move')"
            @click="gizmoMode = 'translate'"
          />
          <UiButton
            icon-only
            :variant="gizmoMode === 'rotate' ? 'soft' : 'ghost'"
            :icon="mdiRotateOrbit"
            :aria-label="t('holo.rotate')"
            :aria-pressed="gizmoMode === 'rotate'"
            :data-tooltip="t('holo.rotate')"
            @click="gizmoMode = 'rotate'"
          />
          <UiButton
            icon-only
            :variant="gizmoMode === 'scale' ? 'soft' : 'ghost'"
            :icon="mdiResize"
            :aria-label="t('holo.scaleTool')"
            :aria-pressed="gizmoMode === 'scale'"
            :data-tooltip="t('holo.scaleTool')"
            @click="gizmoMode = 'scale'"
          />
          <span class="sep" aria-hidden="true" />
          <UiButton
            icon-only
            :variant="snap ? 'soft' : 'ghost'"
            :icon="mdiGrid"
            :aria-label="t('holo.snap')"
            :aria-pressed="snap"
            :data-tooltip="t('holo.snap')"
            @click="snap = !snap"
          />
          <UiButton
            icon-only
            :variant="isolate ? 'soft' : 'ghost'"
            :icon="mdiImageFilterCenterFocus"
            :aria-label="t('holo.isolate')"
            :aria-pressed="isolate"
            :data-tooltip="`${t('holo.isolate')} (H)`"
            @click="isolate = !isolate"
          />
          <span class="sep" aria-hidden="true" />
          <UiButton
            icon-only
            variant="ghost"
            :icon="mdiUndo"
            :disabled="!undoStack.length"
            :aria-label="t('holo.undo')"
            :data-tooltip="`${t('holo.undo')} (${modKey}Z)`"
            @click="undo"
          />
          <UiButton
            icon-only
            variant="ghost"
            :icon="mdiRedo"
            :disabled="!redoStack.length"
            :aria-label="t('holo.redo')"
            :data-tooltip="`${t('holo.redo')} (⇧${modKey}Z)`"
            @click="redo"
          />
          <span class="sep" aria-hidden="true" />
          <UiButton
            icon-only
            variant="ghost"
            :icon="mdiContentCopy"
            :disabled="!selectedObj && !selectedImage"
            :aria-label="t('holo.duplicate')"
            :data-tooltip="`${t('holo.duplicate')} (${modKey}D)`"
            @click="duplicateSelected"
          />
          <UiButton
            icon-only
            variant="ghost"
            :icon="mdiTrashCanOutline"
            :disabled="!selectedObj && !selectedImage"
            :aria-label="t('holo.delete')"
            :data-tooltip="t('holo.delete')"
            @click="deleteSelected"
          />
          <span class="sep" aria-hidden="true" />
          <UiButton
            icon-only
            variant="ghost"
            :icon="mdiCrosshairsGps"
            :disabled="!selectedId"
            :aria-label="t('holo.focus')"
            :data-tooltip="t('holo.focus')"
            @click="viewportRef?.focusSelected()"
          />
          <UiButton
            icon-only
            variant="ghost"
            :icon="mdiRestore"
            :aria-label="t('holo.resetCamera')"
            :data-tooltip="t('holo.resetCamera')"
            @click="viewportRef?.resetCamera()"
          />
        </div>

        <div v-else class="toolbar">
          <span class="mono muted small">{{ previewReadout }}</span>
        </div>
        </Transition>
      </header>

      <!-- Edit viewport -->
      <Transition name="stage">
      <div v-show="shownTab === 'edit'" class="stage-body">
        <HoloViewport3D
          ref="viewportRef"
          :objects="scene.objects"
          :positions="scene.positions"
          :position-id="positionId"
          :meshes="meshCache"
          :selected-id="selectedId"
          :mode="gizmoMode"
          :snap="snap"
          :inner-radius-mm="radii.inner"
          :outer-radius-mm="radii.outer"
          :lamp-mm="opts.lampMm"
          :show-light="openGroup === 'lighting' || selectedId === LAMP_ID"
          :distance-mm="opts.distanceMm"
          :preview-t="previewT"
          :images="scene.images"
          :thumbnails="thumbnails"
          :selected-image-id="selectedImageId"
          :isolate="isolate"
          @select="selectObject"
          @select-image="choosePicture"
          @image-transform="onImageTransform"
          @position="goToPosition"
          @transform-start="onTransformStart"
          @transform-change="onTransformChange"
          @transform-end="onTransformEnd"
        />
        <div v-if="dropHover" class="drop-scrim">
          <UiIcon :path="mdiCubeOutline" :size="28" />
          <span>{{ t('holo.dropHint') }}</span>
        </div>
        <div v-if="!scene.objects.length && !scene.images.length && !loadingMesh" class="hint">{{ t('holo.dropHint') }}</div>
        <div v-if="loadingMesh" class="hint">{{ t('holo.loadingModel') }}</div>
        <div class="float legend small muted" aria-hidden="true">
          <span class="legend-name">{{ t('holo.legendDepth') }}</span>
          <span class="scale">
            <i class="ramp" />
            <i class="unusable" />
            <span>{{ t('holo.legendGood') }}</span>
            <span>{{ t('holo.legendFair') }}</span>
            <span>{{ t('holo.legendPoor') }}</span>
            <span>{{ t('holo.legendNone') }}</span>
          </span>
        </div>
      </div>
      </Transition>

      <!-- Preview: drag round the disc to turn it, move across it to tilt it -->
      <Transition name="stage">
      <div v-show="shownTab === 'preview'" class="stage-body">
        <HoloPreview v-if="previewVisited" v-show="!expViews.length" v-model:turn="lookTurn" v-model:preview-t="previewT" :mode="previewMode.mode" :active="tab === 'preview' && !expViews.length && hasPreviewSource" :weights="weights" :tables="tables" :inner-radius-mm="radii.inner" :outer-radius-mm="radii.outer" :lamp-mm="opts.lampMm" :distance-mm="opts.distanceMm" :tilt-deg="previewMode.tiltDeg" />
        <div v-if="expViews.length" class="preview-stage" :class="{ dragging: previewDragging }" @pointerdown="onPreviewPointerDown" @pointermove="onPreviewPointerMove" @pointerup="onPreviewPointerUp" @pointercancel="onPreviewPointerUp">
          <canvas ref="previewCanvas" class="preview-disc" />
        </div>
        <div v-if="!hasPreviewSource" class="hint">{{ t('holo.previewEmpty') }}</div>
        <div v-if="previewBusy" class="float small muted">{{ t('holo.previewSolve') }}</div>
        <div v-if="hasPreviewSource" class="float left small muted gestures">
          <span v-for="g in previewGestures" :key="g.text" class="gesture"><UiIcon :path="g.icon" :size="16" />{{ g.text }}</span>
        </div>
      </div>
      </Transition>
    </div>
    <UiDialog v-if="templatePicker" :title="t('holo.chooseTemplate')" @close="templatePicker = false">
      <div class="templates">
        <button v-for="tpl in DESIGN_TEMPLATES" :key="tpl.id" type="button" class="template" @click="newDesign(tpl)">
          <UiIcon :path="tpl.icon" :size="24" />
          <span class="title">{{ t(tpl.name) }}</span>
          <span class="small muted">{{ t(tpl.description) }}</span>
        </button>
      </div>
    </UiDialog>
    <UiDialog v-if="pendingDiscard" :title="t('holo.discardTitle')" @close="pendingDiscard = null">
      <p>{{ t('holo.discardBody') }}</p>
      <template #actions>
        <UiButton variant="ghost" @click="pendingDiscard = null">{{ t('common.cancel') }}</UiButton>
        <UiButton variant="danger" @click="discardAndGo">{{ t('holo.discard') }}</UiButton>
      </template>
    </UiDialog>
    <UiDialog v-if="saveLampDialog" :title="t('holo.lampSave')" @close="saveLampDialog = false">
      <label class="field">
        <span class="small muted">{{ t('holo.lampName') }}</span>
        <input
          ref="lampNameInput"
          v-model="lampPresetName"
          class="text-input"
          autocomplete="off"
          @keydown.enter="confirmSaveLampPreset"
          @keydown.escape="saveLampDialog = false"
        />
      </label>
      <template #actions>
        <UiButton variant="ghost" @click="saveLampDialog = false">{{ t('common.cancel') }}</UiButton>
        <UiButton variant="solid" :disabled="!lampPresetName.trim()" @click="confirmSaveLampPreset">{{ t('holo.save') }}</UiButton>
      </template>
    </UiDialog>
  </div>
</template>

<script setup lang="ts">
import {
  mdiAlertOctagonOutline,
  mdiAlertOutline,
  mdiArrowDown,
  mdiArrowUp,
  mdiAxisArrow,
  mdiClose,
  mdiContentCopy,
  mdiContentSaveOutline,
  mdiContentSavePlusOutline,
  mdiCrosshairsGps,
  mdiCubeOutline,
  mdiCursorDefaultOutline,
  mdiCursorMove,
  mdiEyeOffOutline,
  mdiEyeOutline,
  mdiFilePlusOutline,
  mdiFire,
  mdiFlaskOutline,
  mdiFolderOpenOutline,
  mdiGrid,
  mdiImageFilterCenterFocus,
  mdiImageOutline,
  mdiImagePlusOutline,
  mdiLayersOutline,
  mdiLightbulbOutline,
  mdiMouseLeftClickOutline,
  mdiMouseScrollWheel,
  mdiPlus,
  mdiShieldCheckOutline,
  mdiRedo,
  mdiResize,
  mdiRestore,
  mdiRotateOrbit,
  mdiStarFourPointsOutline,
  mdiTrashCanOutline,
  mdiUndo,
} from '@mdi/js'
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, shallowReactive, shallowRef, watch } from 'vue'
import { call, pickDesign, pickDesignFolder, pickImage, pickModel } from '../api'
import { DESIGN_TEMPLATES, type DesignTemplate } from '../holo/templates'
import HoloPreview from '../components/HoloPreview.vue'
import HoloViewport3D, { type GizmoMode } from '../components/HoloViewport3D.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiButton from '../components/UiButton.vue'
import UiIcon from '../components/UiIcon.vue'
import UiAccordion from '../components/UiAccordion.vue'
import UiSection from '../components/UiSection.vue'
import CarveSafety from '../components/CarveSafety.vue'
import LayerLook from '../components/LayerLook.vue'
import LayerPlace from '../components/LayerPlace.vue'
import UiSegmented from '../components/UiSegmented.vue'
import UiSelect, { type UiSelectOption } from '../components/UiSelect.vue'
import UiSlider from '../components/UiSlider.vue'
import UiSwitch from '../components/UiSwitch.vue'
import {
  BUILTIN_SAMPLE_PATH,
  DEFAULT_DATA_INNER_MM,
  DEFAULT_DATA_OUTER_MM,
  DEFAULT_DISTANCE_MM,
  DEFAULT_HEAD_SPAN_MM,
  DEFAULT_TILT_DEG,
  LAMP_CEILING_MM,
  LAMP_HELD_MM,
  LAMP_ID,
  LAMP_MAX_UP_MM,
  LAMP_MIN_UP_MM,
  LAMP_REACH_MM,
  transformTiltDeg,
  DEFAULT_TRANSFORM_TILT_DEG,
  MAX_TRANSFORM_TILT_DEG,
  type Vec3,
} from '../holo/constants'
import {
  cloneMotion,
  defaultMotion,
  evaluateMatrixSamples,
  makePresetMotion,
  MOTION_PRESET_IDS,
  VAR_NAME,
  evaluateMatrixSamples as evalMotion,
  type MotionVar,
  type MotionPresetId,
} from '../holo/motion'
import { decodeDisplayMesh, type DisplayMeshData } from '../holo/sampleMesh'
import {
  cloneScene,
  defaultInkStyle,
  defaultObjectPlacement,
  discRadiiMm,
  emptyScene,
  fileBaseName,
  formatDepthWords,
  loadLastScene,
  freePhi,
  makeObjectId,
  makePosition,
  wrapDeg,
  round1,
  saveLastScene,
  SCENE_VERSION,
  validateScene,
  type HoloInkStyle,
  DEFAULT_PARALLAX,
  type HoloItem,
  type HoloMode,
  type HoloPosition,
  normalizeOrder,
  positionItems,
  type HoloScene,
  type HoloSceneObject,
} from '../holo/scene'
import { t } from '../i18n'
import { notifyError, notifySuccess } from '../notices'
import { useProcess, type StepContext } from '../stores/process'
import { useStudio } from '../stores/studio'
import type { ArtworkAnalysis, DiscGeometryPreset } from '../types'
import { copyLayer, coverSize, makeLayer, restacked } from '../layers'
import { MAX_LAYERS } from '../discRenderer'
import UiDialog from '../components/UiDialog.vue'

type LightMode = 'flash' | 'lamp' | 'far'
interface UserLampPreset { id: string; name: string; lampMm: Vec3; distanceMm: number }
interface ExpView { id: number; imagePath: string; angleDeg: number; dirDeg: number; headXMm: number }
interface ViewSpecDto {
  imagePath: string
  angleDeg: number
  lightDeg: number
  dirDeg: number
  distanceMm: number
  lightMode: LightMode
  lightMm?: Vec3 | null
  eyeMm?: Vec3 | null
}
interface ModelViewsDto {
  views: ViewSpecDto[]
  thumbnails: string[]
  triangles: number
}

const MAX_VIEWS = 40
const MAX_HISTORY = 60
const PREVIEW_SIZE = 576
const FIRST_DEG = 8
const LAST_DEG = 25
const DEFAULT_DIR_DEG = -90
const mac = navigator.userAgent.includes('Mac')
const modKey = mac ? '⌘' : 'Ctrl+'

/** Reference DVD preset used only for preview when no user DVD calibration is selected yet. */
const FALLBACK_DVD_PRESET: DiscGeometryPreset = {
  id: 'preview-reference',
  driveModel: '',
  mediaId: '',
  stamperCode: '',
  totalSectors: 2295104,
  channelBitLengthNm: 133.3,
  innerRadiusMm: DEFAULT_DATA_INNER_MM,
  outerRadiusMm: DEFAULT_DATA_OUTER_MM,
  pitchLinear: 0,
  pitchQuadratic: 0,
  pitchCubic: 0,
  basePitchUm: 0.74,
  rmsResidualDeg: 0,
  measuredSpanMm: [25.3, 56.9],
  createdAt: '',
  media: 'dvd',
}

const s = useStudio()
const p = useProcess()

// ---- Scene & editor state ----
const tab = ref<'edit' | 'preview'>('edit')
/**
 * The view on screen, a step behind `tab`: on the way to the preview the edit view first swings
 * straight down onto the disc, turned as the preview shows it, and only then hands over; on the
 * way back it takes over face-on and swings back to where it was.
 */
const shownTab = ref<'edit' | 'preview'>('edit')
/** How long the edit view takes to swing face-on and back, ms. */
const SWING_MS = 420
const gizmoMode = ref<GizmoMode>('all')
const snap = ref(false)
/** The 3D view shows only the selected model or picture. */
const isolate = ref(false)
const scene = ref<HoloScene>(emptyScene())
const selectedId = ref<string | null>(null)
/** The settings group open in the side panel. */
const openGroup = ref('positions')
// Picking a model in the view opens its settings.
watch(selectedId, (id) => { if (id) openGroup.value = 'models' })
const MAX_POSITIONS = 24
const positionId = ref(scene.value.positions[0].id)
const position = computed(() => scene.value.positions.find((q) => q.id === positionId.value) ?? scene.value.positions[0])
const positionObjects = computed(() => scene.value.objects.filter((o) => o.positionId === position.value.id))
/** Positions that want some of the same disc. */
const overlapping = ref<string[]>([])
/** The angle at the top in the preview. */
const lookTurn = ref(0)
/** What the preview draws: the solved disc, and what each sequence looks like. */
const weights = shallowRef<ArrayBuffer | null>(null)
const tables = shallowRef<ArrayBuffer | null>(null)
let tablesFor = ''
const previewVisited = ref(false)
const missingFiles = ref<string[]>([])
const loadingMesh = ref(false)
/** Kept on the page: the title bar's notice is gone before a slow load is noticed. */
const loadError = ref('')
const dropHover = ref(false)
const viewportRef = ref<InstanceType<typeof HoloViewport3D> | null>(null)

// Cached decoded meshes keyed by model path
const meshCache = shallowReactive(new Map<string, DisplayMeshData>())

// Undo / Redo history stacks
const undoStack = ref<HoloScene[]>([])
const redoStack = ref<HoloScene[]>([])

// Shared hologram optical parameters
const opts = reactive<{
  lampPreset: string
  lampMm: Vec3
  distanceMm: number
  chromaWeight: number
  exposure: number
  grid: number
}>({
  lampPreset: 'held',
  lampMm: [...LAMP_HELD_MM],
  distanceMm: DEFAULT_DISTANCE_MM,
  chromaWeight: 1,
  exposure: 0.6,
  grid: 256,
})

// Editor-only preview parameter for transform mode (not saved, not sent to solver).
const previewT = ref(0.5)
/** The selected model's exact position, turn and size replace its depth and size sliders. */
const modelExact = ref(false)
/** The transform matrix shown for a preset; a custom one always is. */
const matrixOpen = ref(false)
/** A model's tilt either way in transform mode (`turntable::tilt_of`). */
const tiltOf = (o: HoloSceneObject) => (o.tiltDeg > 0 ? o.tiltDeg : DEFAULT_TRANSFORM_TILT_DEG)
/**
 * How the preview and the viewing guide treat a position: as the selected model there is made,
 * else animated if any of its models is (the top one's tilt), else real parallax.
 */
function lookOf(q: HoloPosition): { mode: HoloMode; tiltDeg: number } {
  const mine = scene.value.objects.filter((o) => o.positionId === q.id && o.visible)
  const chosen = mine.find((o) => o.id === selectedId.value) ?? positionItems(scene.value, q).map((x) => x.item).find((o): o is HoloSceneObject => 'mode' in o && o.mode === 'transform' && o.visible)
  return chosen ? { mode: chosen.mode, tiltDeg: tiltOf(chosen) } : { mode: 'real', tiltDeg: DEFAULT_TRANSFORM_TILT_DEG }
}
const previewMode = computed(() => lookOf(position.value))
const motionDraft = ref<string[]>([])
const motionError = ref('')

// ---- Experimental legacy tools state (collapsed inside Advanced) ----
const expViews = ref<ExpView[]>([])
const loadingTest3d = ref(false)
const expOpts = reactive<{ lightMode: LightMode; lightDeg: number }>({
  lightMode: 'lamp',
  lightDeg: 0,
})
let nextExpId = 1

// ---- Preview state ----
const previewCanvas = ref<HTMLCanvasElement | null>(null)
const lookHeadX = ref(0)
const lookTilt = ref(DEFAULT_TILT_DEG)
const lookDir = ref(DEFAULT_DIR_DEG)
const previewBusy = ref(false)
const previewDragging = ref(false)
/** What the pointer does in the preview, as an icon and a few words each. */
const previewGestures = computed(() =>
  expViews.value.length
    ? [{ icon: mdiMouseLeftClickOutline, text: t('holo.gestureView') }]
    : [
        { icon: mdiMouseLeftClickOutline, text: t('holo.gestureDrag') },
        { icon: mdiMouseScrollWheel, text: t('holo.gestureWheel') },
        { icon: mdiCursorDefaultOutline, text: t('holo.gestureTilt') },
      ],
)
let previewDragStartX = 0
let previewDragStartVal = 0
let previewTimer = 0

// ---- Derived values ----
const isDvd = computed(() => !!s.preset && s.preset.media !== 'cd')
const ready = computed(() => isDvd.value && !s.job)
const visibleObjects = computed(() => scene.value.objects.filter((o) => o.visible))
const hasPreviewSource = computed(() => expViews.value.length > 0 || visibleObjects.value.length > 0 || scene.value.images.length > 0)
const canBurn = computed(() => ready.value && hasPreviewSource.value)
const radii = computed(() => discRadiiMm(s.preset))
const selectedObj = computed(() => scene.value.objects.find((o) => o.id === selectedId.value) ?? null)

const tabOptions = computed(() => [
  { value: 'edit', label: t('holo.modeEdit') },
  { value: 'preview', label: t('holo.modePreview') },
])
const lampPresetOptions = computed<UiSelectOption[]>(() => [
  { value: 'held', label: t('holo.lampHeld') },
  { value: 'ceiling', label: t('holo.lampCeiling') },
  ...userLampPresets.value.map((p) => ({ value: p.id, label: p.name })),
  ...(opts.lampPreset === 'custom' ? [{ value: 'custom', label: t('holo.lampCustom') }] : []),
])
const lightModeOptions = computed<UiSelectOption[]>(() => [
  { value: 'lamp', label: t('holo.lightModeLamp') },
  { value: 'flash', label: t('holo.lightModeFlash') },
  { value: 'far', label: t('holo.lightModeFar') },
])
const calcModeOptions = computed<UiSelectOption[]>(() => [
  { value: 'real', label: t('holo.calcModeReal') },
  { value: 'transform', label: t('holo.calcModeTransform') },
])
const motionPresetLabels: Record<MotionPresetId, string> = {
  none: 'holo.motionNone',
  spinY: 'holo.motionSpinY',
  tiltX: 'holo.motionTiltX',
  slideX: 'holo.motionSlideX',
  grow: 'holo.motionGrow',
  custom: 'holo.motionCustom',
}
const motionPresetOptions = computed<UiSelectOption[]>(() =>
  MOTION_PRESET_IDS.map((id) => ({ value: id, label: t(motionPresetLabels[id]) })),
)

const deg = (x: number) => `${x.toFixed(1)}°`
const degInt = (x: number) => `${Math.round(x)}°`
const mm = (x: number) => t('holo.distanceUnit', { n: Math.round(x) })
const signedMm = (x: number) => {
  const n = Math.round(x)
  return t('holo.distanceUnit', { n: n > 0 ? `+${n}` : `${n}` })
}
const fileName = (path: string) => path.split(/[\\/]/).pop() ?? path
const previewReadout = computed(() => {
  if (!expViews.value.length) {
    return previewMode.value.mode === 'transform'
      ? `${degInt(lookTurn.value)}  ${deg(transformTiltDeg(previewMode.value.tiltDeg, previewT.value))}`
      : degInt(lookTurn.value)
  }
  return expOpts.lightMode === 'lamp' ? `${signedMm(lookHeadX.value)}  ${deg(lookTilt.value)}` : `${deg(lookTilt.value)}  ${degInt(lookDir.value)}`
})

// ---- History helpers ----
function pushHistory() {
  undoStack.value.push(cloneScene(scene.value))
  if (undoStack.value.length > MAX_HISTORY) undoStack.value.shift()
  redoStack.value = []
}

function undo() {
  const prev = undoStack.value.pop()
  if (!prev) return
  redoStack.value.push(cloneScene(scene.value))
  scene.value = prev
  if (selectedId.value && selectedId.value !== LAMP_ID && !scene.value.objects.some((o) => o.id === selectedId.value)) {
    selectedId.value = scene.value.objects[0]?.id ?? null
  }
  void ensureMeshesLoaded(scene.value.objects)
}

function redo() {
  const next = redoStack.value.pop()
  if (!next) return
  undoStack.value.push(cloneScene(scene.value))
  scene.value = next
  if (selectedId.value && selectedId.value !== LAMP_ID && !scene.value.objects.some((o) => o.id === selectedId.value)) {
    selectedId.value = scene.value.objects[0]?.id ?? null
  }
  void ensureMeshesLoaded(scene.value.objects)
}

// ---- Mesh loading & scene mutations ----
async function loadMeshForPath(path: string): Promise<DisplayMeshData> {
  const existing = meshCache.get(path)
  if (existing) return existing
  const buf = await call<ArrayBuffer>('holo_load_mesh', { path })
  const decoded = decodeDisplayMesh(buf)
  meshCache.set(path, decoded)
  return decoded
}

async function ensureMeshesLoaded(objects: HoloSceneObject[]): Promise<string[]> {
  const missing: string[] = []
  const uniquePaths = Array.from(new Set(objects.map((o) => o.path)))
  for (const path of uniquePaths) {
    if (meshCache.has(path)) continue
    try {
      await loadMeshForPath(path)
    } catch {
      missing.push(path)
    }
  }
  return missing
}

async function addModelPath(path: string, displayName?: string) {
  loadingMesh.value = true
  loadError.value = ''
  try {
    await loadMeshForPath(path)
    pushHistory()
    expViews.value = []
    const place = defaultObjectPlacement(s.preset, positionObjects.value.length)
    const obj: HoloSceneObject = {
      id: makeObjectId(),
      path,
      name: displayName ?? fileBaseName(path),
      visible: true,
      positionId: position.value.id,
      style: defaultInkStyle(),
      motion: defaultMotion(),
      mode: 'real',
      parallax: DEFAULT_PARALLAX,
      tiltDeg: 0,
      ...place,
    }
    scene.value.objects.push(obj)
    putOnTop(obj.id)
    chooseModel(obj.id)
    tab.value = 'edit'
  } catch (e) {
    loadError.value = String(e)
  } finally {
    loadingMesh.value = false
  }
}

async function addModelFromPicker() {
  const path = await pickModel()
  if (!path) return
  await addModelPath(path)
}

async function addSample() {
  await addModelPath(BUILTIN_SAMPLE_PATH, t('holo.sampleName'))
}

function selectObject(id: string | null) {
  chooseModel(id)
  if (!id) selectedImageId.value = null
  const obj = scene.value.objects.find((o) => o.id === id)
  if (obj && obj.positionId !== positionId.value) goToPosition(obj.positionId)
}

// ---- The position's list: models and pictures, top first ----
const items = computed(() => positionItems(scene.value, position.value))
/** One thing is selected at a time, a model or a picture. */
function chooseModel(id: string | null) {
  selectedId.value = id
  if (id) selectedImageId.value = null
}
function choosePicture(id: string | null) {
  selectedImageId.value = id
  if (id) selectedId.value = null
}
const isPicked = (x: HoloItem) => (x.kind === 'model' ? selectedId.value : selectedImageId.value) === x.item.id
function pickItem(x: HoloItem) {
  if (x.kind === 'model') chooseModel(x.item.id)
  else choosePicture(x.item.id)
}
/** `id` to the top of the shown position's list. */
function putOnTop(id: string) {
  const q = position.value
  q.order = [id, ...positionItems(scene.value, q).map((x) => x.item.id).filter((x) => x !== id)]
}
/** `by` -1 moves `id` up the list, 1 down. */
function restackItem(id: string, by: 1 | -1) {
  const ids = items.value.map((x) => ({ id: x.item.id }))
  const next = restacked(ids, id, by)
  if (!next) return
  pushHistory()
  position.value.order = next.map((x) => x.id)
}

/**
 * Positions picked together with Shift or Ctrl/Cmd: their kept range and neutral colour change as
 * one. The shown position (`positionId`) is always among them.
 */
const pickedRaw = ref<string[]>([])
const pickedIds = computed(() => {
  const ids = pickedRaw.value.filter((id) => id !== positionId.value && scene.value.positions.some((q) => q.id === id))
  return [positionId.value, ...ids]
})
const manyPicked = computed(() => pickedIds.value.length > 1)
const pickedPositions = computed(() => scene.value.positions.filter((q) => pickedIds.value.includes(q.id)))

/** A click on a position's tab: alone, with Shift a run from the shown one, with Ctrl/Cmd one more or less. */
function pickPosition(e: MouseEvent, id: string) {
  if (e.shiftKey) {
    const ids = scene.value.positions.map((q) => q.id)
    const [a, b] = [ids.indexOf(positionId.value), ids.indexOf(id)].sort((x, y) => x - y)
    pickedRaw.value = ids.slice(a, b + 1)
  } else if (e.metaKey || e.ctrlKey) {
    if (!pickedIds.value.includes(id)) {
      pickedRaw.value = [...pickedIds.value, id]
      showPosition(id)
    } else if (manyPicked.value) {
      const rest = pickedIds.value.filter((x) => x !== id)
      pickedRaw.value = rest
      if (id === positionId.value) showPosition(rest[rest.length - 1])
    }
  } else {
    goToPosition(id)
  }
}

/** Turn to a position, on its own. A model selected elsewhere is let go; nothing is picked for you. */
function goToPosition(id: string) {
  pickedRaw.value = []
  showPosition(id)
}

/** Turn to a position, keeping any picked with it. */
function showPosition(id: string) {
  positionId.value = id
  const selected = scene.value.objects.find((o) => o.id === selectedId.value)
  if (selected && selected.positionId !== id) selectedId.value = null
  lookTurn.value = position.value.phiDeg
}

function addPosition() {
  pushHistory()
  const q = makePosition(freePhi(scene.value.positions))
  scene.value.positions.push(q)
  goToPosition(q.id)
}

function removePosition() {
  if (scene.value.positions.length < 2) return
  pushHistory()
  const id = position.value.id
  const at = scene.value.positions.findIndex((q) => q.id === id)
  scene.value.objects = scene.value.objects.filter((o) => o.positionId !== id)
  scene.value.positions = scene.value.positions.filter((q) => q.id !== id)
  // The one before it, or the one that was after the first.
  goToPosition(scene.value.positions[Math.max(0, at - 1)].id)
}

function setPhi(deg: number) {
  position.value.phiDeg = wrapDeg(deg)
  lookTurn.value = position.value.phiDeg
}

function setCrop(deg: number) {
  for (const q of pickedPositions.value) q.cropDeg = Math.min(90, Math.max(5, Math.round(deg)))
}

/** How flat pictures are dithered: the carve page's burn safety settings. */
function carveTone() {
  return { servoSafeDither: s.config.servoSafeDither, maxDarkDutyCycle: s.config.maxDarkDutyCycle }
}

// ---- Pictures ----
const positionImages = computed(() => scene.value.images.filter((i) => i.positionId === position.value.id))
const imagesFull = computed(() => positionImages.value.length >= MAX_LAYERS)
const selectedImageId = ref<string | null>(null)
const selectedImage = computed(() => positionImages.value.find((i) => i.id === selectedImageId.value) ?? null)
/** Thumbnails and tone hints, by path. */
const pictures = ref<Record<string, ArtworkAnalysis>>({})
const thumbnails = computed(() => Object.fromEntries(Object.entries(pictures.value).map(([p, a]) => [p, a.thumbnail])))
watch(() => [...new Set(scene.value.images.map((i) => i.path))].join('\n'), async (joined) => {
  for (const path of joined ? joined.split('\n') : []) {
    if (pictures.value[path]) continue
    try {
      pictures.value[path] = await call<ArtworkAnalysis>('analyze_artwork', { imagePath: path })
    } catch {
      // Shown as missing when the scene is opened; a picture gone since keeps a blank thumbnail.
    }
  }
}, { immediate: true })

async function addImage() {
  if (imagesFull.value) return
  const file = await pickImage()
  if (!file) return
  try {
    const a = await call<ArtworkAnalysis>('analyze_artwork', { imagePath: file })
    pictures.value[file] = a
    pushHistory()
    const l = makeLayer(file, a, positionImages.value.length === 0)
    scene.value.images.push({ ...l, positionId: position.value.id })
    putOnTop(l.id)
    choosePicture(l.id)
    openGroup.value = 'models'
    tab.value = 'edit'
  } catch (e) {
    notifyError(String(e))
  }
}

function removeImage(id: string) {
  pushHistory()
  scene.value.images = scene.value.images.filter((i) => i.id !== id)
  normalizeOrder(scene.value)
  if (selectedImageId.value === id) selectedImageId.value = null
}

function duplicateImage(id: string) {
  const at = scene.value.images.findIndex((i) => i.id === id)
  if (at < 0 || imagesFull.value) return
  pushHistory()
  const copy = copyLayer(scene.value.images[at])
  scene.value.images.splice(at + 1, 0, copy)
  // Just above the original.
  const q = position.value
  const ids = positionItems(scene.value, q).map((x) => x.item.id).filter((x) => x !== copy.id)
  ids.splice(Math.max(0, ids.indexOf(id)), 0, copy.id)
  q.order = ids
  choosePicture(copy.id)
}

function resetImage() {
  const l = selectedImage.value
  if (!l) return
  pushHistory()
  Object.assign(l, { x: 0, y: 0, rotationDeg: 0, size: positionImages.value[0] === l ? coverSize(l) : 0.5 })
}

function invertSelectedImage() {
  if (!selectedImage.value) return
  pushHistory()
  selectedImage.value.invert = true
}

/** The position's neutral colour, or the default for its models: real parallax's when it has any. */
const neutralNow = computed(() => position.value.neutral ?? (positionObjects.value.some((o) => o.mode === 'real') || !positionObjects.value.length ? opts.chromaWeight : 0))

function setNeutral(v: number) {
  for (const q of pickedPositions.value) q.neutral = Math.round(v * 100) / 100
}

// ---- The selected model's way of making its views ----
const selectedTilt = computed(() => (selectedObj.value ? tiltOf(selectedObj.value) : DEFAULT_TRANSFORM_TILT_DEG))

function setSelectedMode(mode: HoloMode) {
  const obj = selectedObj.value
  if (!obj || obj.mode === mode) return
  pushHistory()
  obj.mode = mode
}

function setSelectedTilt(deg: number) {
  if (selectedObj.value) selectedObj.value.tiltDeg = Math.min(MAX_TRANSFORM_TILT_DEG, Math.max(1, Math.round(deg * 2) / 2))
}

function setSelectedParallax(v: number) {
  if (selectedObj.value) selectedObj.value.parallax = Math.min(6, Math.max(1, round1(v)))
}

function setMotionPreset(preset: MotionPresetId) {
  const obj = selectedObj.value
  if (!obj || obj.motion.preset === preset) return
  pushHistory()
  motionError.value = ''
  if (preset === 'custom') {
    obj.motion.preset = 'custom'
  } else {
    obj.motion = makePresetMotion(preset)
  }
}

/** Re-sample the selected motion from a matrix and variables; on a bad one keep what was there and say why. */
function applyMotion(matrix: string[], vars: MotionVar[], keepAnyway = false) {
  const obj = selectedObj.value
  if (!obj) return
  const res = evalMotion(matrix, vars)
  if (!res.ok) {
    motionError.value = t('holo.motionError', { row: Math.floor(res.cell / 4) + 1, col: (res.cell % 4) + 1, error: res.error })
    if (keepAnyway) obj.motion = { ...obj.motion, matrix, vars }
    return
  }
  motionError.value = ''
  obj.motion = { preset: obj.motion.preset, matrix, vars, samples: res.samples }
}

function setMotionVarValue(i: number, e: Event) {
  const obj = selectedObj.value
  const value = Number((e.target as HTMLInputElement).value)
  if (!obj || !Number.isFinite(value)) return
  applyMotion(obj.motion.matrix, obj.motion.vars.map((x, j) => (j === i ? { ...x, value } : x)))
}

function setMotionVarName(i: number, e: Event) {
  const obj = selectedObj.value
  const el = e.target as HTMLInputElement
  const name = el.value.trim()
  if (!obj || obj.motion.preset !== 'custom') return
  const old = obj.motion.vars[i].name
  if (name === old) return
  if (!VAR_NAME.test(name) || obj.motion.vars.some((x) => x.name === name)) {
    el.value = old
    return
  }
  pushHistory()
  const rename = (s: string) => s.replace(new RegExp(`\\{\\{\\s*${old}\\s*\\}\\}`, 'g'), `{{${name}}}`)
  applyMotion(obj.motion.matrix.map(rename), obj.motion.vars.map((x, j) => (j === i ? { ...x, name } : x)), true)
}

function addMotionVar() {
  const obj = selectedObj.value
  if (!obj || obj.motion.preset !== 'custom') return
  let n = obj.motion.vars.length + 1
  while (obj.motion.vars.some((x) => x.name === `v${n}`)) n++
  pushHistory()
  obj.motion = { ...obj.motion, vars: [...obj.motion.vars, { name: `v${n}`, value: 0 }] }
}

function removeMotionVar(i: number) {
  const obj = selectedObj.value
  if (!obj || obj.motion.preset !== 'custom') return
  pushHistory()
  applyMotion(obj.motion.matrix, obj.motion.vars.filter((_, j) => j !== i), true)
}

function onMotionCellInput(idx: number, e: Event) {
  motionDraft.value[idx] = (e.target as HTMLInputElement).value
}

function commitMotionMatrix() {
  const obj = selectedObj.value
  if (!obj || obj.motion.preset !== 'custom') return
  const next = motionDraft.value.map((s) => s.trim())
  const res = evaluateMatrixSamples(next, obj.motion.vars)
  if (!res.ok) {
    const row = Math.floor(res.cell / 4) + 1
    const col = (res.cell % 4) + 1
    motionError.value = t('holo.motionError', { row, col, error: res.error })
    return
  }
  motionError.value = ''
  motionDraft.value = next
  if (next.every((v, i) => v === obj.motion.matrix[i])) return
  pushHistory()
  obj.motion = { preset: 'custom', matrix: next, vars: obj.motion.vars, samples: res.samples }
}

watch(
  () => (selectedObj.value ? `${selectedObj.value.id}|${selectedObj.value.motion.preset}|${selectedObj.value.motion.matrix.join('\0')}` : ''),
  () => {
    motionDraft.value = selectedObj.value ? [...selectedObj.value.motion.matrix] : []
    motionError.value = ''
  },
  { immediate: true },
)

const cropWords = (deg: number) => (deg >= 90 ? t('holo.cropNone') : t('holo.cropRange', { n: Math.round(deg) }))

// Undo, open and removal can leave the current position behind.
watch(() => scene.value.positions.map((q) => q.id).join(), () => {
  if (!scene.value.positions.some((q) => q.id === positionId.value)) goToPosition(scene.value.positions[0].id)
})

function toggleVisibility(id: string) {
  const obj = scene.value.objects.find((o) => o.id === id)
  if (!obj) return
  pushHistory()
  obj.visible = !obj.visible
}

function deleteObject(id: string) {
  const idx = scene.value.objects.findIndex((o) => o.id === id)
  if (idx < 0) return
  pushHistory()
  scene.value.objects.splice(idx, 1)
  normalizeOrder(scene.value)
  if (selectedId.value === id) {
    selectedId.value = scene.value.objects[Math.min(idx, scene.value.objects.length - 1)]?.id ?? null
  }
}

/** Whatever is selected, a model or a picture. */
function deleteSelected() {
  if (selectedObj.value) deleteObject(selectedObj.value.id)
  else if (selectedImage.value) removeImage(selectedImage.value.id)
}

function duplicateSelected() {
  if (selectedImage.value) return duplicateImage(selectedImage.value.id)
  const src = selectedObj.value
  if (!src) return
  pushHistory()
  const copy: HoloSceneObject = {
    ...src,
    id: makeObjectId(),
    positionMm: [round1(src.positionMm[0] + 6), round1(src.positionMm[1]), round1(src.positionMm[2])],
    rotationDeg: [...src.rotationDeg] as Vec3,
    scaleMm: [...src.scaleMm] as Vec3,
    style: { ...src.style },
    motion: cloneMotion(src.motion),
  }
  scene.value.objects.push(copy)
  putOnTop(copy.id)
  chooseModel(copy.id)
}

function resetSelected() {
  const obj = selectedObj.value
  if (!obj) return
  pushHistory()
  const place = defaultObjectPlacement(s.preset, 0)
  obj.positionMm = [...place.positionMm] as Vec3
  obj.rotationDeg = [...place.rotationDeg] as Vec3
  obj.scaleMm = [...place.scaleMm] as Vec3
}

let sliderSnapshotTaken = false
function beginControlEdit() {
  if (!sliderSnapshotTaken) {
    pushHistory()
    sliderSnapshotTaken = true
    window.setTimeout(() => (sliderSnapshotTaken = false), 350)
  }
}

function setSelectedDepth(z: number) {
  const obj = selectedObj.value
  if (!obj) return
  obj.positionMm[2] = round1(z)
}

/** The model's size along each of its own axes: width, height and thickness. */
const SIZE_AXES = ['holo.sizeX', 'holo.sizeY', 'holo.sizeZ'] as const

function setSelectedScale(idx: number, v: number) {
  const obj = selectedObj.value
  if (!obj) return
  obj.scaleMm[idx] = round1(v)
}

function setSelectedStyle(key: keyof HoloInkStyle, v: number) {
  const obj = selectedObj.value
  if (!obj) return
  obj.style[key] = Math.round(v * 100) / 100
}

function setSelectedPosAxis(idx: number, e: Event) {
  const obj = selectedObj.value
  if (!obj) return
  const v = Number((e.target as HTMLInputElement).value)
  if (!Number.isFinite(v)) return
  obj.positionMm[idx] = Math.max(-60, Math.min(60, round1(v)))
}

function setSelectedRotAxis(idx: number, e: Event) {
  const obj = selectedObj.value
  if (!obj) return
  const v = Number((e.target as HTMLInputElement).value)
  if (!Number.isFinite(v)) return
  obj.rotationDeg[idx] = Math.max(-180, Math.min(180, round1(v)))
}

function setSelectedScaleInput(idx: number, e: Event) {
  const obj = selectedObj.value
  if (!obj) return
  const v = Number((e.target as HTMLInputElement).value)
  if (!Number.isFinite(v)) return
  obj.scaleMm[idx] = Math.max(2, Math.min(120, round1(v)))
}

function onTransformStart(_id: string) {
  pushHistory()
}

function onTransformChange(payload: { id: string; positionMm: Vec3; rotationDeg: Vec3; scaleMm: Vec3 }) {
  if (payload.id === LAMP_ID) {
    opts.lampMm = payload.positionMm
    opts.lampPreset = matchLampPreset(opts.lampMm, opts.distanceMm)
    return
  }
  const obj = scene.value.objects.find((o) => o.id === payload.id)
  if (!obj) return
  obj.positionMm = payload.positionMm
  obj.rotationDeg = payload.rotationDeg
  obj.scaleMm = payload.scaleMm
}

/** A picture moved, turned or sized with the handles in the 3D view. */
function onImageTransform(p: { id: string; x: number; y: number; size: number; stretchX: number; stretchY: number; rotationDeg: number }) {
  const img = scene.value.images.find((i) => i.id === p.id)
  if (img) Object.assign(img, { x: p.x, y: p.y, size: p.size, stretchX: p.stretchX, stretchY: p.stretchY, rotationDeg: p.rotationDeg })
}

function onTransformEnd(_id: string) {
  saveLastScene(scene.value)
}

// ---- Lamp & optical controls ----
const LAMP_PRESETS_KEY = 'discratcher.holo.lampPresets'

function isFiniteNum(x: unknown): x is number {
  return typeof x === 'number' && Number.isFinite(x)
}

function loadUserLampPresets(): UserLampPreset[] {
  try {
    const raw = localStorage.getItem(LAMP_PRESETS_KEY)
    if (!raw) return []
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed)) return []
    const seen = new Set<string>(['held', 'ceiling', 'custom'])
    const out: UserLampPreset[] = []
    for (const item of parsed) {
      if (!item || typeof item !== 'object') return []
      const r = item as Record<string, unknown>
      if (
        typeof r.id !== 'string' ||
        !r.id ||
        seen.has(r.id) ||
        typeof r.name !== 'string' ||
        !r.name.trim() ||
        !Array.isArray(r.lampMm) ||
        r.lampMm.length !== 3 ||
        !r.lampMm.every(isFiniteNum) ||
        !isFiniteNum(r.distanceMm)
      ) {
        return []
      }
      seen.add(r.id)
      out.push({ id: r.id, name: r.name.trim(), lampMm: [r.lampMm[0], r.lampMm[1], r.lampMm[2]], distanceMm: r.distanceMm })
    }
    return out
  } catch {
    return []
  }
}

function saveUserLampPresets(presets: UserLampPreset[]) {
  try {
    localStorage.setItem(LAMP_PRESETS_KEY, JSON.stringify(presets))
  } catch {
    // Storage unavailable; presets stay in memory for this session.
  }
}

const userLampPresets = ref<UserLampPreset[]>(loadUserLampPresets())
const isUserLampPreset = computed(() => userLampPresets.value.some((p) => p.id === opts.lampPreset))
const saveLampDialog = ref(false)
const lampPresetName = ref('')
const lampNameInput = ref<HTMLInputElement | null>(null)

const sameVec3 = (a: Vec3, b: Vec3) => a[0] === b[0] && a[1] === b[1] && a[2] === b[2]

function matchLampPreset(lampMm: Vec3, distanceMm: number): string {
  if (sameVec3(lampMm, LAMP_HELD_MM) && distanceMm === DEFAULT_DISTANCE_MM) return 'held'
  if (sameVec3(lampMm, LAMP_CEILING_MM) && distanceMm === DEFAULT_DISTANCE_MM) return 'ceiling'
  const hit = userLampPresets.value.find((p) => sameVec3(p.lampMm, lampMm) && p.distanceMm === distanceMm)
  return hit ? hit.id : 'custom'
}

function setLampPreset(preset: string) {
  if (preset === 'held') {
    opts.lampPreset = 'held'
    opts.lampMm = [...LAMP_HELD_MM]
    opts.distanceMm = DEFAULT_DISTANCE_MM
  } else if (preset === 'ceiling') {
    opts.lampPreset = 'ceiling'
    opts.lampMm = [...LAMP_CEILING_MM]
    opts.distanceMm = DEFAULT_DISTANCE_MM
  } else {
    const found = userLampPresets.value.find((p) => p.id === preset)
    if (!found) return
    opts.lampPreset = found.id
    opts.lampMm = [...found.lampMm]
    opts.distanceMm = found.distanceMm
  }
}

function setLampAxis(idx: number, v: number) {
  opts.lampMm[idx] = Math.round(v)
  opts.lampPreset = matchLampPreset(opts.lampMm, opts.distanceMm)
}

function setDistance(v: number) {
  opts.distanceMm = Math.round(v)
  opts.lampPreset = matchLampPreset(opts.lampMm, opts.distanceMm)
}

function openSaveLampPreset() {
  let n = userLampPresets.value.length + 1
  while (userLampPresets.value.some((p) => p.name === t('holo.lampDefaultName', { n }))) n++
  lampPresetName.value = t('holo.lampDefaultName', { n })
  saveLampDialog.value = true
  void nextTick(() => {
    lampNameInput.value?.focus()
    lampNameInput.value?.select()
  })
}

function confirmSaveLampPreset() {
  const name = lampPresetName.value.trim()
  if (!name) return
  const preset: UserLampPreset = {
    id: `lamp-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
    name,
    lampMm: [...opts.lampMm] as Vec3,
    distanceMm: opts.distanceMm,
  }
  userLampPresets.value = [...userLampPresets.value, preset]
  saveUserLampPresets(userLampPresets.value)
  opts.lampPreset = preset.id
  saveLampDialog.value = false
}

function removeLampPreset(id: string) {
  userLampPresets.value = userLampPresets.value.filter((p) => p.id !== id)
  saveUserLampPresets(userLampPresets.value)
  opts.lampPreset = matchLampPreset(opts.lampMm, opts.distanceMm)
}

const cm = (x: number) => t('holo.cm', { n: Math.round(Math.abs(x) / 5) / 2 })
const lampSideWords = (x: number) => (x === 0 ? t('holo.centred') : t(x > 0 ? 'holo.toRight' : 'holo.toLeft', { d: cm(x) }))
const lampAwayWords = (y: number) => (y === 0 ? t('holo.centred') : t(y > 0 ? 'holo.awayFromYou' : 'holo.towardsYou', { d: cm(y) }))

// The lamp belongs to the scene, so it is saved, opened and undone with it.
watch(() => [...opts.lampMm], (v) => {
  if (String(v) !== String(scene.value.lampMm)) scene.value.lampMm = [...v] as Vec3
}, { immediate: true })
watch(() => scene.value, (sc) => {
  if (!sc.lampMm) sc.lampMm = [...opts.lampMm] as Vec3
  else if (String(sc.lampMm) !== String(opts.lampMm)) {
    const at = sc.lampMm
    opts.lampMm = [...at]
    opts.lampPreset = matchLampPreset(at, opts.distanceMm)
  }
})

// ---- Designs: a folder holding design.json and every file the design uses ----
/** The design's folder, once saved or opened. */
const designDir = ref<string | null>(null)
/** The scene as last saved, opened or started, to tell unsaved changes. */
const savedScene = ref('')
const markSaved = () => (savedScene.value = JSON.stringify(scene.value))
/** The design's folder and its last saved state, kept for the next start with the scene itself. */
const LAST_DESIGN_KEY = 'discratcher.holo.lastDesign'
watch([designDir, savedScene], () => {
  try {
    localStorage.setItem(LAST_DESIGN_KEY, JSON.stringify({ dir: designDir.value, saved: savedScene.value }))
  } catch {
    // Storage unavailable: the next start just forgets the folder.
  }
})
/** A design never saved counts as changed once it holds anything. */
const dirty = computed(() =>
  designDir.value ? JSON.stringify(scene.value) !== savedScene.value : scene.value.objects.length + scene.value.images.length > 0 && JSON.stringify(scene.value) !== savedScene.value,
)
const designName = computed(() => (designDir.value ? fileName(designDir.value) : t('holo.untitled')))
const templatePicker = ref(false)

/** Something that would replace the design, waiting for the user to let unsaved changes go. */
const pendingDiscard = ref<null | (() => void)>(null)
function guardUnsaved(then: () => void) {
  if (dirty.value) pendingDiscard.value = then
  else then()
}
function discardAndGo() {
  const then = pendingDiscard.value
  pendingDiscard.value = null
  then?.()
}

/** Save into the design's folder, or a new one when there is none yet or `asNew`. */
async function saveDesign(asNew = false) {
  let dir = asNew ? null : designDir.value
  if (!dir) dir = await pickDesignFolder(t('holo.untitled'))
  if (!dir) return
  try {
    const saved = validateScene(await call<HoloScene>('design_save', { dir, scene: cloneScene(scene.value) }))
    // Only the paths change: they now point at the copies inside the folder.
    for (const o of scene.value.objects) o.path = saved.objects.find((x) => x.id === o.id)?.path ?? o.path
    for (const i of scene.value.images) i.path = saved.images.find((x) => x.id === i.id)?.path ?? i.path
    await ensureMeshesLoaded(scene.value.objects)
    designDir.value = dir
    markSaved()
    saveLastScene(scene.value)
    notifySuccess(t('holo.saved', { name: fileName(dir) }))
  } catch (e) {
    notifyError(String(e))
  }
}

async function openDesign() {
  const path = await pickDesign()
  if (!path) return
  try {
    const opened = await call<{ scene: unknown; dir: string | null }>('design_open', { path })
    await applyLoadedScene(validateScene(opened.scene), opened.dir)
    notifySuccess(t('holo.opened', { name: fileName(opened.dir ?? path) }))
  } catch (e) {
    notifyError(String(e))
  }
}

/** A new, unsaved design from `tpl`. */
function newDesign(tpl: DesignTemplate) {
  templatePicker.value = false
  const fresh = tpl.make()
  scene.value = fresh
  positionId.value = fresh.positions[0].id
  pickedRaw.value = []
  lookTurn.value = fresh.positions[0].phiDeg
  selectedId.value = null
  selectedImageId.value = null
  designDir.value = null
  missingFiles.value = []
  expViews.value = []
  undoStack.value = []
  redoStack.value = []
  markSaved()
}

async function applyLoadedScene(loaded: HoloScene, dir: string | null) {
  const missing = await ensureMeshesLoaded(loaded.objects)
  // A picture is there if it can be read.
  for (const path of new Set(loaded.images.map((i) => i.path))) {
    try {
      pictures.value[path] = await call<ArtworkAnalysis>('analyze_artwork', { imagePath: path })
    } catch {
      missing.push(path)
    }
  }
  const missingSet = new Set(missing)
  const kept = loaded.objects.filter((o) => !missingSet.has(o.path))
  const images = loaded.images.filter((i) => !missingSet.has(i.path))
  missingFiles.value = missing.map((p) => fileName(p))
  scene.value = { version: SCENE_VERSION, positions: loaded.positions, objects: kept, images, ...(loaded.lampMm ? { lampMm: loaded.lampMm } : {}) }
  positionId.value = loaded.positions[0].id
  lookTurn.value = loaded.positions[0].phiDeg
  selectedId.value = kept.find((o) => o.positionId === positionId.value)?.id ?? null
  designDir.value = dir
  expViews.value = []
  undoStack.value = []
  redoStack.value = []
  markSaved()
}


// ---- Drag & drop (HTML5 + Tauri native file drop) ----
const MODEL_EXT_RE = /\.(obj|glb|gltf)$/i
function onDragOver(e: DragEvent) {
  if (e.dataTransfer?.types.includes('Files')) dropHover.value = true
}
function onDragLeave(e: DragEvent) {
  if (e.currentTarget === e.target) dropHover.value = false
}
async function onDrop(e: DragEvent) {
  dropHover.value = false
  const files = Array.from(e.dataTransfer?.files ?? [])
  for (const f of files) {
    const p = (f as File & { path?: string }).path ?? f.name
    if (MODEL_EXT_RE.test(p)) {
      await addModelPath(p, fileBaseName(f.name))
    }
  }
}

let unlistenTauriDrop: (() => void) | null = null
async function setupTauriFileDrop() {
  if (!('__TAURI_INTERNALS__' in window)) return
  try {
    const { getCurrentWebview } = await import('@tauri-apps/api/webview')
    unlistenTauriDrop = await getCurrentWebview().onDragDropEvent((ev) => {
      if (s.view !== 'holo') return
      if (ev.payload.type === 'enter' || ev.payload.type === 'over') {
        dropHover.value = true
      } else if (ev.payload.type === 'leave') {
        dropHover.value = false
      } else if (ev.payload.type === 'drop') {
        dropHover.value = false
        for (const fp of ev.payload.paths) {
          if (MODEL_EXT_RE.test(fp)) void addModelPath(fp)
        }
      }
    })
  } catch {
    // Webview drop API unavailable; HTML5 fallback remains active.
  }
}

// ---- Experimental legacy tools (kept working inside collapsed section) ----
function spreadExpViews() {
  const n = expViews.value.length
  if (expOpts.lightMode === 'lamp') {
    expViews.value.forEach((v, i) => {
      v.headXMm = n === 1 ? 0 : -DEFAULT_HEAD_SPAN_MM + (2 * DEFAULT_HEAD_SPAN_MM * i) / (n - 1)
    })
  } else {
    expViews.value.forEach((v, i) => {
      v.angleDeg = n === 1 ? (FIRST_DEG + LAST_DEG) / 2 : FIRST_DEG + ((LAST_DEG - FIRST_DEG) * i) / (n - 1)
    })
  }
}

async function addExpView() {
  const path = await pickImage()
  if (!path) return
  const tilt = expOpts.lightMode === 'lamp' ? (opts.lampPreset === 'ceiling' ? 30 : DEFAULT_TILT_DEG) : 0
  expViews.value.push({ id: nextExpId++, imagePath: path, angleDeg: tilt, dirDeg: DEFAULT_DIR_DEG, headXMm: 0 })
  spreadExpViews()
  tab.value = 'preview'
}

function removeExpView(i: number) {
  expViews.value.splice(i, 1)
}

function clearExpViews() {
  expViews.value = []
}

async function loadTest3d() {
  loadingTest3d.value = true
  try {
    const specs = await call<ViewSpecDto[]>('holo_test3d_views', { preset: s.preset })
    if (!specs.length) return
    expOpts.lightMode = specs[0].lightMode
    expOpts.lightDeg = specs[0].lightDeg
    expViews.value = specs.map((v) => {
      if (v.lightMode === 'lamp' && v.eyeMm) {
        const tilt = (Math.atan2(-v.eyeMm[1], v.eyeMm[2]) * 180) / Math.PI
        return { id: nextExpId++, imagePath: v.imagePath, angleDeg: Math.round(tilt * 2) / 2, dirDeg: DEFAULT_DIR_DEG, headXMm: v.eyeMm[0] }
      }
      return { id: nextExpId++, imagePath: v.imagePath, angleDeg: v.angleDeg, dirDeg: v.dirDeg, headXMm: 0 }
    })
    tab.value = 'preview'
  } catch (e) {
    notifyError(String(e))
  } finally {
    loadingTest3d.value = false
  }
}

function burnSwatch() {
  const preset = s.preset!
  p.openBurn({
    title: t('holo.swatchTitle'),
    note: t('holo.swatchNote'),
    fileName: 'holo-swatch.iso',
    prepareLabel: t('holo.compose'),
    prepare: async (path: string) => {
      const r = await call<{ mismatchedBytes: number; layoutPath: string }>('holo_build_swatch_iso', { preset, outputIsoPath: path })
      if (r.mismatchedBytes) throw t('holo.selfCheckFail', { n: r.mismatchedBytes })
    },
  })
}

// ---- Preview & Burn pipeline ----
function activePresetForPreview(): DiscGeometryPreset {
  return s.preset && s.preset.media !== 'cd' ? s.preset : FALLBACK_DVD_PRESET
}

const sceneOptions = () => ({
  distanceMm: opts.distanceMm,
  chromaWeight: opts.chromaWeight,
  exposure: opts.exposure,
  grid: GRID,
})

function expViewSpec(imagePath: string, angleDeg: number, dirDeg: number, headXMm: number): ViewSpecDto {
  if (expOpts.lightMode === 'lamp') {
    const a = (angleDeg * Math.PI) / 180
    return {
      imagePath,
      angleDeg,
      lightDeg: 0,
      dirDeg: DEFAULT_DIR_DEG,
      distanceMm: opts.distanceMm,
      lightMode: 'lamp',
      lightMm: [...opts.lampMm],
      eyeMm: [headXMm, -opts.distanceMm * Math.sin(a), opts.distanceMm * Math.cos(a)],
    }
  }
  return {
    imagePath,
    angleDeg,
    lightDeg: expOpts.lightDeg,
    dirDeg,
    distanceMm: opts.distanceMm,
    lightMode: expOpts.lightMode,
  }
}

/** Quick first pass while the full one is worked out. */
const DRAFT_GRID = 192
/** A cell of 0.23 mm, about the length of disc one sync frame takes: finer shows nothing more. */
const GRID = 512
let sceneVersion = 0
let solvedVersion = -1
let solving = false

/** Solves the scene as it is now, coarse first; a change on the way starts over. */
async function solveScene() {
  if (solving || expViews.value.length || !visibleObjects.value.length) return
  solving = true
  try {
    while (solvedVersion !== sceneVersion) {
      const version = sceneVersion
      const request = { scene: cloneScene(scene.value), options: sceneOptions(), preset: activePresetForPreview() }
      previewBusy.value = true
      const key = `${request.preset.id} ${request.preset.channelBitLengthNm} ${request.preset.basePitchUm}`
      if (tablesFor !== key) {
        tables.value = await call<ArrayBuffer>('holo_response_tables', { preset: request.preset })
        tablesFor = key
      }
      for (const grid of [DRAFT_GRID, GRID]) {
        const options = { ...request.options, grid }
        const solved = await call<{ overlapping: string[] }>('holo_solve_scene', { ...request, options })
        if (version !== sceneVersion) break
        overlapping.value = solved.overlapping
        weights.value = await call<ArrayBuffer>('holo_scene_weights', { ...request, options, tone: carveTone() })
        if (grid === GRID) solvedVersion = version
      }
    }
  } catch (e) {
    solvedVersion = sceneVersion
    notifyError(String(e))
  } finally {
    solving = false
    previewBusy.value = false
  }
}

let frameWanted = false
let framing = false
/** Draws as often as frames come back: the latest look each time, none skipped for waiting. */
function requestFrame() {
  frameWanted = true
  if (!framing) void drawFrames()
}
async function drawFrames() {
  framing = true
  try {
    while (frameWanted && tab.value === 'preview') {
      frameWanted = false
      const buf = await frame()
      const c = previewCanvas.value
      if (!buf || !c) continue
      if (c.width !== PREVIEW_SIZE) c.width = c.height = PREVIEW_SIZE
      c.getContext('2d')!.putImageData(new ImageData(new Uint8ClampedArray(buf), PREVIEW_SIZE, PREVIEW_SIZE), 0, 0)
    }
  } catch (e) {
    notifyError(String(e))
  } finally {
    framing = false
  }
}
function frame(): Promise<ArrayBuffer> | null {
  if (expViews.value.length) {
    return call<ArrayBuffer>('holo_design_preview', {
      preset: activePresetForPreview(),
      input: {
        views: expViews.value.map((v) => expViewSpec(v.imagePath, v.angleDeg, v.dirDeg, v.headXMm)),
        chromaWeight: opts.chromaWeight,
        exposure: opts.exposure,
        grid: opts.grid,
      },
      view: expViewSpec('', lookTilt.value, lookDir.value, lookHeadX.value),
      size: PREVIEW_SIZE,
    })
  }
  return null
}

/** Something that is burned changed. Worked out in the background too, so the preview is ready. */
function sceneChanged() {
  sceneVersion++
  clearTimeout(previewTimer)
  previewTimer = window.setTimeout(() => void solveScene(), tab.value === 'preview' ? 120 : 500)
}

function onPreviewPointerDown(e: PointerEvent) {
  if (e.button !== 0) return
  previewDragging.value = true
  previewDragStartX = e.clientX
  previewDragStartVal = expOpts.lightMode !== 'lamp' ? lookDir.value : lookHeadX.value
  ;(e.currentTarget as HTMLElement | null)?.setPointerCapture(e.pointerId)
}

function onPreviewPointerMove(e: PointerEvent) {
  if (!previewDragging.value) return
  const dx = e.clientX - previewDragStartX
  if (expOpts.lightMode !== 'lamp') lookDir.value = Math.max(-180, Math.min(180, Math.round(previewDragStartVal + dx * 0.6)))
  else lookHeadX.value = Math.max(-DEFAULT_HEAD_SPAN_MM, Math.min(DEFAULT_HEAD_SPAN_MM, Math.round(previewDragStartVal + dx * 0.55)))
}

function onPreviewPointerUp() {
  previewDragging.value = false
}

function burnDesign() {
  const preset = s.preset!
  const usingExp = expViews.value.length > 0
  const sceneSnapshot = cloneScene(scene.value)
  const expSnapshot = expViews.value.map((v) => expViewSpec(v.imagePath, v.angleDeg, v.dirDeg, v.headXMm))
  const sceneOpts = sceneOptions()
  const chromaWeight = opts.chromaWeight
  const exposure = opts.exposure
  const grid = opts.grid

  const guidePos = positionObjects.value.some((o) => o.visible)
    ? position.value
    : sceneSnapshot.positions.find((q) => sceneSnapshot.objects.some((o) => o.visible && o.positionId === q.id)) ?? null
  // Flat pictures show in any light; a disc of nothing else needs no viewing guide.
  const tone = carveTone()

  p.openBurn({
    title: t('holo.burnTitle'),
    note: usingExp
      ? t('holo.burnNote', { n: expSnapshot.length })
      : visibleObjects.value.length
        ? t('holo.burnNoteModels', { n: visibleObjects.value.length })
        : t('holo.burnNoteImages', { n: scene.value.images.length }),
    fileName: 'holo.iso',
    prepareLabel: t('holo.compose'),
    viewingGuide: usingExp ? 'real' : guidePos ? lookOf(guidePos).mode : false,
    viewingTiltDeg: guidePos ? lookOf(guidePos).tiltDeg : DEFAULT_TRANSFORM_TILT_DEG,
    prepare: async (path: string, ctx: StepContext) => {
      if (usingExp) {
        await call('holo_build_design_iso', { preset, input: { views: expSnapshot, chromaWeight, exposure, grid }, outputIsoPath: path })
      } else {
        ctx.report(5, t('holo.previewSolve'))
        await call('holo_build_scene_iso', { preset, scene: sceneSnapshot, options: sceneOpts, tone, outputIsoPath: path })
      }
    },
  })
}

// ---- Keyboard shortcuts ----
function isTypingTarget(el: EventTarget | null): boolean {
  if (!(el instanceof HTMLElement)) return false
  const tag = el.tagName
  return tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT' || el.isContentEditable
}

function onKeyDown(e: KeyboardEvent) {
  if (s.view !== 'holo' || isTypingTarget(e.target)) return
  const mod = e.metaKey || e.ctrlKey
  const key = e.key.toLowerCase()

  if (mod && key === 'z') {
    e.preventDefault()
    if (e.shiftKey) redo()
    else undo()
    return
  }
  if (mod && key === 'y') {
    e.preventDefault()
    redo()
    return
  }
  if (mod && key === 'd') {
    e.preventDefault()
    duplicateSelected()
    return
  }
  if (mod && key === 's') {
    e.preventDefault()
    void saveDesign(e.shiftKey)
    return
  }
  if (mod && key === 'o') {
    e.preventDefault()
    guardUnsaved(openDesign)
    return
  }
  if (mod || e.altKey) return

  if (tab.value === 'edit') {
    if (key === 'q') { gizmoMode.value = 'all'; e.preventDefault() }
    else if (key === 'h') { isolate.value = !isolate.value; e.preventDefault() }
    else if (key === 'w') { gizmoMode.value = 'translate'; e.preventDefault() }
    else if (key === 'e') { gizmoMode.value = 'rotate'; e.preventDefault() }
    else if (key === 'r') { gizmoMode.value = 'scale'; e.preventDefault() }
    else if (key === 'f' && selectedObj.value) { viewportRef.value?.focusSelected(); e.preventDefault() }
    else if ((e.key === 'Delete' || e.key === 'Backspace') && (selectedObj.value || selectedImage.value)) {
      deleteSelected()
      e.preventDefault()
    }
  }
}

// ---- Reactive persistence & preview watchers ----
watch(scene, (val) => {
  saveLastScene(val)
  sceneChanged()
}, { deep: true })

watch([opts, () => s.preset], sceneChanged, { deep: true })
// Flat pictures are previewed with the burn safety they will be burned with.
watch(() => [s.config.servoSafeDither, s.config.maxDarkDutyCycle], () => {
  if (scene.value.images.length) sceneChanged()
})

watch([expViews, expOpts], () => {
  if (tab.value === 'preview') requestFrame()
}, { deep: true })

watch(tab, async (to) => {
  const turn = lookTurn.value - position.value.phiDeg
  const view = viewportRef.value
  if (to === 'preview') {
    await view?.faceOn(turn, SWING_MS)
    if (tab.value === 'preview') shownTab.value = 'preview'
  } else {
    await view?.faceOn(turn, 0)
    shownTab.value = 'edit'
    // Once the preview has faded out.
    window.setTimeout(() => { if (tab.value === 'edit') void viewportRef.value?.faceOff(SWING_MS) }, 200)
  }
})

watch(tab, () => {
  if (tab.value !== 'preview') return
  previewVisited.value = true
  clearTimeout(previewTimer)
  void solveScene()
  requestFrame()
})

watch([lookHeadX, lookTilt, lookDir], () => {
  if (tab.value === 'preview') requestFrame()
})

onMounted(async () => {
  window.addEventListener('keydown', onKeyDown)
  void setupTauriFileDrop()

  const restored = loadLastScene()
  if (restored && (restored.objects.length || restored.images.length)) {
    let last: { dir?: string | null; saved?: string } = {}
    try {
      last = JSON.parse(localStorage.getItem(LAST_DESIGN_KEY) ?? '{}')
    } catch {
      // Unreadable: start as unsaved.
    }
    await applyLoadedScene(restored, last.dir ?? null)
    if (typeof last.saved === 'string') savedScene.value = last.saved
  }
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeyDown)
  clearTimeout(previewTimer)
  unlistenTauriDrop?.()
})
</script>

<style scoped>
/* The stage runs to the panel's edge, so the panel keeps its own gap from it. */
.side { width: calc(var(--oc-page-gutter) + 348px + var(--oc-space-lg)); flex: none; min-height: 0; display: flex; flex-direction: column; gap: var(--oc-space-md); padding: 64px var(--oc-space-lg) var(--oc-space-lg) var(--oc-page-gutter); }
.fields { width: auto; max-width: none; min-height: 0; overflow-y: auto; scrollbar-gutter: stable; }
.fields .section + .section { margin-top: var(--oc-space-sm); }
.actions { flex: none; display: flex; gap: var(--oc-space-sm); }
.drop { width: 100%; height: 148px; padding: 0; border: 1px dashed var(--oc-border-strong); border-radius: var(--oc-radius-control); background: var(--oc-bg-base); display: grid; place-items: center; }
.drop:hover:not(:disabled), .drop.over { background-image: linear-gradient(var(--oc-bg-hover), var(--oc-bg-hover)); }
.drop:disabled { opacity: var(--oc-opacity-disabled); }
.drop-inner { display: flex; flex-direction: column; align-items: center; gap: var(--oc-space-2xs); }
.obj-list { display: flex; flex-direction: column; gap: 2px; }
.design-title { display: flex; flex-direction: column; gap: 2px; min-width: 0; }
.templates { display: grid; grid-template-columns: 1fr 1fr; gap: var(--oc-space-sm); }
.template { display: flex; flex-direction: column; align-items: flex-start; gap: var(--oc-space-2xs); padding: var(--oc-space-md); border: 1px solid var(--oc-border-muted); border-radius: var(--oc-radius-control); background: var(--oc-bg-base); color: var(--oc-fg-default); text-align: left; }
.template:hover { border-color: var(--oc-accent); }
.image-controls { display: flex; flex-direction: column; gap: var(--oc-space-md); }
.add-row { display: flex; gap: var(--oc-space-2xs); }
.add-row > * { flex: 1; }
img.thumb { object-fit: cover; }
.obj-row { display: flex; align-items: center; gap: var(--oc-space-sm); height: 40px; padding: 0 var(--oc-space-2xs) 0 var(--oc-space-xs); border-radius: var(--oc-radius-control); cursor: pointer; user-select: none; }
.obj-row:hover { background: var(--oc-bg-hover); }
.obj-row.on { background: var(--oc-bg-selected, var(--oc-bg-hover)); box-shadow: inset 0 0 0 1px var(--oc-accent); }
.obj-row.hidden .thumb, .obj-row.hidden .ellipsis { opacity: 0.5; }
.thumb { width: 30px; height: 30px; flex: none; display: grid; place-items: center; border-radius: 4px; background: var(--oc-bg-base); }
.obj-tools { display: flex; opacity: 0; }
.obj-row:hover .obj-tools, .obj-row.on .obj-tools, .obj-row:focus-within .obj-tools { opacity: 1; }
.positions { display: flex; flex-wrap: wrap; gap: 2px; }
/* Square, like the add button beside them. */
.positions .pos { width: var(--oc-size-md); padding: 0; }
/* With several picked, the one shown is ringed. */
.positions .pos.lead { box-shadow: inset 0 0 0 1px var(--oc-accent); }
.positions .clash { color: var(--oc-fg-warning); }
.field { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
.lamp-row { display: flex; align-items: center; gap: var(--oc-space-2xs); }
.lamp-row > :not(.grow) { flex: none; }
.text-input { width: 100%; min-width: 0; height: var(--oc-size-md); padding: 0 var(--oc-space-sm); border: 0; border-radius: var(--oc-radius-control); background: var(--oc-bg-input); color: var(--oc-fg-default); }
.num-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: var(--oc-space-xs); }
.num-field { display: flex; align-items: center; gap: var(--oc-space-2xs); min-width: 0; }
.num-input { width: 100%; min-width: 0; height: var(--oc-size-md); padding: 0 var(--oc-space-xs); border: 0; border-radius: var(--oc-radius-control); background: var(--oc-bg-input); appearance: textfield; -moz-appearance: textfield; }
.num-input::-webkit-inner-spin-button, .num-input::-webkit-outer-spin-button { appearance: none; -webkit-appearance: none; margin: 0; }
.var-row { display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr) auto; align-items: center; gap: var(--oc-space-2xs); }
.var-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; color: var(--oc-fg-muted); }
.action-link.end { align-self: flex-end; }
.mat-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: var(--oc-space-2xs); }
.mat-input { width: 100%; min-width: 0; height: var(--oc-size-sm); padding: 0 var(--oc-space-2xs); border: 0; border-radius: 4px; background: var(--oc-bg-input); color: var(--oc-fg-default); font-size: var(--oc-text-xs); text-align: center; }
.mat-input[readonly] { background: var(--oc-bg-base); color: var(--oc-fg-muted); text-overflow: ellipsis; }
.exp-actions { display: flex; flex-direction: column; gap: var(--oc-space-sm); }
.exp-view { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
.stage { position: relative; flex: 1; min-width: 0; min-height: 0; overflow: hidden; }
.stage-bar { position: absolute; z-index: 1; top: var(--oc-space-md); left: var(--oc-space-lg); right: var(--oc-space-md); display: flex; align-items: center; gap: var(--oc-space-sm); pointer-events: none; }
.stage-bar > :not(.grow) { pointer-events: auto; }
.mode-seg { width: 148px; }
.toolbar { display: flex; align-items: center; gap: 2px; padding: 2px; border-radius: var(--oc-radius-control); background: var(--oc-bg-glass); backdrop-filter: blur(18px); }
.sep { width: 1px; height: 16px; margin: 0 var(--oc-space-2xs); background: var(--oc-border-muted); }
.stage-body { position: absolute; inset: 0; display: grid; place-items: center; }
/* Edit and preview cross-fade, the one arriving settling in from a touch larger. */
.stage-enter-active, .stage-leave-active { transition: opacity 280ms var(--oc-ease), transform 280ms var(--oc-ease); }
.stage-enter-from { opacity: 0; transform: scale(1.02); }
.stage-leave-to { opacity: 0; transform: scale(0.98); }
.bar-enter-active, .bar-leave-active { transition: opacity 120ms var(--oc-ease), transform 120ms var(--oc-ease); }
.bar-enter-from, .bar-leave-to { opacity: 0; transform: translateY(-2px); }
@media (prefers-reduced-motion: reduce) { .stage-enter-active, .stage-leave-active, .bar-enter-active, .bar-leave-active { transition: none; } }
.drop-scrim { position: absolute; inset: 0; display: grid; place-items: center; background: var(--oc-bg-glass); pointer-events: none; }
.hint { position: absolute; padding: var(--oc-space-2xs) var(--oc-space-md); border-radius: var(--oc-radius-pill); background: var(--oc-bg-glass); backdrop-filter: blur(18px); pointer-events: none; }
.float { position: absolute; right: var(--oc-space-md); bottom: var(--oc-space-sm); pointer-events: none; }
.float.left { right: auto; left: var(--oc-space-lg); }
.gestures { display: flex; gap: var(--oc-space-md); }
.gesture { display: inline-flex; align-items: center; gap: var(--oc-space-2xs); }
.legend { display: flex; align-items: flex-start; gap: var(--oc-space-sm); }
.legend-name { line-height: 8px; }
.scale { display: grid; grid-template-columns: repeat(3, 44px) 52px; row-gap: var(--oc-space-2xs); text-align: center; }
.ramp, .unusable { height: 8px; }
.ramp { grid-column: span 3; border-radius: var(--oc-radius-pill) 0 0 var(--oc-radius-pill); background: linear-gradient(90deg, rgba(144, 69, 249, 0.85), rgba(144, 69, 249, 0.08)), #2a2a31; }
.unusable { border-radius: 0 var(--oc-radius-pill) var(--oc-radius-pill) 0; background: repeating-linear-gradient(45deg, #8a8a92 0 1px, #1c1c22 1px 4px); }
.preview-stage { position: absolute; inset: 0; display: grid; place-items: center; cursor: grab; touch-action: none; }
.preview-stage.dragging { cursor: grabbing; }
.preview-disc { width: min(86%, 86vh - 108px); aspect-ratio: 1; will-change: transform; }
</style>
