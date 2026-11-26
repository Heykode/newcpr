<script setup lang="ts">
import type { ProfileOptions, ProfilePreview, ProfileSelection } from './profile'
import type { ProfileEditorProps } from '@/components/request-profile/useProfileEditor'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import HelpPopover from '@/components/HelpPopover.vue'
import ProfileEditorFrame from '@/components/request-profile/ProfileEditorFrame.vue'
import { useProfileEditor } from '@/components/request-profile/useProfileEditor'

const props = defineProps<ProfileEditorProps<ProfileSelection>>()
const model = defineModel<ProfileSelection | null | undefined>({ required: true })
const { t } = useI18n({ useScope: 'global' })
const { options, preview, effective, source, frame, loading } = useProfileEditor<ProfileSelection, ProfileOptions, ProfilePreview>(
  'antigravity',
  model,
  props,
  selection => selection.versionMode === 'fixed' && !selection.clientVersion,
)
const platforms = { macos: 'MacOS', linux: 'Linux', windows: 'Windows' }
const presetOptions = computed(() => (options.value?.presets ?? []).map(({ configuration }) => ({
  label: `${platforms[configuration.platform]} · ${configuration.client === 'desktop'
    ? 'Desktop'
    : 'CLI'}`,
  value: `${configuration.platform}-${configuration.client}`,
})))
const currentPreset = computed(() => options.value?.presets.find(({ configuration }) =>
  configuration.platform === effective.value?.platform && configuration.client === effective.value?.client))
const selectedPreset = computed({
  get: () => effective.value
    ? `${effective.value.platform}-${effective.value.client}`
    : undefined,
  set: (value: string) => {
    const preset = options.value?.presets.find(({ configuration }) => `${configuration.platform}-${configuration.client}` === value)
    if (preset) {
      model.value = {
        ...preset.configuration,
        systemPrompt: effective.value?.systemPrompt ?? null,
        versionMode: preset.automaticAvailable ? 'latest' : 'fixed',
      }
    }
  },
})
const versionMode = computed({
  get: () => effective.value?.versionMode ?? 'latest',
  set: (value: string) => {
    if (effective.value) {
      model.value = {
        ...effective.value,
        versionMode: value === 'fixed'
          ? 'fixed'
          : 'latest',
        clientVersion: value === 'fixed'
          ? preview.value?.clientVersion ?? null
          : null,
      }
    }
  },
})

function updateVersion(value: string) {
  if (effective.value)
    model.value = { ...effective.value, clientVersion: value.trim() || null }
}

const systemPrompt = computed({
  get: () => effective.value?.systemPrompt ?? options.value?.defaultSystemPrompt ?? '',
  set: (value: string) => updateSystemPrompt(value),
})
function updateSystemPrompt(value: string | null) {
  if (effective.value)
    model.value = { ...effective.value, systemPrompt: value }
}
</script>

<template>
  <ProfileEditorFrame v-model="source" v-bind="frame">
    <template #source-extra>
      <slot name="source-extra" />
    </template>
    <div class="grid gap-4 sm:grid-cols-2">
      <BaseFormItem :label="t('settings.clientProfile.clientDefault')">
        <BaseSelect v-model="selectedPreset" class="w-full" :options="presetOptions" :disabled="disabled || loading || !presetOptions.length" />
      </BaseFormItem>
      <BaseFormItem :label="t('settings.clientProfile.versionStrategy')">
        <BaseSelect
          v-model="versionMode"
          class="w-full"
          :disabled="disabled || loading"
          :options="[
            { label: t('settings.clientProfile.followLatestVersion'), value: 'latest', disabled: !currentPreset?.automaticAvailable },
            { label: t('settings.clientProfile.customVersion'), value: 'fixed' },
          ]"
        />
      </BaseFormItem>
    </div>
    <p v-if="currentPreset?.reason" class="m-0 text-cp-sm text-cp-text-secondary">
      {{ currentPreset.reason }}
    </p>
    <div v-if="effective?.versionMode === 'fixed'" class="grid gap-4 sm:grid-cols-2">
      <BaseFormItem :label="t('settings.clientProfile.clientVersion')" required>
        <BaseInput
          :model-value="effective?.clientVersion ?? ''"
          :disabled="disabled || loading"
          :placeholder="t('settings.clientProfile.forExample')"
          @update:model-value="updateVersion($event)"
        />
      </BaseFormItem>
    </div>
    <BaseFormItem :label="t('settings.clientProfile.systemPrompt')">
      <template #label-extra>
        <HelpPopover
          :label="t('settings.clientProfile.systemPrompt')"
          :content="t('settings.clientProfile.systemPromptHint', { source: options?.systemPromptSource ?? '' })"
        />
      </template>
      <div class="grid gap-2">
        <BaseTextarea
          v-model="systemPrompt"
          :aria-label="t('settings.clientProfile.systemPrompt')"
          :disabled="disabled || loading"
          :rows="5"
          resize="vertical"
        />
        <BaseButton variant="link" size="sm" class="justify-self-end" :disabled="disabled || loading || effective?.systemPrompt == null" @click="updateSystemPrompt(null)">
          {{ t('settings.clientProfile.restoreSystemPrompt') }}
        </BaseButton>
      </div>
    </BaseFormItem>
  </ProfileEditorFrame>
</template>
