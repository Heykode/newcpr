<script setup lang="ts">
import type { ResetWindowWeights } from '@/api/modules/settings'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import HelpPopover from '@/components/HelpPopover.vue'
import { t } from '@/i18n'

defineProps<{ disabled?: boolean, help: string }>()
const model = defineModel<ResetWindowWeights>({ required: true })
const periods = ['fiveHour', 'weekly', 'monthly'] as const
function setWeight(key: keyof ResetWindowWeights, value: string) {
  model.value = { ...model.value, [key]: value.trim() === '' ? Number.NaN : Number(value) }
}
</script>

<template>
  <section class="grid gap-3" :aria-label="t('settings.scheduling.windowMode')">
    <div class="flex items-center gap-1">
      <h3 class="m-0 text-sm font-medium">
        {{ t('settings.scheduling.windowMode') }}
      </h3>
      <HelpPopover :label="t('settings.scheduling.windowMode')" :content="help" />
    </div>
    <div class="grid gap-3 sm:grid-cols-3">
      <BaseFormItem v-for="period in periods" :key="period" :label="t(`settings.scheduling.${period}`)">
        <BaseInput :model-value="Number.isFinite(model[period]) ? String(model[period]) : ''" :aria-label="t(`settings.scheduling.${period}`)" type="number" min="0" max="10" step="0.1" :disabled="disabled" @update:model-value="setWeight(period, $event)" />
      </BaseFormItem>
    </div>
  </section>
</template>
