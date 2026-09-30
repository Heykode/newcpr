<script setup lang="ts">
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import { chinaToday } from '../utils/purchaseCost'

defineProps<{ disabled?: boolean, batch?: boolean, importing?: boolean }>()
const amount = defineModel<string>('amount', { default: '' })
const cycleStart = defineModel<string>('cycleStart', { default: '' })
const apply = defineModel<boolean>('apply', { default: false })
</script>

<template>
  <fieldset class="m-0 grid min-w-0 gap-3 border-0 p-0">
    <legend class="mb-3 text-cp font-medium text-cp-text-secondary">
      账号成本
    </legend>
    <BaseCheckbox v-if="batch" v-model="apply" label="修改所选账号成本" show-label :disabled="disabled" />
    <div class="grid min-w-0 gap-3 sm:grid-cols-2">
      <BaseFormItem label="每账号月成本（元，选填）">
        <BaseInput v-model="amount" aria-label="每账号月成本" inputmode="decimal" :disabled="disabled || (batch && !apply)" :placeholder="importing ? '不填则保留已有成本' : '留空清除成本'" />
      </BaseFormItem>
      <BaseFormItem label="续费起算日（选填）">
        <BaseInput v-model="cycleStart" aria-label="续费起算日" type="date" min="2000-01-01" :max="chinaToday()" :disabled="disabled || (batch && !apply)" />
      </BaseFormItem>
    </div>
    <p class="m-0 text-cp-xs text-cp-text-secondary">
      起算日留空保留原周期，首次设置默认为今天；按月续用同一成本。单位：元／美元额度。
    </p>
  </fieldset>
</template>
