<script setup lang="ts">
import type { TokenPrices } from '@/api'

import { useI18n } from 'vue-i18n'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import { effectivePrice, priceFields, validPrice } from './model'

const props = defineProps<{
  base?: TokenPrices
  custom?: TokenPrices
  multiplierBps?: number
  disabled: boolean
}>()

defineEmits<{ change: [field: keyof TokenPrices, value: string] }>()

const { t } = useI18n({ useScope: 'global' })

function preview(field: keyof TokenPrices) {
  const price = (props.custom ?? props.base)?.[field]
  return props.multiplierBps === undefined || price === undefined || !validPrice(price)
    ? '—'
    : effectivePrice(price, props.multiplierBps)
}
</script>

<template>
  <div class="grid grid-cols-1 gap-x-4 gap-y-5 sm:grid-cols-2 sm:gap-x-6">
    <div v-for="field in priceFields" :key="field.key" class="min-w-0">
      <BaseFormItem v-if="custom" :label="field.label">
        <BaseInput
          :model-value="custom[field.key]"
          :aria-label="t('settings.pricing.unitPrice', { label: field.label })"
          class="font-mono"
          inputmode="decimal"
          placeholder="0.00"
          :disabled="disabled"
          @update:model-value="$emit('change', field.key, $event)"
        >
          <template #suffix>
            <span class="text-cp-xs">USD / 1M</span>
          </template>
        </BaseInput>
      </BaseFormItem>
      <div v-else>
        <div class="mb-2 text-cp leading-none font-medium text-cp-text-secondary">
          {{ field.label }}
        </div>
        <div class="flex h-cp-control items-center font-mono text-lg tabular-nums text-cp-text">
          {{ effectivePrice(base?.[field.key]) }}
        </div>
      </div>
      <div v-if="(custom && base) || multiplierBps !== 10000" class="mt-2 grid gap-1 text-cp-xs">
        <span v-if="custom && base" class="text-cp-text-tertiary">
          {{ t('settings.pricing.source') }} <span class="break-all font-mono tabular-nums">{{ effectivePrice(base[field.key]) }}</span>
        </span>
        <span v-if="multiplierBps !== 10000" class="text-cp-text-secondary">
          {{ t('settings.pricing.takeEffect') }} <span class="break-all font-mono tabular-nums text-cp-primary-text">{{ preview(field.key) }}</span>
        </span>
      </div>
    </div>
  </div>
</template>
