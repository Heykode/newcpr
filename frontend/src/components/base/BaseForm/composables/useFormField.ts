import type { useAttrs } from 'vue'
import type { FormValidationTrigger } from '../types'
import { computed, inject, nextTick, onBeforeUnmount, onBeforeUpdate, shallowRef, useId } from 'vue'
import { formFieldKey } from '../context'

export function useFormField(attrs: ReturnType<typeof useAttrs>, fallbackId?: string) {
  const field = inject(formFieldKey, null)
  const generatedId = useId()
  const resolveId = () => typeof attrs.id === 'string' ? attrs.id : (fallbackId ?? `control-${generatedId}`)
  const controlId = shallowRef(resolveId())
  // attrs 不可监听，在组件更新前同步 ID，表单项只登记控件，不向多个控件分发同一个 ID
  onBeforeUpdate(() => {
    controlId.value = resolveId()
  })
  const unregister = field?.registerControl(controlId)
  onBeforeUnmount(() => unregister?.())
  const belongsToField = computed(() => field && (!field.controlId.value || field.controlId.value === controlId.value))
  const labelledBy = computed(() => field?.controlId.value === controlId.value ? field.labelledBy.value : undefined)
  const invalid = computed(() => Boolean(
    (belongsToField.value && field?.invalid.value) || attrs['aria-invalid'] === true || attrs['aria-invalid'] === 'true',
  ))
  const describedBy = computed(() => [
    typeof attrs['aria-describedby'] === 'string' ? attrs['aria-describedby'] : undefined,
    belongsToField.value ? field?.describedBy.value : undefined,
  ].filter(Boolean).join(' ') || undefined)
  const required = computed(() => Boolean((belongsToField.value && field?.required.value) || attrs.required === '' || attrs.required))
  const ariaRequired = computed(() => attrs['aria-required'] === undefined
    ? required.value || undefined
    : attrs['aria-required'] === true || attrs['aria-required'] === 'true')
  const rootAttrs = computed(() => ({ class: attrs.class, style: attrs.style }))
  const controlAttrs = computed(() => Object.fromEntries(Object.entries(attrs).filter(([key]) => ![
    'class',
    'style',
    'id',
    'aria-describedby',
    'aria-invalid',
    'required',
    'aria-required',
  ].includes(key))))

  const validate = async (trigger: FormValidationTrigger) => {
    if (field?.isResetting.value)
      return
    // 等待 v-model 回传到表单，避免校验到控件变更前的值
    await nextTick()
    void field?.validate(trigger).catch(() => undefined)
  }
  return { controlId, labelledBy, invalid, describedBy, required, ariaRequired, rootAttrs, controlAttrs, validate }
}
