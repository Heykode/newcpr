<script setup lang="ts">
import type { ComponentSize } from './controls/types'
import { computed, useAttrs, useTemplateRef } from 'vue'
import { useFormField } from './BaseForm/composables/useFormField'
import { choiceLabelClasses, useComponentSize } from './controls/size'

defineOptions({ inheritAttrs: false })

const props = withDefaults(
  defineProps<{
    size?: ComponentSize
    validateEvent?: boolean
    value: string | number
    name?: string
    label?: string
    showLabel?: boolean
    disabled?: boolean
  }>(),
  {
    validateEvent: true,
    disabled: false,
    showLabel: false,
  },
)
const emit = defineEmits<{ change: [value: string | number], focus: [event: FocusEvent], blur: [event: FocusEvent] }>()

defineSlots<{
  default?: () => unknown
}>()

const componentSize = useComponentSize(() => props.size)

const model = defineModel<string | number>({ required: true })
const attrs = useAttrs()
const inputRef = useTemplateRef<HTMLInputElement>('input')
defineExpose({ focus: () => inputRef.value?.focus(), blur: () => inputRef.value?.blur() })
const { controlId, invalid, describedBy, required, ariaRequired, rootAttrs, controlAttrs, validate } = useFormField(attrs, () => props.validateEvent)
const checked = computed(() => model.value === props.value)
const indicatorClasses = computed(() => [
  'relative inline-grid shrink-0 place-items-center rounded-full transition-colors duration-150 motion-reduce:transition-none',
  { sm: 'size-3.5', md: 'size-4', lg: 'size-5' }[componentSize.value],
  props.disabled
    ? 'bg-cp-bg-container-disabled text-cp-text-disabled'
    : invalid.value
      ? checked.value ? 'bg-cp-error text-cp-white' : 'bg-cp-error-container text-transparent shadow-[inset_0_0_0_1px_var(--cp-color-error)]'
      : checked.value
        ? 'bg-cp-primary text-(--cp-button-primary-color)'
        : 'bg-cp-fill-secondary text-transparent',
])
</script>

<template>
  <!-- 原生控件嵌套在标签内，整行保持同一点击目标 -->
  <!-- eslint-disable-next-line vue-a11y/label-has-for -->
  <label
    v-bind="rootAttrs"
    class="group relative inline-flex min-h-4 min-w-4 items-center"
    :class="[choiceLabelClasses[componentSize], disabled ? 'cursor-not-allowed opacity-55' : 'cursor-pointer']"
  >
    <input
      v-bind="controlAttrs"
      :id="controlId"
      ref="input"
      v-model="model"
      :aria-describedby="describedBy"
      :aria-invalid="invalid || undefined"
      :required="required"
      :aria-required="ariaRequired"
      type="radio"
      class="peer sr-only"
      :name="name"
      :value="value"
      :aria-label="typeof attrs['aria-label'] === 'string' ? attrs['aria-label'] : label"
      :disabled="disabled"
      @change="emit('change', props.value); validate('change')"
      @focus="emit('focus', $event)"
      @blur="emit('blur', $event); validate('blur')"
    >
    <span
      :class="indicatorClasses"
      class="peer-focus-visible:ring-2 peer-focus-visible:ring-cp-control-outline peer-focus-visible:ring-offset-2 peer-focus-visible:ring-offset-cp-bg-container"
    >
      <span class="size-[37.5%] rounded-full bg-current" />
    </span>
    <span v-if="$slots.default" class="min-w-0 flex-1">
      <slot />
    </span>
    <span v-else-if="showLabel" class="leading-none font-emphasis">
      {{ label }}
    </span>
  </label>
</template>
