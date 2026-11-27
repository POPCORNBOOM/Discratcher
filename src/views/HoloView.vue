<template>
  <div
    class="page holo"
    @dragover.prevent="onDragOver"
    @dragleave="onDragLeave"
    @drop.prevent="onDrop"
  >
    <aside class="side">
      <div class="flow fields">
        <h1>{{ t('holo.title') }}</h1>

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
        <section v-if="!scene.objects.length && !expViews.length" class="section empty-section">
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
        </section>

        <!-- Object list + selected object controls -->
        <template v-else-if="scene.objects.length">
          <!-- Where on the disc this position is, and how it is solved. -->
          <UiSection :icon="mdiCrosshairsGps" :title="t('holo.positions')">
            <template #actions><a v-if="scene.positions.length > 1" class="action-link" role="button" tabindex="0" @click="removePosition" @keydown.enter="removePosition">{{ t('holo.removePosition') }}</a></template>
            <div class="positions" role="tablist" :aria-label="t('holo.positions')">
              <UiButton v-for="(q, i) in scene.positions" :key="q.id" role="tab" :aria-selected="q.id === positionId" :variant="q.id === positionId ? 'soft' : 'ghost'" :class="{ clash: overlapping.includes(q.id) }" :data-tooltip="overlapping.includes(q.id) ? t('holo.overlap') : undefined" @click="goToPosition(q.id)">{{ i + 1 }}</UiButton>
              <UiButton icon-only variant="ghost" :icon="mdiPlus" :disabled="scene.positions.length >= MAX_POSITIONS" :aria-label="t('holo.addPosition')" :data-tooltip="t('holo.addPosition')" @click="addPosition" />
            </div>
            <UiSlider v-if="scene.positions.length > 1" :model-value="position.phiDeg" :label="t('holo.phi')" :min="0" :max="359" :step="1" :format="degInt" @pointerdown="beginControlEdit" @update:model-value="setPhi" />
            <UiSlider :model-value="position.cropDeg" :label="t('holo.crop')" :min="5" :max="90" :step="1" :format="cropWords" @pointerdown="beginControlEdit" @update:model-value="setCrop" />
            <UiSegmented :model-value="position.mode" :options="calcModeOptions" @update:model-value="setMode($event as HoloPositionMode)" />
            <UiSlider
              v-if="position.mode === 'real'"
              :model-value="position.parallax"
              :label="t('holo.parallax')"
              :min="1"
              :max="6"
              :step="0.1"
              :format="(x) => t('holo.times', { n: x.toFixed(1) })"
              :ends="[t('holo.parallaxLow'), t('holo.parallaxHigh')]"
              @pointerdown="beginControlEdit"
              @update:model-value="setParallax"
            />
            <template v-else>
              <UiSlider :model-value="tiltNow.deg" :label="t('holo.tiltRange')" :min="1" :max="MAX_TRANSFORM_TILT_DEG" :step="0.5" :format="(x) => t('holo.tiltRangeWords', { n: x })" :ends="[t('holo.tiltLow'), t('holo.tiltHigh')]" @pointerdown="beginControlEdit" @update:model-value="setTilt" />
              <a v-if="position.tiltDeg > 0" class="action-link end" role="button" tabindex="0" @click="setTilt(0)" @keydown.enter="setTilt(0)">{{ t('holo.tiltAuto', { n: tiltNow.autoDeg }) }}</a>
            </template>
            <div v-if="overlapping.length" class="notice warn">
              <UiIcon :path="mdiAlertOutline" />
              <div class="grow">{{ t('holo.overlap') }}</div>
            </div>
            <UiAdvanced :label="t('holo.colour')">
              <UiSlider
                :model-value="neutralNow"
                @pointerdown="beginControlEdit"
                @update:model-value="setNeutral"
                :label="t('holo.chroma')"
                :min="0"
                :max="3"
                :step="0.05"
                :ends="[t('holo.chromaLow'), t('holo.chromaHigh')]"
              />
            </UiAdvanced>
          </UiSection>

          <UiSection :icon="mdiCubeOutline" :title="t('holo.models')">
            <div class="obj-list" role="listbox" :aria-label="t('holo.models')">
              <div
                v-for="obj in positionObjects"
                :key="obj.id"
                class="obj-row"
                role="option"
                tabindex="0"
                :aria-selected="obj.id === selectedId"
                :class="{ on: obj.id === selectedId, hidden: !obj.visible }"
                @click="selectObject(obj.id)"
                @keydown.enter.prevent="selectObject(obj.id)"
                @keydown.space.prevent="selectObject(obj.id)"
              >
                <span class="thumb"><UiIcon :path="mdiCubeOutline" :size="16" class="muted" /></span>
                <span class="grow ellipsis" :data-tooltip="obj.name" data-tooltip-overflow>{{ obj.name }}</span>
                <span class="obj-tools">
                  <UiButton
                    icon-only
                    variant="ghost"
                    :icon="obj.visible ? mdiEyeOutline : mdiEyeOffOutline"
                    :aria-label="obj.visible ? t('holo.hide') : t('holo.show')"
                    :data-tooltip="obj.visible ? t('holo.hide') : t('holo.show')"
                    @click.stop="toggleVisibility(obj.id)"
                  />
                  <UiButton
                    icon-only
                    variant="ghost"
                    :icon="mdiTrashCanOutline"
                    :aria-label="t('holo.delete')"
                    :data-tooltip="t('holo.delete')"
                    @click.stop="deleteObject(obj.id)"
                  />
                </span>
              </div>
            </div>
            <UiButton variant="ghost" :icon="mdiPlus" :disabled="loadingMesh || !!s.job" @click="addModelFromPicker">{{ t('holo.addModel') }}</UiButton>

          <section v-if="selectedObj" class="section">
            <div class="title row">
              <span class="grow ellipsis">{{ selectedObj.name }}</span>
              <a class="action-link" role="button" tabindex="0" @click="resetSelected" @keydown.enter="resetSelected">{{ t('holo.reset') }}</a>
            </div>
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
                :model-value="selectedObj.scaleMm"
                :label="t('holo.size')"
                :min="6"
                :max="60"
                :step="1"
                :format="mm"
                @pointerdown="beginControlEdit"
                @update:model-value="setSelectedScale"
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
                <input
                  type="number"
                  class="num-input mono"
                  :value="selectedObj.scaleMm"
                  min="4"
                  max="120"
                  step="1"
                  @focus="beginControlEdit"
                  @change="setSelectedScaleInput"
                />
              </div>
            </template>
            <template v-if="position.mode === 'transform'">
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
            <UiAdvanced v-if="position.mode === 'transform'" :label="t('holo.motionMatrix')" :open="selectedObj.motion.preset === 'custom' || matrixOpen" @update:open="matrixOpen = $event">
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

          <!-- The lamp and where you look from, shared by every position. -->
          <UiSection :icon="mdiLightbulbOutline" :title="t('holo.lighting')">
            <template #actions><a class="action-link" role="button" tabindex="0" @click="setLampPreset('held')" @keydown.enter="setLampPreset('held')">{{ t('holo.reset') }}</a></template>
            <UiSelect :model-value="opts.lampPreset" :label="t('holo.lampPosition')" :options="lampPresetOptions" @update:model-value="setLampPreset($event as LampPreset)" />
            <UiAdvanced :label="t('holo.exactPlacement')">
              <UiSlider :model-value="opts.lampMm[0]" :label="t('holo.lampSide')" :min="-LAMP_REACH_MM" :max="LAMP_REACH_MM" :step="5" :format="lampSideWords" @update:model-value="(v) => setLampAxis(0, v)" />
              <UiSlider :model-value="opts.lampMm[1]" :label="t('holo.lampAway')" :min="-LAMP_REACH_MM" :max="LAMP_REACH_MM" :step="5" :format="lampAwayWords" @update:model-value="(v) => setLampAxis(1, v)" />
              <UiSlider v-if="opts.lampPreset !== 'ceiling'" :model-value="opts.lampMm[2]" :label="t('holo.lampUp')" :min="LAMP_MIN_UP_MM" :max="LAMP_MAX_UP_MM" :step="5" :format="cm" @update:model-value="(v) => setLampAxis(2, v)" />
              <UiSlider
                v-model="opts.distanceMm"
                :label="t('holo.distance')"
                :min="150"
                :max="600"
                :step="10"
                :format="mm"
              />
            </UiAdvanced>
          </UiSection>
        </template>

        <!-- Experimental tools -->
        <UiSection :icon="mdiFlaskOutline" :title="t('holo.expTools')" folded>
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
          <UiButton icon-only variant="ghost" :icon="mdiFolderOpenOutline" :aria-label="t('holo.openScene')" :data-tooltip="`${t('holo.openScene')} (${modKey}O)`" @click="openSceneFile" />
          <UiButton icon-only variant="ghost" :icon="mdiContentSaveOutline" :disabled="!scene.objects.length" :aria-label="t('holo.saveScene')" :data-tooltip="`${t('holo.saveScene')} (${modKey}S)`" @click="saveSceneFile" />
        </div>

        <div v-if="tab === 'edit'" class="toolbar" role="toolbar" :aria-label="t('holo.modeEdit')">
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
            :disabled="!selectedObj"
            :aria-label="t('holo.duplicate')"
            :data-tooltip="`${t('holo.duplicate')} (${modKey}D)`"
            @click="duplicateSelected"
          />
          <UiButton
            icon-only
            variant="ghost"
            :icon="mdiTrashCanOutline"
            :disabled="!selectedObj"
            :aria-label="t('holo.delete')"
            :data-tooltip="t('holo.delete')"
            @click="selectedId && deleteObject(selectedId)"
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
          <UiButton
            icon-only
            variant="ghost"
            :icon="mdiRestore"
            :aria-label="t('holo.resetCamera')"
            :data-tooltip="t('holo.resetCamera')"
            @click="resetPreviewLook"
          />
        </div>
      </header>

      <!-- Edit viewport -->
      <div v-show="tab === 'edit'" class="stage-body">
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
          :distance-mm="opts.distanceMm"
          :preview-t="previewT"
          @select="selectObject"
          @position="goToPosition"
          @transform-start="onTransformStart"
          @transform-change="onTransformChange"
          @transform-end="onTransformEnd"
        />
        <div v-if="dropHover" class="drop-scrim">
          <UiIcon :path="mdiCubeOutline" :size="28" />
          <span>{{ t('holo.dropHint') }}</span>
        </div>
        <div v-if="!scene.objects.length && !loadingMesh" class="hint">{{ t('holo.dropHint') }}</div>
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

      <!-- Preview: drag round the disc to turn it, move across it to tilt it -->
      <div v-show="tab === 'preview'" class="stage-body">
        <HoloPreview v-if="previewVisited" v-show="!expViews.length" v-model:turn="lookTurn" v-model:preview-t="previewT" :mode="position.mode" :active="tab === 'preview' && !expViews.length && hasPreviewSource" :weights="weights" :tables="tables" :inner-radius-mm="radii.inner" :outer-radius-mm="radii.outer" :lamp-mm="opts.lampMm" :distance-mm="opts.distanceMm" :tilt-deg="tiltNow.deg" />
        <div v-if="expViews.length" class="preview-stage" :class="{ dragging: previewDragging }" @pointerdown="onPreviewPointerDown" @pointermove="onPreviewPointerMove" @pointerup="onPreviewPointerUp" @pointercancel="onPreviewPointerUp">
          <canvas ref="previewCanvas" class="preview-disc" />
        </div>
        <div v-if="!hasPreviewSource" class="hint">{{ t('holo.previewEmpty') }}</div>
        <div v-if="previewBusy" class="float small muted">{{ t('holo.previewSolve') }}</div>
        <div v-if="hasPreviewSource" class="float left small muted">{{ expViews.length ? t('holo.previewDrag') : t('holo.previewTurn') }}</div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import {
  mdiAlertOctagonOutline,
  mdiAlertOutline,
  mdiClose,
  mdiContentCopy,
  mdiContentSaveOutline,
  mdiCrosshairsGps,
  mdiCubeOutline,
  mdiCursorMove,
  mdiEyeOffOutline,
  mdiEyeOutline,
  mdiFire,
  mdiFlaskOutline,
  mdiFolderOpenOutline,
  mdiGrid,
  mdiLightbulbOutline,
  mdiPlus,
  mdiRedo,
  mdiResize,
  mdiRestore,
  mdiRotateOrbit,
  mdiStarFourPointsOutline,
  mdiTrashCanOutline,
  mdiUndo,
} from '@mdi/js'
import { computed, onBeforeUnmount, onMounted, reactive, ref, shallowReactive, shallowRef, watch } from 'vue'
import { call, pickHoloScene, pickHoloSceneSave, pickImage, pickModel } from '../api'
import HoloPreview from '../components/HoloPreview.vue'
import HoloViewport3D, { type GizmoMode } from '../components/HoloViewport3D.vue'
import UiAdvanced from '../components/UiAdvanced.vue'
import UiButton from '../components/UiButton.vue'
import UiIcon from '../components/UiIcon.vue'
import UiSection from '../components/UiSection.vue'
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
  type HoloPositionMode,
  type HoloScene,
  type HoloSceneObject,
} from '../holo/scene'
import { t } from '../i18n'
import { notifyError, notifySuccess } from '../notices'
import { useProcess, type StepContext } from '../stores/process'
import { useStudio } from '../stores/studio'
import type { DiscGeometryPreset } from '../types'

type LightMode = 'flash' | 'lamp' | 'far'
type LampPreset = 'held' | 'ceiling' | 'custom'
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
const gizmoMode = ref<GizmoMode>('translate')
const snap = ref(false)
const scene = ref<HoloScene>(emptyScene())
const selectedId = ref<string | null>(null)
const MAX_POSITIONS = 8
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
const sceneFilePath = ref<string | null>(null)
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
  lampPreset: LampPreset
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
/** How a transform position is tilted, as `turntable::Tilt`. */
interface Tilt { id: string; deg: number; autoDeg: number; ok: boolean }
const tilts = ref<Tilt[]>([])
const tiltNow = computed<Tilt>(() => {
  const found = tilts.value.find((x) => x.id === position.value.id)
  const autoDeg = found?.autoDeg ?? DEFAULT_TRANSFORM_TILT_DEG
  const deg = position.value.tiltDeg > 0 ? position.value.tiltDeg : autoDeg
  return { id: position.value.id, deg, autoDeg, ok: found && found.deg === deg ? found.ok : true }
})
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
let previewDragStartX = 0
let previewDragStartVal = 0
let previewTimer = 0

// ---- Derived values ----
const isDvd = computed(() => !!s.preset && s.preset.media !== 'cd')
const ready = computed(() => isDvd.value && !s.job)
const visibleObjects = computed(() => scene.value.objects.filter((o) => o.visible))
const hasPreviewSource = computed(() => expViews.value.length > 0 || visibleObjects.value.length > 0)
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
  { value: 'custom', label: t('holo.lampCustom') },
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
    return position.value.mode === 'transform'
      ? `${degInt(lookTurn.value)}  ${deg(transformTiltDeg(tiltNow.value.deg, previewT.value))}`
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
      ...place,
    }
    scene.value.objects.push(obj)
    selectedId.value = obj.id
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
  selectedId.value = id
  const obj = scene.value.objects.find((o) => o.id === id)
  if (obj && obj.positionId !== positionId.value) goToPosition(obj.positionId)
}

/** Turn to a position. A model selected elsewhere is let go; nothing is picked for you. */
function goToPosition(id: string) {
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
  scene.value.objects = scene.value.objects.filter((o) => o.positionId !== id)
  scene.value.positions = scene.value.positions.filter((q) => q.id !== id)
  goToPosition(scene.value.positions[0].id)
}

function setPhi(deg: number) {
  position.value.phiDeg = wrapDeg(deg)
  lookTurn.value = position.value.phiDeg
}

function setCrop(deg: number) {
  position.value.cropDeg = Math.min(90, Math.max(5, Math.round(deg)))
}

function setMode(mode: HoloPositionMode) {
  if (position.value.mode === mode) return
  pushHistory()
  position.value.mode = mode
}

/** The position's neutral colour, or its mode's default (`turntable::default_neutral`). */
const neutralNow = computed(() => position.value.neutral ?? (position.value.mode === 'real' ? opts.chromaWeight : 0))

function setNeutral(v: number) {
  position.value.neutral = Math.round(v * 100) / 100
}

function setTilt(deg: number) {
  position.value.tiltDeg = deg > 0 ? Math.min(MAX_TRANSFORM_TILT_DEG, Math.max(1, Math.round(deg * 2) / 2)) : 0
}

function setParallax(v: number) {
  position.value.parallax = Math.min(6, Math.max(1, round1(v)))
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
  if (selectedId.value === id) {
    selectedId.value = scene.value.objects[Math.min(idx, scene.value.objects.length - 1)]?.id ?? null
  }
}

function duplicateSelected() {
  const src = selectedObj.value
  if (!src) return
  pushHistory()
  const copy: HoloSceneObject = {
    ...src,
    id: makeObjectId(),
    positionMm: [round1(src.positionMm[0] + 6), round1(src.positionMm[1]), round1(src.positionMm[2])],
    rotationDeg: [...src.rotationDeg] as Vec3,
    style: { ...src.style },
    motion: cloneMotion(src.motion),
  }
  scene.value.objects.push(copy)
  selectedId.value = copy.id
}

function resetSelected() {
  const obj = selectedObj.value
  if (!obj) return
  pushHistory()
  const place = defaultObjectPlacement(s.preset, 0)
  obj.positionMm = [...place.positionMm] as Vec3
  obj.rotationDeg = [...place.rotationDeg] as Vec3
  obj.scaleMm = place.scaleMm
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

function setSelectedScale(scaleMm: number) {
  const obj = selectedObj.value
  if (!obj) return
  obj.scaleMm = round1(scaleMm)
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

function setSelectedScaleInput(e: Event) {
  const obj = selectedObj.value
  if (!obj) return
  const v = Number((e.target as HTMLInputElement).value)
  if (!Number.isFinite(v)) return
  obj.scaleMm = Math.max(4, Math.min(120, round1(v)))
}

function onTransformStart(_id: string) {
  pushHistory()
}

function onTransformChange(payload: { id: string; positionMm: Vec3; rotationDeg: Vec3; scaleMm: number }) {
  if (payload.id === LAMP_ID) {
    opts.lampMm = payload.positionMm
    opts.lampPreset = 'custom'
    return
  }
  const obj = scene.value.objects.find((o) => o.id === payload.id)
  if (!obj) return
  obj.positionMm = payload.positionMm
  obj.rotationDeg = payload.rotationDeg
  obj.scaleMm = payload.scaleMm
}

function onTransformEnd(_id: string) {
  saveLastScene(scene.value)
}

// ---- Lamp & optical controls ----
function setLampPreset(preset: LampPreset) {
  opts.lampPreset = preset
  if (preset === 'held') opts.lampMm = [...LAMP_HELD_MM]
  else if (preset === 'ceiling') opts.lampMm = [...LAMP_CEILING_MM]
}

function setLampAxis(idx: number, v: number) {
  opts.lampMm[idx] = Math.round(v)
  opts.lampPreset = 'custom'
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
    opts.lampPreset = String(at) === String(LAMP_HELD_MM) ? 'held' : String(at) === String(LAMP_CEILING_MM) ? 'ceiling' : 'custom'
  }
})

// ---- Save / Open scene JSON project file ----
async function saveSceneFile() {
  if (!scene.value.objects.length) return
  const defaultName = sceneFilePath.value
    ? fileName(sceneFilePath.value)
    : `${scene.value.objects[0].name || 'scene'}.holo.json`
  const path = await pickHoloSceneSave(defaultName)
  if (!path) return
  try {
    const payload: HoloScene = cloneScene(scene.value)
    await call('holo_save_scene', { path, scene: payload })
    sceneFilePath.value = path
    saveLastScene(payload)
    notifySuccess(t('holo.saved', { name: fileName(path) }))
  } catch (e) {
    notifyError(String(e))
  }
}

async function applyLoadedScene(loaded: HoloScene, sourcePath: string | null) {
  const missing = await ensureMeshesLoaded(loaded.objects)
  const missingSet = new Set(missing)
  const kept = loaded.objects.filter((o) => !missingSet.has(o.path))
  missingFiles.value = missing.map((p) => fileName(p))
  scene.value = { version: SCENE_VERSION, positions: loaded.positions, objects: kept, ...(loaded.lampMm ? { lampMm: loaded.lampMm } : {}) }
  positionId.value = loaded.positions[0].id
  lookTurn.value = loaded.positions[0].phiDeg
  selectedId.value = kept.find((o) => o.positionId === positionId.value)?.id ?? null
  sceneFilePath.value = sourcePath
  expViews.value = []
  undoStack.value = []
  redoStack.value = []
}

async function openSceneFile() {
  const path = await pickHoloScene()
  if (!path) return
  try {
    const raw = await call<unknown>('holo_open_scene', { path })
    const validated = validateScene(raw)
    await applyLoadedScene(validated, path)
    notifySuccess(t('holo.opened', { name: fileName(path) }))
  } catch (e) {
    notifyError(String(e))
  }
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
        const solved = await call<{ overlapping: string[]; tilts?: Tilt[] }>('holo_solve_scene', { ...request, options })
        if (version !== sceneVersion) break
        overlapping.value = solved.overlapping
        tilts.value = solved.tilts ?? []
        weights.value = await call<ArrayBuffer>('holo_scene_weights', { ...request, options })
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

function resetPreviewLook() {
  lookHeadX.value = 0
  lookTurn.value = position.value.phiDeg
  lookTilt.value = DEFAULT_TILT_DEG
  lookDir.value = DEFAULT_DIR_DEG
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
    : sceneSnapshot.positions.find((q) => sceneSnapshot.objects.some((o) => o.visible && o.positionId === q.id)) ?? position.value

  p.openBurn({
    title: t('holo.burnTitle'),
    note: usingExp
      ? t('holo.burnNote', { n: expSnapshot.length })
      : t('holo.burnNoteModels', { n: visibleObjects.value.length }),
    fileName: 'holo.iso',
    prepareLabel: t('holo.compose'),
    viewingGuide: usingExp ? 'real' : guidePos.mode,
    viewingTiltDeg: guidePos.tiltDeg > 0 ? guidePos.tiltDeg : (tilts.value.find((x) => x.id === guidePos.id)?.autoDeg ?? DEFAULT_TRANSFORM_TILT_DEG),
    prepare: async (path: string, ctx: StepContext) => {
      if (usingExp) {
        await call('holo_build_design_iso', { preset, input: { views: expSnapshot, chromaWeight, exposure, grid }, outputIsoPath: path })
      } else {
        ctx.report(5, t('holo.previewSolve'))
        await call('holo_build_scene_iso', { preset, scene: sceneSnapshot, options: sceneOpts, outputIsoPath: path })
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
    void saveSceneFile()
    return
  }
  if (mod && key === 'o') {
    e.preventDefault()
    void openSceneFile()
    return
  }
  if (mod || e.altKey) return

  if (tab.value === 'edit') {
    if (key === 'w') { gizmoMode.value = 'translate'; e.preventDefault() }
    else if (key === 'e') { gizmoMode.value = 'rotate'; e.preventDefault() }
    else if (key === 'r') { gizmoMode.value = 'scale'; e.preventDefault() }
    else if (key === 'f' && selectedObj.value) { viewportRef.value?.focusSelected(); e.preventDefault() }
    else if ((e.key === 'Delete' || e.key === 'Backspace') && selectedId.value) {
      deleteObject(selectedId.value)
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

watch([expViews, expOpts], () => {
  if (tab.value === 'preview') requestFrame()
}, { deep: true })

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
  if (restored && restored.objects.length) {
    await applyLoadedScene(restored, null)
  }
})

onBeforeUnmount(() => {
  window.removeEventListener('keydown', onKeyDown)
  clearTimeout(previewTimer)
  unlistenTauriDrop?.()
})
</script>

<style scoped>
.side { width: calc(var(--oc-page-gutter) + 348px); flex: none; min-height: 0; display: flex; flex-direction: column; gap: var(--oc-space-md); padding: 64px 0 var(--oc-space-lg) var(--oc-page-gutter); }
.fields { width: auto; max-width: none; min-height: 0; overflow-y: auto; scrollbar-gutter: stable; }
.fields .section + .section { margin-top: var(--oc-space-sm); }
.actions { flex: none; display: flex; gap: var(--oc-space-sm); }
.drop { width: 100%; height: 148px; padding: 0; border: 1px dashed var(--oc-border-strong); border-radius: var(--oc-radius-control); background: var(--oc-bg-base); display: grid; place-items: center; }
.drop:hover:not(:disabled), .drop.over { background-image: linear-gradient(var(--oc-bg-hover), var(--oc-bg-hover)); }
.drop:disabled { opacity: var(--oc-opacity-disabled); }
.drop-inner { display: flex; flex-direction: column; align-items: center; gap: var(--oc-space-2xs); }
.obj-list { display: flex; flex-direction: column; gap: 2px; }
.obj-row { display: flex; align-items: center; gap: var(--oc-space-sm); height: 40px; padding: 0 var(--oc-space-2xs) 0 var(--oc-space-xs); border-radius: var(--oc-radius-control); cursor: pointer; user-select: none; }
.obj-row:hover { background: var(--oc-bg-hover); }
.obj-row.on { background: var(--oc-bg-selected, var(--oc-bg-hover)); box-shadow: inset 0 0 0 1px var(--oc-accent); }
.obj-row.hidden .thumb, .obj-row.hidden .ellipsis { opacity: 0.5; }
.thumb { width: 30px; height: 30px; flex: none; display: grid; place-items: center; border-radius: 4px; background: var(--oc-bg-base); }
.obj-tools { display: flex; opacity: 0; }
.obj-row:hover .obj-tools, .obj-row.on .obj-tools, .obj-row:focus-within .obj-tools { opacity: 1; }
.action-link { font-size: var(--oc-text-xs); color: var(--oc-fg-muted); text-decoration: none; cursor: pointer; user-select: none; }
.action-link:hover { color: var(--oc-fg-default); }
.positions { display: flex; flex-wrap: wrap; gap: 2px; }
.positions .clash { color: var(--oc-fg-warning); }
.field { display: flex; flex-direction: column; gap: var(--oc-space-2xs); }
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
.drop-scrim { position: absolute; inset: 0; display: grid; place-items: center; background: var(--oc-bg-glass); pointer-events: none; }
.hint { position: absolute; padding: var(--oc-space-2xs) var(--oc-space-md); border-radius: var(--oc-radius-pill); background: var(--oc-bg-glass); backdrop-filter: blur(18px); pointer-events: none; }
.float { position: absolute; right: var(--oc-space-md); bottom: var(--oc-space-sm); pointer-events: none; }
.float.left { right: auto; left: var(--oc-space-lg); }
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
