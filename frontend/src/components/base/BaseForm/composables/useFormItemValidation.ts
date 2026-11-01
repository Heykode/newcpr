import type { FormContext } from '../context'
import type { FormItemProp, FormItemValidateState, FormRule, FormValidateCallback, FormValidateFailure } from '../types'
import Schema from '@rc-component/async-validator'
import { cloneDeep } from 'es-toolkit'
import { get, set } from 'es-toolkit/compat'
import { computed, nextTick, onBeforeUnmount, shallowRef } from 'vue'
import { useLocale } from '../locale'

interface ValidationProps {
  prop?: FormItemProp
  rules?: FormRule | FormRule[]
  required?: boolean
  error?: string
  validateStatus?: FormItemValidateState
}

export function useFormItemValidation(props: ValidationProps, form: FormContext | null) {
  const locale = useLocale()
  const propString = computed(() => Array.isArray(props.prop) ? props.prop.join('.') : props.prop ?? '')
  const fieldValue = computed(() => form?.model.value && props.prop ? get(form.model.value, props.prop) : undefined)
  const state = shallowRef<FormItemValidateState>('')
  const message = shallowRef('')
  const isResetting = shallowRef(false)
  const validateState = computed(() => props.validateStatus || (props.error ? 'error' : state.value))
  const validateMessage = computed(() => props.error || message.value)
  const rules = computed(() => {
    const formRules = propString.value ? get(form?.rules.value, propString.value) as FormRule | FormRule[] | undefined : undefined
    const normalized = [props.rules, formRules].flatMap(value => value ? Array.isArray(value) ? value : [value] : [])
    if (props.required !== undefined) {
      if (normalized.some(rule => 'required' in rule))
        return normalized.map(rule => 'required' in rule ? { ...rule, required: props.required } : rule)
      normalized.push({ required: props.required })
    }
    return normalized
  })
  const isRequired = computed(() => rules.value.some(rule => rule.required))
  let validationSequence = 0

  async function validate(trigger = '', callback?: FormValidateCallback) {
    if (isResetting.value || !props.prop)
      return false
    const selectedRules = rules.value
      .filter(rule => !trigger || !rule.trigger || (Array.isArray(rule.trigger) ? rule.trigger : [rule.trigger]).includes(trigger))
      .map(({ trigger: _trigger, ...rule }) => rule)
    if (!selectedRules.length && !props.error) {
      if (!rules.value.length)
        clearValidate()
      await callback?.(true)
      return true
    }
    const sequence = ++validationSequence
    state.value = 'validating'
    const validator = new Schema({ [propString.value]: selectedRules })
    validator.messages({
      required: () => locale.value.form.requiredMessage,
      default: () => locale.value.form.invalidMessage,
    })
    let failure: FormValidateFailure | undefined
    try {
      await validator.validate({ [propString.value]: fieldValue.value }, { firstFields: true, suppressWarning: true, suppressValidatorError: true })
    }
    catch (cause) {
      if (!cause || typeof cause !== 'object' || !('fields' in cause))
        throw cause
      failure = cause as FormValidateFailure
    }
    // 清空、卸载或后一次校验会使当前异步结果失效，不能用旧结果覆盖最新输入
    if (sequence !== validationSequence)
      return false
    if (props.error) {
      const error = { message: props.error, field: propString.value }
      failure = { errors: [error], fields: { [propString.value]: [error] } }
    }
    if (failure) {
      state.value = 'error'
      message.value = failure.errors?.[0]?.message || locale.value.form.invalidMessage
      form?.emitValidate(props.prop, false, message.value)
      if (callback) {
        await callback(false, failure.fields)
        return false
      }
      return Promise.reject(failure.fields)
    }
    state.value = 'success'
    message.value = ''
    form?.emitValidate(props.prop, true, '')
    await callback?.(true)
    return true
  }
  function clearValidate() {
    validationSequence++
    state.value = ''
    message.value = ''
  }
  function getInitialValue() {
    return cloneDeep(form?.getInitialValue(propString.value))
  }
  function setInitialValue(value: unknown) {
    form?.setInitialValue(propString.value, value)
  }
  async function resetField() {
    if (!form?.model.value || !props.prop)
      return
    isResetting.value = true
    clearValidate()
    set(form.model.value, props.prop, getInitialValue())
    await nextTick()
    clearValidate()
    isResetting.value = false
  }
  onBeforeUnmount(clearValidate)
  return { propString, fieldValue, validateState, validateMessage, isRequired, isResetting, validate, clearValidate, resetField, getInitialValue, setInitialValue }
}
