<script setup lang="ts">
import type { ClientProfileConfiguration, OutboundUserAgentSettings, PresetProfile } from '@/api/modules/outbound-user-agent'
import { computed } from 'vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'

const props = defineProps<{ settings: OutboundUserAgentSettings | null, preview: OutboundUserAgentSettings | null, disabled: boolean }>()
const model = defineModel<ClientProfileConfiguration>({ required: true })
const preset = computed(() => model.value.mode === 'custom' ? null : model.value)
const exact = computed(() => model.value.mode === 'custom' ? model.value : null)
const currentPreview = computed(() => props.preview?.profilePreview ?? (JSON.stringify(props.settings?.configuration) === JSON.stringify(model.value) ? props.settings?.profilePreview : null))
const selected = computed(() => props.settings?.profileOptions?.presets.find(row => row.configuration.client === preset.value?.client && row.configuration.platform === preset.value?.platform))
function change(fields: Partial<PresetProfile>) {
  if (preset.value)
    model.value = { ...preset.value, ...fields }
}
function target(client: 'desktop' | 'cli', platform: 'macos' | 'linux' | 'windows') {
  const row = props.settings?.profileOptions?.presets.find(row => row.configuration.client === client && row.configuration.platform === platform)
  if (row)
    model.value = { ...row.configuration, ...row.defaults, osVersion: preset.value?.platform === platform ? preset.value.osVersion || row.defaults.osVersion : row.defaults.osVersion }
}
function automatic(enabled: boolean) {
  if (!preset.value)
    return
  if (enabled) {
    change({ versionMode: 'latest', codexVersion: null, desktopVersion: null, desktopBuild: null })
  }
  else {
    const value = currentPreview.value
    change({ versionMode: 'fixed', versionLag: null, codexVersion: value?.codexVersion ?? '', desktopVersion: preset.value.client === 'desktop' ? value?.desktopVersion ?? '' : null, desktopBuild: preset.value.client === 'desktop' ? value?.desktopBuild ?? '' : null })
  }
}
</script>

<template>
  <div class="mt-4 grid min-w-0 gap-3 sm:grid-cols-2">
    <template v-if="preset">
      <BaseFormItem label="客户端">
        <BaseSelect aria-label="客户端" :model-value="preset.client" :disabled="disabled" :options="[{ label: 'Codex Desktop', value: 'desktop' }, { label: 'Codex CLI', value: 'cli' }]" @update:model-value="value => target(value as 'desktop' | 'cli', preset!.platform)" />
      </BaseFormItem>
      <BaseFormItem label="平台">
        <BaseSelect aria-label="平台" :model-value="preset.platform" :disabled="disabled" :options="[{ label: 'macOS', value: 'macos' }, { label: 'Linux', value: 'linux' }, { label: 'Windows', value: 'windows' }]" @update:model-value="value => target(preset!.client, value as 'macos' | 'linux' | 'windows')" />
      </BaseFormItem>
      <BaseFormItem label="架构">
        <BaseSelect aria-label="架构" :model-value="preset.arch ?? selected?.defaults.arch ?? ''" :disabled="disabled" :options="[{ label: 'arm64', value: 'arm64' }, { label: 'x86_64', value: 'x86_64' }]" @update:model-value="value => change({ arch: value })" />
      </BaseFormItem>
      <BaseFormItem v-if="preset.client === 'cli'" label="CLI 入口">
        <BaseSelect aria-label="CLI 入口" :model-value="preset.cliEntry ?? ''" :disabled="disabled" :options="[{ label: '默认', value: '' }, { label: 'TUI', value: 'tui' }, { label: 'Exec', value: 'exec' }]" @update:model-value="value => change({ cliEntry: value ? value as 'tui' | 'exec' : null, originator: null })" />
      </BaseFormItem>
      <BaseFormItem label="系统版本">
        <BaseInput aria-label="系统版本" :model-value="preset.osVersion ?? selected?.defaults.osVersion ?? ''" :disabled="disabled" maxlength="128" @update:model-value="value => change({ osVersion: value || null })" />
      </BaseFormItem>
      <BaseFormItem label="Codex 客户端版本号">
        <BaseInput aria-label="Codex 客户端版本号" :model-value="preset.versionMode === 'latest' ? currentPreview?.codexVersion ?? '' : preset.codexVersion ?? ''" :disabled="disabled || preset.versionMode === 'latest'" maxlength="64" @update:model-value="value => change({ codexVersion: value })" />
      </BaseFormItem>
      <BaseCheckbox class="sm:col-span-2" :model-value="preset.versionMode === 'latest'" :disabled="disabled" label="自动同步客户端版本" show-label @update:model-value="automatic" />
      <BaseFormItem v-if="preset.versionMode === 'latest'" label="版本滞后">
        <BaseSelect aria-label="版本滞后" :disabled="disabled" :model-value="String(preset.versionLag ?? 0)" :options="Array.from({ length: (settings?.profileOptions?.maxVersionLag ?? 10) + 1 }, (_, index) => ({ label: index ? `${index} 个版本` : '最新', value: String(index) }))" @update:model-value="value => change({ versionLag: Number(value) || null })" />
      </BaseFormItem>
      <template v-if="preset.client === 'desktop' && preset.versionMode === 'fixed'">
        <BaseFormItem label="Desktop 应用版本">
          <BaseInput aria-label="Desktop 应用版本" :model-value="preset.desktopVersion ?? ''" :disabled="disabled" @update:model-value="value => change({ desktopVersion: value })" />
        </BaseFormItem>
        <BaseFormItem label="Desktop 构建号">
          <BaseInput aria-label="Desktop 构建号" :model-value="preset.desktopBuild ?? ''" :disabled="disabled" @update:model-value="value => change({ desktopBuild: value })" />
        </BaseFormItem>
      </template>
      <BaseFormItem label="Originator">
        <BaseInput aria-label="Originator" :model-value="preset.originator ?? ''" :disabled="disabled" @update:model-value="value => change({ originator: value || null })" />
      </BaseFormItem>
      <BaseFormItem label="终端">
        <BaseInput aria-label="终端" :model-value="preset.terminal ?? ''" :disabled="disabled" @update:model-value="value => change({ terminal: value || null })" />
      </BaseFormItem>
    </template>
    <template v-else-if="exact">
      <BaseFormItem label="完整 UA" class="sm:col-span-2">
        <BaseTextarea aria-label="完整 UA" :model-value="exact.userAgent" :disabled="disabled" :maxlength="4096" :rows="3" @update:model-value="value => model = { ...exact!, userAgent: value }" />
      </BaseFormItem>
      <BaseFormItem label="Originator">
        <BaseInput aria-label="Originator" :model-value="exact.originator ?? ''" :disabled="disabled" @update:model-value="value => model = { ...exact!, originator: value || null }" />
      </BaseFormItem>
      <BaseFormItem label="Codex 客户端版本号">
        <BaseInput aria-label="Codex 客户端版本号" :model-value="exact.codexVersion ?? ''" :disabled="disabled" @update:model-value="value => model = { ...exact!, codexVersion: value || null }" />
      </BaseFormItem>
    </template>
    <p v-if="currentPreview" class="m-0 break-all text-xs text-cp-text-secondary sm:col-span-2">
      {{ currentPreview.versionSource === 'official' ? '官方发布' : '固定自定义' }} · {{ currentPreview.checkedAt ?? currentPreview.verifiedAt ?? '未核验' }}
      <span v-if="currentPreview.error"> · {{ currentPreview.error }}</span>
    </p>
  </div>
</template>
