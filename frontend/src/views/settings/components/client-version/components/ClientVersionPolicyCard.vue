<script setup lang="ts">
import { CircleHelp, MonitorUp, TerminalSquare } from '@lucide/vue'
import BaseCard from '@/components/base/BaseCard.vue'

import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import { t } from '@/i18n'

defineOptions({ name: 'ClientVersionPolicyCard' })

withDefaults(
  defineProps<{
    loading?: boolean
    desktopError?: string
    cliError?: string
  }>(),
  {
    loading: false,
    desktopError: '',
    cliError: '',
  },
)

const emit = defineEmits<{
  help: []
}>()

const minCodexDesktopVersion = defineModel<string>('minCodexDesktopVersion', { required: true })
const minCodexCliVersion = defineModel<string>('minCodexCliVersion', { required: true })
</script>

<template>
  <BaseCard
    description="低于最低版本的 Codex 客户端将无法发起请求，留空表示不限制"
  >
    <template #title>
      <span class="inline-flex items-center gap-1.5">
        <span>{{ t('settings.clientVersion.clientVersionRestrictions') }}</span>
        <BaseIconButton
          :label="t('settings.clientVersion.openGuide')"
          size="sm"
          class="-my-1"
          @click="emit('help')"
        >
          <CircleHelp class="size-3.5" />
        </BaseIconButton>
      </span>
    </template>

    <BaseForm class="max-w-6xl sm:grid-cols-2">
      <BaseFormItem
        :label="t('settings.clientVersion.codexDesktopMinimumVersion')"
        description="只检查 Desktop 应用版本，例如 26.825.51511"
        :error="desktopError"
      >
        <BaseInput
          v-model="minCodexDesktopVersion"
          :aria-label="t('settings.clientVersion.codexDesktopMinimumVersion')"
          autocomplete="off"
          spellcheck="false"
          placeholder="不限制"
          :disabled="loading"
        >
          <template #prefix>
            <MonitorUp class="size-4" />
          </template>
        </BaseInput>
      </BaseFormItem>

      <BaseFormItem
        :label="t('settings.clientVersion.codexCliMinimumVersion')"
        description="只检查独立 CLI 版本，例如 0.152.0"
        :error="cliError"
      >
        <BaseInput
          v-model="minCodexCliVersion"
          :aria-label="t('settings.clientVersion.codexCliMinimumVersion')"
          autocomplete="off"
          spellcheck="false"
          placeholder="不限制"
          :disabled="loading"
        >
          <template #prefix>
            <TerminalSquare class="size-4" />
          </template>
        </BaseInput>
      </BaseFormItem>
    </BaseForm>
  </BaseCard>
</template>
