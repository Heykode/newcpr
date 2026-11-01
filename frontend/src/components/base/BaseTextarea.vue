<script setup lang="ts">
import { computed, useAttrs } from 'vue'
import { useFormField } from './BaseForm/composables/useFormField'

type TextareaSize = 'sm' | 'md' | 'lg'

defineOptions({ inheritAttrs: false })

const props = withDefaults(
  defineProps<{
    placeholder?: string
    disabled?: boolean
    size?: TextareaSize
    rows?: number
  }>(),
  {
    placeholder: '',
    disabled: false,
    size: 'md',
    rows: 5,
  },
)

const model = defineModel<string>({ default: '' })
const attrs = useAttrs()
const { controlId, invalid, describedBy, required, ariaRequired, validate } = useFormField(attrs)

const rootAttrs = computed(() => ({ class: attrs.class, style: attrs.style }))
const controlAttrs = computed(() => Object.fromEntries(
  Object.entries(attrs).filter(([key]) => ![
    'class',
    'style',
    'id',
    'aria-describedby',
    'aria-invalid',
    'aria-required',
  ].includes(key)),
))

const sizeClasses: Record<TextareaSize, string> = {
  sm: 'px-3 py-2.5 text-xs',
  md: 'px-3.5 py-3 text-cp',
  lg: 'px-4 py-3.5 text-cp-lg',
}

const textareaClasses = computed(() => [
  'cp-scrollbar w-full resize-none rounded-cp border-0 text-cp-text outline-none transition-[background-color,box-shadow,color] duration-160 placeholder:text-cp-text-quaternary motion-reduce:transition-none',
  'leading-[1.55] font-emphasis',
  sizeClasses[props.size],
  props.disabled
    ? 'cursor-not-allowed bg-cp-bg-container-disabled text-cp-text-disabled shadow-none'
    : invalid.value
      ? 'bg-(--cp-input-error-active-bg) shadow-cp-input-error-active'
      : 'bg-[var(--cp-input-bg)] shadow-cp-input hover:not-focus:bg-[var(--cp-input-hover-bg)] hover:not-focus:shadow-cp-input-hover focus:bg-(--cp-input-active-bg) focus:shadow-cp-input-active',
])
</script>

<template>
  <div v-bind="rootAttrs" class="min-w-0">
    <textarea
      v-bind="controlAttrs"
      :id="controlId"
      v-model="model"
      :class="textareaClasses"
      :rows="rows"
      :placeholder="placeholder"
      :disabled="disabled"
      :required="required || undefined"
      :aria-describedby="describedBy"
      :aria-invalid="invalid || undefined"
      :aria-required="ariaRequired"
      @input="validate('change')"
      @blur="validate('blur')"
    />
  </div>
</template>
