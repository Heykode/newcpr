<script setup lang="ts">
import type { ComponentSize } from './controls/types'
import { Minus, Plus } from '@lucide/vue'
import { computed, useAttrs } from 'vue'

import { useFormField } from './BaseForm/composables/useFormField'
import BaseIconButton from './BaseIconButton.vue'
import { getInputClasses } from './controls/input-styles'

defineOptions({ inheritAttrs: false })

const props = withDefaults(
  defineProps<{
    size?: ComponentSize
    label: string
    min?: number
    max?: number
    step?: number
    unit?: string
    disabled?: boolean
  }>(),
  {
    size: 'sm',
    min: undefined,
    max: undefined,
    step: 1,
    unit: undefined,
    disabled: false,
  },
)

const model = defineModel<number>({ required: true })
const attrs = useAttrs()

const { rootAttrs, controlAttrs, controlId, invalid, describedBy, ariaRequired, validate } = useFormField(attrs)
const canDecrease = computed(() => !props.disabled && (props.min === undefined || model.value > props.min))
const canIncrease = computed(() => !props.disabled && (props.max === undefined || model.value < props.max))

function clampValue(value: number) {
  return Math.min(props.max ?? Number.POSITIVE_INFINITY, Math.max(props.min ?? Number.NEGATIVE_INFINITY, value))
}

function stepPrecision() {
  const decimal = String(props.step).split('.')[1]
  return decimal?.length ?? 0
}

function updateModel(event: Event) {
  const value = (event.target as HTMLInputElement).valueAsNumber
  if (Number.isFinite(value))
    model.value = clampValue(value)
  void validate('change')
}

function restoreValue(event: Event) {
  const input = event.target as HTMLInputElement
  input.value = String(model.value)
  void validate('blur')
}

function stepBy(direction: -1 | 1) {
  const next = clampValue(model.value + props.step * direction)
  model.value = Number(next.toFixed(stepPrecision()))
  void validate('change')
}
</script>

<template>
  <div
    v-bind="rootAttrs"
    class="inline-flex items-center"
    :class="getInputClasses(size, disabled, invalid, { inset: 'item' })"
  >
    <BaseIconButton
      :label="`减少${label}`"
      size="sm"
      variant="ghost"
      class="size-(--field-row)!"
      :disabled="!canDecrease"
      @click="stepBy(-1)"
    >
      <Minus class="size-3" />
    </BaseIconButton>

    <span class="inline-flex min-w-0 flex-1 items-baseline justify-center gap-1">
      <input
        v-bind="controlAttrs"
        :id="controlId"
        :aria-invalid="invalid || undefined"
        :aria-describedby="describedBy"
        :aria-required="ariaRequired"
        :value="model"
        type="number"
        inputmode="decimal"
        :aria-label="label"
        :min="min"
        :max="max"
        :step="step"
        :disabled="disabled"
        class="h-(--field-row) min-w-0 appearance-[textfield] border-0 bg-transparent p-0 font-mono tabular-nums text-cp-text outline-none disabled:text-cp-text-disabled [&::-webkit-inner-spin-button]:m-0 [&::-webkit-inner-spin-button]:appearance-none [&::-webkit-outer-spin-button]:m-0 [&::-webkit-outer-spin-button]:appearance-none"
        :class="size === 'sm' ? 'w-9 text-right font-bold' : 'w-full text-center'"
        @input="updateModel"
        @blur="restoreValue"
      >
      <span v-if="unit" class="shrink-0 text-[9px] font-emphasis text-cp-text-quaternary">
        {{ unit }}
      </span>
    </span>

    <BaseIconButton
      :label="`增加${label}`"
      size="sm"
      variant="ghost"
      class="size-(--field-row)!"
      :disabled="!canIncrease"
      @click="stepBy(1)"
    >
      <Plus class="size-3" />
    </BaseIconButton>
  </div>
</template>
