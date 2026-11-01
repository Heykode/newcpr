<script setup lang="ts">
import type { ValidateFieldsError } from '@rc-component/async-validator'
import type { Ref } from 'vue'
import type { FormItemContext, FormItemProp, FormLabelPosition, FormRules, FormValidateCallback } from './types'
import { cloneDeep } from 'es-toolkit'
import { get, set } from 'es-toolkit/compat'
import { computed, inject, nextTick, provide, shallowReactive, watch } from 'vue'
import { formKey } from './context'

const props = withDefaults(defineProps<{
  model?: object
  rules?: FormRules
  labelPosition?: FormLabelPosition
  labelWidth?: string | number
  showMessage?: boolean
  validateOnRuleChange?: boolean
  scrollToError?: boolean
  scrollIntoViewOptions?: boolean | ScrollIntoViewOptions
  focusOnError?: boolean
  as?: 'form' | 'div'
}>(), {
  labelPosition: 'top',
  labelWidth: 'auto',
  showMessage: true,
  validateOnRuleChange: true,
  scrollIntoViewOptions: undefined,
})
const emit = defineEmits<{ validate: [prop: FormItemProp, isValid: boolean, message: string] }>()
defineSlots<{ default: () => unknown }>()

const fields = shallowReactive<FormItemContext[]>([])
const parentForm = inject(formKey, undefined)
const initialValues = new Map<string, unknown>()
const labelWidths = shallowReactive(new Set<Readonly<Ref<number>>>())
const autoLabelWidth = computed(() => Math.max(0, ...[...labelWidths].map(width => width.value)))
const scrollOptions = computed<boolean | ScrollIntoViewOptions>(() => props.scrollIntoViewOptions ?? { block: 'nearest' })
const pathString = (path: FormItemProp) => Array.isArray(path) ? path.join('.') : path

function selectedFields(paths?: FormItemProp | FormItemProp[]) {
  const keys = paths === undefined || paths.length === 0 ? undefined : (Array.isArray(paths) ? paths : [paths]).map(pathString)
  return fields.filter(field => keys === undefined || keys.includes(field.propString)).sort((a, b) => {
    if (!a.$el || !b.$el)
      return 0
    const position = a.$el.compareDocumentPosition(b.$el)
    return position & Node.DOCUMENT_POSITION_FOLLOWING ? -1 : position & Node.DOCUMENT_POSITION_PRECEDING ? 1 : 0
  })
}

async function validateField(paths?: FormItemProp | FormItemProp[], callback?: FormValidateCallback) {
  if (!props.model)
    return false
  const selected = selectedFields(paths)
  const results = await Promise.allSettled(selected.map(field => field.validate('')))
  const invalidFields: ValidateFieldsError = {}
  let firstInvalid: FormItemContext | undefined
  results.forEach((result, index) => {
    if (result.status === 'rejected') {
      if (result.reason instanceof Error)
        throw result.reason
      Object.assign(invalidFields, result.reason)
      firstInvalid ??= selected[index]
    }
  })
  if (!Object.keys(invalidFields).length) {
    const valid = results.every(result => result.status === 'fulfilled' && result.value)
    await callback?.(valid)
    return valid
  }
  await nextTick()
  if (props.scrollToError)
    firstInvalid?.scrollIntoView(scrollOptions.value)
  if (props.focusOnError)
    firstInvalid?.focus()
  if (callback) {
    await callback(false, invalidFields)
    return false
  }
  return Promise.reject(invalidFields)
}
function validate(callback?: FormValidateCallback) {
  return validateField(undefined, callback)
}
function clearValidate(paths?: FormItemProp | FormItemProp[]) {
  selectedFields(paths).forEach(field => field.clearValidate())
}
function resetFields(paths?: FormItemProp | FormItemProp[]) {
  if (!props.model)
    return
  const selected = selectedFields(paths)
  selected.forEach(field => void field.resetField())
  const keys = paths === undefined || paths.length === 0 ? [...initialValues.keys()] : (Array.isArray(paths) ? paths : [paths]).map(pathString)
  // 临时卸载的字段仍使用首次登记的初值，避免重新挂载后把草稿当作已保存值
  for (const key of keys) {
    if (initialValues.has(key) && !selected.some(field => field.propString === key))
      set(props.model, key, cloneDeep(initialValues.get(key)))
  }
}
function getField(path: FormItemProp) {
  return fields.find(field => field.propString === pathString(path))
}
function scrollToField(path: FormItemProp) {
  getField(path)?.scrollIntoView(scrollOptions.value)
}
function focusField(path: FormItemProp) {
  getField(path)?.focus()
}
function setInitialValues(model: object) {
  for (const path of initialValues.keys())
    initialValues.set(path, cloneDeep(get(model, path)))
}
const instance = { validate, validateField, resetFields, clearValidate, scrollToField, focusField, fields, getField, setInitialValues }

if (props.model || !parentForm) {
  provide(formKey, {
    ...instance,
    model: computed(() => props.model),
    rules: computed(() => props.rules ?? {}),
    labelPosition: computed(() => props.labelPosition),
    labelWidth: computed(() => props.labelWidth),
    autoLabelWidth,
    showMessage: computed(() => props.showMessage),
    register(field) {
      fields.push(field)
      if (!initialValues.has(field.propString))
        initialValues.set(field.propString, cloneDeep(field.fieldValue))
      return () => {
        const index = fields.indexOf(field)
        if (index >= 0)
          fields.splice(index, 1)
      }
    },
    registerLabelWidth(width) {
      labelWidths.add(width)
      return () => labelWidths.delete(width)
    },
    getInitialValue: path => initialValues.get(path),
    setInitialValue: (path, value) => initialValues.set(path, cloneDeep(value)),
    emitValidate: (prop, valid, message) => emit('validate', prop, valid, message),
  })
}
watch(() => props.rules, () => {
  if (props.validateOnRuleChange)
    void validate().catch(() => undefined)
}, { deep: true, flush: 'post' })
defineExpose(instance)
</script>

<template>
  <component :is="as ?? (parentForm ? 'div' : 'form')" class="grid min-w-0 gap-4.5" @submit.prevent>
    <slot />
  </component>
</template>
