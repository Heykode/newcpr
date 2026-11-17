<script setup lang="ts">
import type { ComponentSize } from './controls/types'
import { Check, Minus } from '@lucide/vue'
import { computed, useAttrs, useTemplateRef, watchEffect } from 'vue'
import { useFormField } from './BaseForm/composables/useFormField'
import { choiceLabelClasses, useComponentSize } from './controls/size'

defineOptions({ inheritAttrs: false })

const props = withDefaults(
  defineProps<{
    size?: ComponentSize
    validateEvent?: boolean
    label?: string
    showLabel?: boolean
    indeterminate?: boolean
    disabled?: boolean
  }>(),
  {
    validateEvent: true,
    indeterminate: false,
    disabled: false,
    showLabel: false,
  },
)
const emit = defineEmits<{ change: [value: boolean], focus: [event: FocusEvent], blur: [event: FocusEvent] }>()

const componentSize = useComponentSize(() => props.size)

const model = defineModel<boolean>({ default: false })
const attrs = useAttrs()
const { controlId, invalid, describedBy, required, ariaRequired, rootAttrs, controlAttrs, validate } = useFormField(attrs, () => props.validateEvent)
const inputRef = useTemplateRef<HTMLInputElement>('input')
defineExpose({ focus: () => inputRef.value?.focus(), blur: () => inputRef.value?.blur() })
const checked = computed(() => model.value || props.indeterminate)

watchEffect(() => {
  if (inputRef.value)
    inputRef.value.indeterminate = props.indeterminate
})

const boxClasses = computed(() => [
  'relative inline-flex shrink-0 items-center justify-center rounded-cp-sm border-0 transition-[background-color,box-shadow,color] duration-150 motion-reduce:transition-none',
  { sm: 'size-3.5', md: 'size-4', lg: 'size-5' }[componentSize.value],
  props.disabled
    ? 'bg-cp-bg-container-disabled text-cp-text-disabled'
    : invalid.value
      ? checked.value ? 'bg-cp-error text-cp-white' : 'bg-cp-error-container text-transparent shadow-[inset_0_0_0_1px_var(--cp-color-error)]'
      : checked.value
        ? 'bg-cp-primary text-(--cp-button-primary-color)'
        : 'bg-cp-bg-container text-transparent shadow-[inset_0_0_0_1px_var(--cp-color-border)]',
])
</script>

<template>
  <!-- 原生控件嵌套在标签内，整行保持同一点击目标 -->
  <!-- eslint-disable-next-line vue-a11y/label-has-for -->
  <label
    v-bind="rootAttrs"
    class="group relative inline-flex min-h-4 min-w-4 select-none items-center leading-none font-emphasis"
    :class="[choiceLabelClasses[componentSize], disabled ? 'cursor-not-allowed opacity-55' : 'cursor-pointer']"
  >
    <input
      v-bind="controlAttrs"
      :id="controlId"
      ref="input"
      v-model="model"
      type="checkbox"
      class="peer sr-only"
      :aria-label="typeof attrs['aria-label'] === 'string' ? attrs['aria-label'] : label"
      :aria-describedby="describedBy"
      :aria-invalid="invalid || undefined"
      :required="required"
      :aria-required="ariaRequired"
      :disabled="disabled"
      @change="emit('change', ($event.target as HTMLInputElement).checked); validate('change')"
      @focus="emit('focus', $event)"
      @blur="emit('blur', $event); validate('blur')"
    >
    <span
      :class="boxClasses"
      class="peer-focus-visible:ring-2 peer-focus-visible:ring-cp-control-outline peer-focus-visible:ring-offset-2 peer-focus-visible:ring-offset-cp-bg-container"
    >
      <Minus
        class="absolute size-3/4 transition-opacity duration-150 motion-reduce:transition-none"
        :class="indeterminate ? 'opacity-100' : 'opacity-0'"
      />
      <Check
        class="absolute size-3/4 transition-opacity duration-150 motion-reduce:transition-none"
        :class="!indeterminate && model ? 'opacity-100' : 'opacity-0'"
      />
    </span>
    <span v-if="showLabel || $slots.label" class="min-w-0 leading-none font-emphasis"><slot name="label">{{ label }}</slot></span>
  </label>
</template>
