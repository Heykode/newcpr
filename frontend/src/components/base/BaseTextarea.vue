<script setup lang="ts">
import type { ComponentSize } from './controls/types'
import { useResizeObserver } from '@vueuse/core'
import { nextTick, shallowRef, useAttrs, useTemplateRef, watch } from 'vue'
import { useFormField } from './BaseForm/composables/useFormField'
import { getInputClasses } from './controls/input-styles'

defineOptions({ inheritAttrs: false })

const props = withDefaults(
  defineProps<{
    placeholder?: string
    disabled?: boolean
    size?: ComponentSize
    rows?: number
    autosize?: boolean | { minRows?: number, maxRows?: number }
    resize?: 'none' | 'both' | 'horizontal' | 'vertical'
  }>(),
  {
    placeholder: '',
    disabled: false,
    size: 'md',
    rows: 5,
    resize: 'vertical',
  },
)

const emit = defineEmits<{ input: [value: string], change: [value: string], focus: [event: FocusEvent], blur: [event: FocusEvent] }>()
const model = defineModel<string>({ default: '' })
const attrs = useAttrs()
const { controlId, invalid, describedBy, required, ariaRequired, rootAttrs, controlAttrs, validate } = useFormField(attrs)
const textareaRef = useTemplateRef<HTMLTextAreaElement>('textarea')
const composing = shallowRef(false)
function updateModel(event: Event) {
  if (composing.value)
    return
  const value = (event.target as HTMLTextAreaElement).value
  if (value !== model.value) {
    model.value = value
    emit('input', value)
  }
}

function handleChange(event: Event) {
  updateModel(event)
  emit('change', (event.target as HTMLTextAreaElement).value)
  validate('change')
}

function finishComposition(event: CompositionEvent) {
  composing.value = false
  // 浏览器可能紧接着发送 input，等模型同步后再补齐没有 input 的情况
  queueMicrotask(() => {
    void nextTick(() => updateModel(event))
  })
}
function resizeToContent() {
  const element = textareaRef.value
  if (!element)
    return
  if (!props.autosize) {
    element.style.height = ''
    return
  }
  const options = typeof props.autosize === 'object' ? props.autosize : {}
  const style = getComputedStyle(element)
  const lineHeight = Number.parseFloat(style.lineHeight)
  const padding = Number.parseFloat(style.paddingTop) + Number.parseFloat(style.paddingBottom)
  const minHeight = (options.minRows ?? props.rows) * lineHeight + padding
  const maxHeight = (options.maxRows ?? Infinity) * lineHeight + padding
  element.style.height = 'auto'
  element.style.height = `${Math.max(minHeight, Math.min(maxHeight, element.scrollHeight))}px`
}
watch([model, () => props.autosize, () => props.rows, () => props.size], () => {
  void nextTick(resizeToContent)
}, { immediate: true, deep: true })
let previousWidth = 0
useResizeObserver(textareaRef, ([entry]) => {
  if (entry && entry.contentRect.width !== previousWidth) {
    previousWidth = entry.contentRect.width
    resizeToContent()
  }
})
function clear() {
  model.value = ''
  emit('input', '')
  emit('change', '')
  validate('change')
}
defineExpose({ focus: () => textareaRef.value?.focus(), blur: () => textareaRef.value?.blur(), select: () => textareaRef.value?.select(), clear })
</script>

<template>
  <div v-bind="rootAttrs" class="cp-textarea min-w-0">
    <textarea
      v-bind="controlAttrs"
      :id="controlId"
      ref="textarea"
      :value="model"
      class="cp-scrollbar w-full font-emphasis leading-(--field-row) placeholder:font-normal placeholder:text-cp-text-quaternary"
      :class="getInputClasses(size, disabled, invalid)"
      :style="{ resize: disabled || autosize ? 'none' : resize }"
      :rows="rows"
      :placeholder="placeholder"
      :disabled="disabled"
      :required="required"
      :aria-describedby="describedBy"
      :aria-invalid="invalid || undefined"
      :aria-required="ariaRequired"
      @input="updateModel"
      @change="handleChange"
      @compositionstart="composing = true"
      @compositionend="finishComposition"
      @focus="emit('focus', $event)"
      @blur="emit('blur', $event); validate('blur')"
    />
  </div>
</template>
