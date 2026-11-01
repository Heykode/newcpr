<script setup lang="ts">
import type { Ref } from 'vue'
import type { FormItemContext, FormItemProp, FormItemValidateState, FormLabelPosition, FormRule } from './types'
import { useResizeObserver } from '@vueuse/core'
import { computed, inject, onBeforeUnmount, onMounted, provide, reactive, shallowReactive, shallowRef, useId, useTemplateRef, watch } from 'vue'
import { useFormItemValidation } from './composables/useFormItemValidation'
import { formFieldKey, formKey } from './context'
import { useLocale } from './locale'

const props = withDefaults(defineProps<{
  label?: string
  labelPosition?: FormLabelPosition | ''
  labelWidth?: string | number
  description?: string
  error?: string
  required?: boolean
  controlId?: string
  for?: string
  prop?: FormItemProp
  rules?: FormRule | FormRule[]
  showMessage?: boolean
  validateStatus?: FormItemValidateState
}>(), { required: undefined, showMessage: undefined })
const slots = defineSlots<{ 'default': () => unknown, 'label'?: () => unknown, 'label-extra'?: () => unknown, 'extra'?: () => unknown }>()
const locale = useLocale()
const form = inject(formKey, null)
const validation = useFormItemValidation(props, form)
const { propString, validateMessage, isRequired, isResetting, validate } = validation
const fieldId = `field-${useId()}`
const labelId = `${fieldId}-label`
const controls = shallowReactive(new Set<Readonly<Ref<string>>>())
const controlIds = computed(() => [...controls].map(control => control.value))
const rootRef = useTemplateRef<HTMLElement>('root')
const labelRef = useTemplateRef<HTMLElement>('labelContent')
const resolvedControlId = computed(() => props.for ?? props.controlId ?? (controlIds.value.length === 1 ? controlIds.value[0] : undefined))
const isGroup = computed(() => !resolvedControlId.value && controlIds.value.length > 1)
const hasLabel = computed(() => Boolean(props.label || slots.label))
const labelPosition = computed(() => props.labelPosition || form?.labelPosition.value || 'top')
const labelWidth = computed(() => props.labelWidth ?? form?.labelWidth.value ?? 'auto')
const isHorizontal = computed(() => labelPosition.value !== 'top')
const measuredWidth = shallowRef(0)
useResizeObserver(labelRef, ([entry]) => {
  measuredWidth.value = Math.ceil(entry?.contentRect.width ?? 0)
})
const registeredWidth = computed(() => isHorizontal.value && labelWidth.value === 'auto' && hasLabel.value ? measuredWidth.value : 0)
const unregisterLabel = form?.registerLabelWidth(registeredWidth)
const layoutStyle = computed(() => {
  if (!isHorizontal.value)
    return undefined
  const width = labelWidth.value === 'auto' ? form?.autoLabelWidth.value || measuredWidth.value : labelWidth.value
  return { gridTemplateColumns: `${typeof width === 'number' ? `${width}px` : width} minmax(0, 1fr)` }
})
const showError = computed(() => Boolean(validateMessage.value && (props.showMessage ?? form?.showMessage.value ?? true)))
const descriptionId = computed(() => props.description ? `${fieldId}-description` : undefined)
const errorId = computed(() => showError.value ? `${fieldId}-error` : undefined)
const describedBy = computed(() => [descriptionId.value, errorId.value].filter(Boolean).join(' ') || undefined)
function focus() {
  const id = resolvedControlId.value ?? controlIds.value[0]
  const control = rootRef.value && [...rootRef.value.querySelectorAll<HTMLElement>('[id]')].find(element => element.id === id)
  const target = control?.querySelector<HTMLElement>('[tabindex="0"]') ?? control
  target?.focus({ preventScroll: true })
}
function scrollIntoView(options: boolean | ScrollIntoViewOptions = { block: 'nearest' }) {
  rootRef.value?.scrollIntoView(options)
}
const context: FormItemContext = reactive({
  $el: computed(() => rootRef.value ?? undefined),
  prop: computed(() => props.prop),
  propString,
  fieldValue: validation.fieldValue,
  validateState: validation.validateState,
  validateMessage,
  validate,
  resetField: validation.resetField,
  clearValidate: validation.clearValidate,
  setInitialValue: validation.setInitialValue,
  getInitialValue: validation.getInitialValue,
  focus,
  scrollIntoView,
})
let unregisterField: (() => void) | undefined
onMounted(() => {
  watch(propString, () => {
    unregisterField?.()
    validation.clearValidate()
    unregisterField = propString.value ? form?.register(context) : undefined
  }, { immediate: true })
})
onBeforeUnmount(() => {
  unregisterField?.()
  unregisterLabel?.()
})
defineExpose(context)
provide(formFieldKey, {
  controlId: resolvedControlId,
  labelledBy: computed(() => hasLabel.value ? labelId : undefined),
  describedBy,
  invalid: computed(() => validation.validateState.value === 'error'),
  required: isRequired,
  isResetting,
  validate,
  registerControl(id) {
    controls.add(id)
    return () => controls.delete(id)
  },
})
</script>

<template>
  <div
    ref="root"
    class="relative min-w-0"
    :class="{ 'grid items-start gap-x-3': isHorizontal }"
    :style="layoutStyle"
    :role="isGroup ? 'group' : undefined"
    :aria-labelledby="isGroup && hasLabel ? labelId : undefined"
  >
    <div
      v-if="hasLabel || $slots.extra || isHorizontal"
      class="flex min-w-0 items-center gap-3"
      :class="isHorizontal ? ['min-h-cp-control', labelPosition === 'right' ? 'justify-end' : 'justify-start'] : 'mb-2 min-h-4 justify-between'"
    >
      <div ref="labelContent" class="inline-flex items-center gap-1.5" :class="isHorizontal && labelWidth === 'auto' ? 'w-max shrink-0' : 'min-w-0'">
        <!-- 复合字段提供组标签，控件保留独立 ID；显式 for 只关联指定控件 -->
        <component
          :is="resolvedControlId ? 'label' : 'span'"
          :id="labelId"
          :for="resolvedControlId"
          class="flex min-w-0 items-center gap-1.5 text-cp font-medium text-cp-text-secondary"
          :class="isHorizontal ? 'leading-snug' : 'leading-none'"
        >
          <slot name="label">
            {{ label }}
          </slot>
          <span v-if="isRequired" class="font-bold text-cp-error">*</span>
          <span v-if="isRequired" class="sr-only">{{ locale.form.required }}</span>
        </component>
        <slot name="label-extra" />
      </div>
      <div v-if="$slots.extra && !isHorizontal" class="inline-flex shrink-0 items-center">
        <slot name="extra" />
      </div>
    </div>
    <div class="relative min-w-0">
      <div v-if="$slots.extra && isHorizontal" class="mb-2 flex justify-end">
        <slot name="extra" />
      </div>
      <p v-if="description" :id="descriptionId" class="mt-0 mb-2 text-xs leading-[1.35] font-emphasis text-cp-text-quaternary">
        {{ description }}
      </p>
      <slot />
      <Transition name="cp-zoom-in-top">
        <p v-if="showError" :id="errorId" :title="validateMessage" class="mt-2 mb-0 text-xs leading-snug font-normal text-cp-error-text" aria-live="polite">
          {{ validateMessage }}
        </p>
      </Transition>
    </div>
  </div>
</template>
