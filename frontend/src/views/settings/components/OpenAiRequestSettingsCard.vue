<script setup lang="ts">
import type { OpenAiRequestSettings, OpenAiWebSocketPoolSettings } from '@/api/modules/settings'
import { computed } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import HelpPopover from '@/components/HelpPopover.vue'
import { t } from '@/i18n'
import RequestCountryCard from './RequestCountryCard.vue'

defineProps<{ disabled: boolean }>()
const settings = defineModel<OpenAiRequestSettings>({ required: true })
const pool = defineModel<OpenAiWebSocketPoolSettings>('pool', { required: true })
function optionalHeader(key: 'residency' | 'accountRoutingOverride') {
  return computed({
    get: () => settings.value[key] ?? '',
    set: (value: string) => { settings.value[key] = value || null },
  })
}
const residency = optionalHeader('residency')
const routing = optionalHeader('accountRoutingOverride')
const poolKeys = ['maxAgeMs', 'fastPathBudgetMs', 'idleTimeoutMs', 'streamIdleTimeoutMs'] as const
const fields = poolKeys.map(key => ({
  key,
  value: computed({ get: () => String(pool.value[key]), set: (value: string) => { pool.value[key] = value.trim() ? Number(value) : Number.NaN } }),
}))
</script>

<template>
  <BaseCard :title="t('settings.requestOptions.title')" :description="t('settings.requestOptions.description')">
    <div class="grid min-w-0 gap-4 sm:grid-cols-2">
      <BaseFormItem :label="t('settings.requestOptions.residency')" :description="t('settings.requestOptions.residencyHelp')">
        <BaseInput v-model="residency" :aria-label="t('settings.requestOptions.residency')" placeholder="us" clearable :disabled="disabled" />
      </BaseFormItem>
      <BaseFormItem :label="t('settings.requestOptions.accountRoutingOverride')" :description="t('settings.requestOptions.accountRoutingOverrideHelp')">
        <BaseInput v-model="routing" :aria-label="t('settings.requestOptions.accountRoutingOverride')" placeholder="us / us_cr" clearable :disabled="disabled" />
      </BaseFormItem>
      <BaseFormItem :label="t('settings.requestOptions.guardian')" :description="t('settings.requestOptions.guardianHelp')" class="sm:col-span-2">
        <BaseSwitch v-model="settings.oauthGuardianReviewModel" :label="t('settings.requestOptions.guardian')" show-label :disabled="disabled" />
      </BaseFormItem>
    </div>
  </BaseCard>
  <RequestCountryCard v-model="settings.requestCountry" :disabled="disabled" />
  <BaseCard :title="t('settings.websocketPool.title')" :description="t('settings.websocketPool.accountOpenings')">
    <BaseSwitch v-model="pool.enabled" :label="t('settings.websocketPool.enabled')" show-label :disabled="disabled" class="mb-4" />
    <div class="grid min-w-0 gap-4 sm:grid-cols-2">
      <BaseFormItem v-for="field in fields" :key="field.key" :label="t(`settings.websocketPool.${field.key}`)" :description="t('settings.websocketPool.unit')">
        <template #label-extra>
          <HelpPopover :label="t(`settings.websocketPool.${field.key}`)" :content="t(`settings.websocketPool.${field.key}Help`)" />
        </template>
        <BaseInput v-model="field.value.value" :aria-label="t(`settings.websocketPool.${field.key}`)" type="number" :min="field.key === 'streamIdleTimeoutMs' ? 0 : 1" :max="field.key === 'fastPathBudgetMs' ? 15000 : undefined" step="1" :disabled="disabled" />
      </BaseFormItem>
    </div>
  </BaseCard>
</template>
