<script setup lang="ts">
import { Check, Eye, Pencil, RotateCcw, Save, Search, Undo2 } from '@lucide/vue'
import { computed, shallowRef } from 'vue'
import { useI18n } from 'vue-i18n'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'

import BaseScrollbar from '@/components/base/BaseScrollbar.vue'

import BaseSegmented from '@/components/base/BaseSegmented.vue'

import { toast } from '@/components/base/BaseToast'

import { useThemeEditor } from '../composables/useThemeEditor'
import ThemeComponentTokenPanel from './ThemeComponentTokenPanel.vue'
import ThemeEditorPreview from './ThemeEditorPreview.vue'
import ThemeGlobalTokenPanel from './ThemeGlobalTokenPanel.vue'

const { t } = useI18n({ useScope: 'global' })
const mobilePreviewOpen = shallowRef(false)

const {
  draft,
  scope,
  globalCategory,
  preview,
  previewTheme,
  component,
  query,
  resolvedPreview,
  previewStyle,
  modificationCount,
  dirty,
  selectMaterial,
  applyProfile,
  resetPrimaryColor,
  setPrimaryColor,
  setMode,
  setSeedColor,
  setSeedNumber,
  resetSeed,
  setComponentNumber,
  resetComponentNumber,
  setTokenOverride,
  resetTokenOverride,
  resetDraft,
  restoreDefaults,
  save,
} = useThemeEditor()

const scopeOptions = computed(() => ([
  {
    label: t('appearance.settings.global'),
    value: 'global',
  },
  {
    label: t('appearance.settings.components'),
    value: 'component',
  },
]))

function saveTheme(event: MouseEvent) {
  if (!save(event)) {
    toast.error(t('appearance.settings.invalidTheme'))
    return
  }
  toast.success(t('appearance.settings.themeSavedAndApplied'))
}
</script>

<template>
  <div class="@container flex min-h-0 min-w-0 w-full flex-col">
    <BasePageHeader :title="t('navigation.appearance')">
      <template #description>
        <span class="flex flex-wrap items-center gap-x-2 gap-y-1">
          <span>{{ t('appearance.settings.description') }}</span>
          <span
            class="inline-flex items-center gap-1.5 text-cp-sm font-heavy"
            :class="dirty ? 'text-cp-primary-text' : 'text-cp-text-tertiary'"
          >
            <span class="size-1.5 rounded-full bg-current" />
            {{ dirty ? t('appearance.settings.pendingChanges', { count: modificationCount }) : t('appearance.settings.synced') }}
          </span>
        </span>
      </template>

      <template #actions>
        <div class="flex max-w-[100cqw] flex-wrap items-center justify-end gap-2">
          <BaseButton size="sm" class="md:hidden" @click="mobilePreviewOpen = !mobilePreviewOpen">
            <template #icon>
              <Pencil v-if="mobilePreviewOpen" class="size-3.5" />
              <Eye v-else class="size-3.5" />
            </template>
            {{ mobilePreviewOpen ? t('appearance.preview.edit') : t('appearance.preview.open') }}
          </BaseButton>
          <BaseButton size="sm" variant="ghost" :disabled="!dirty" @click="resetDraft">
            <template #icon>
              <Undo2 class="size-3.5" />
            </template>
            {{ t('appearance.settings.undoDraft') }}
          </BaseButton>
          <BaseButton size="sm" @click="restoreDefaults">
            <template #icon>
              <RotateCcw class="size-3.5" />
            </template>
            {{ t('appearance.settings.restoreDefault') }}
          </BaseButton>
          <BaseButton size="sm" variant="primary" :disabled="!dirty" @click="saveTheme">
            <template #icon>
              <Save v-if="dirty" class="size-3.5" />
              <Check v-else class="size-3.5" />
            </template>
            {{ dirty ? t('appearance.settings.saveAndApply') : t('appearance.settings.saved') }}
          </BaseButton>
        </div>
      </template>
    </BasePageHeader>

    <section
      class="theme-workbench isolate mt-5 overflow-hidden xl:flex xl:min-h-0 xl:flex-1 xl:flex-col"
      :aria-label="t('appearance.settings.themeEditor')"
    >
      <div
        class="theme-workbench-body grid min-h-180 gap-3 xl:min-h-0 xl:flex-1 xl:overflow-hidden xl:grid-cols-[370px_minmax(0,1fr)]"
      >
        <BaseCard
          as="aside"
          padding="none"
          class="h-full min-h-0 grid-rows-[auto_minmax(0,1fr)] gap-3 py-3 md:grid"
          :class="mobilePreviewOpen ? 'hidden' : 'grid'"
          :aria-label="t('appearance.settings.themeTokenEditingPanel')"
        >
          <div class="grid gap-3 px-3">
            <BaseSegmented
              v-model="scope"
              :label="t('appearance.settings.themeEditingLevel')"
              :options="scopeOptions"
            />
            <BaseInput
              v-model="query"
              :aria-label="t('appearance.settings.searchToken')"
              :placeholder="t('appearance.settings.searchPlaceholder')"
            >
              <template #prefix>
                <Search class="size-4" />
              </template>
            </BaseInput>
          </div>

          <BaseScrollbar class="h-full min-h-0">
            <div class="px-3">
              <ThemeGlobalTokenPanel
                v-if="scope === 'global'"
                v-model:category="globalCategory"
                :draft="draft"
                :resolved="resolvedPreview"
                :query="query"
                @mode="setMode"
                @material="selectMaterial"
                @profile="applyProfile"
                @reset-primary="resetPrimaryColor"
                @primary="setPrimaryColor"
                @seed-color="setSeedColor"
                @seed-number="setSeedNumber"
                @reset-seed="resetSeed"
              />
              <ThemeComponentTokenPanel
                v-else
                v-model="component"
                :draft="draft"
                :resolved="resolvedPreview"
                :query="query"
                @change="setTokenOverride"
                @reset="resetTokenOverride"
                @component-number="setComponentNumber"
                @reset-component-number="resetComponentNumber"
              />
            </div>
          </BaseScrollbar>
        </BaseCard>

        <div class="min-h-0 md:block" :class="mobilePreviewOpen ? 'block' : 'hidden'">
          <ThemeEditorPreview v-model="preview" class="h-full" :theme="previewTheme" :style="previewStyle" />
        </div>
      </div>
    </section>
  </div>
</template>
