<script setup lang="ts">
import type { rotationOptions } from '../constants'
import type { SmartSchedulingConfig } from '@/api/modules/settings'
import { RotateCcw } from '@lucide/vue'
import { defaultSmartScheduling } from '@/api/modules/settings'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'

type RotationOption = (typeof rotationOptions)[number]
type RotationStrategy = RotationOption['value']

defineProps<{
  options: readonly RotationOption[]
  disabled?: boolean
}>()

const model = defineModel<RotationStrategy | ''>({ required: true })
const smart = defineModel<SmartSchedulingConfig>('smartScheduling', { required: true })
const weights = [
  { key: 'loadWeight', label: '负载权重' },
  { key: 'quotaWeight', label: '剩余额度权重' },
  { key: 'healthWeight', label: '健康权重' },
  { key: 'latencyWeight', label: '首输出延迟权重' },
  { key: 'resetWeight', label: '额度重置权重' },
  { key: 'queueWeight', label: '排队压力权重' },
] as const
function setWeight(key: typeof weights[number]['key'], value: string) {
  smart.value = { ...smart.value, [key]: value.trim() === '' ? Number.NaN : Number(value) }
}
</script>

<template>
  <BaseCard
    title="调度策略"
    description="决定每次请求如何调度账号池"
  >
    <div class="grid max-w-6xl gap-3 lg:grid-cols-4">
      <button
        v-for="option in options"
        :key="option.value"
        type="button"
        class="min-h-25 cursor-pointer rounded-cp border-0 px-4 py-3.5 text-left shadow-cp-input outline-none transition-[background-color,box-shadow,color] duration-160 focus-visible:ring-2 focus-visible:ring-cp-control-outline"
        :class="
          model === option.value
            ? 'bg-cp-control-item-bg-active text-cp-primary-text shadow-cp-tertiary'
            : 'bg-(--cp-input-bg,var(--cp-input-bg)) text-cp-text hover:bg-(--cp-input-hover-bg,var(--cp-input-hover-bg)) hover:shadow-cp-input-hover'
        "
        :aria-pressed="model === option.value"
        :disabled="disabled"
        @click="model = option.value"
      >
        <span class="flex items-center gap-2">
          <span
            class="inline-flex size-4 shrink-0 items-center justify-center rounded-full bg-cp-bg-container shadow-[inset_0_0_0_1px_var(--cp-color-border)]"
          >
            <span
              class="size-2 rounded-full transition-opacity duration-150"
              :class="model === option.value ? 'bg-cp-primary opacity-100' : 'opacity-0'"
            />
          </span>
          <span class="text-cp-lg leading-[1.15] font-heavy">{{ option.label }}</span>
        </span>
        <span class="mt-2 block text-cp leading-normal font-emphasis text-cp-text-secondary">
          {{ option.description }}
        </span>
      </button>
    </div>
    <section v-if="model === 'smart' || model === 'sticky'" aria-label="智能调度参数" class="mt-5 border-t border-cp-border pt-4">
      <div class="mb-4 flex flex-wrap items-center justify-between gap-3">
        <h3 class="m-0 text-sm font-medium">
          智能调度参数
        </h3>
        <BaseButton :disabled="disabled" title="恢复默认" @click="smart = defaultSmartScheduling()">
          <template #icon>
            <RotateCcw class="size-4" />
          </template>
          恢复默认
        </BaseButton>
      </div>
      <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <BaseFormItem v-for="weight in weights" :key="weight.key" :label="weight.label">
          <BaseInput
            :model-value="Number.isFinite(smart[weight.key]) ? String(smart[weight.key]) : ''"
            :aria-label="weight.label"
            type="number"
            min="0"
            max="10"
            step="0.1"
            :disabled="disabled"
            @update:model-value="setWeight(weight.key, $event)"
          />
        </BaseFormItem>
      </div>
      <BaseSwitch v-model="smart.preferHigherWeight" class="mt-4" :disabled="disabled" label="主动回切高权重账号" show-label />
    </section>
  </BaseCard>
</template>
