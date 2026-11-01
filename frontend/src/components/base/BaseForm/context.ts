import type { ComputedRef, InjectionKey, Ref } from 'vue'
import type { FormInstance, FormItemContext, FormItemProp, FormLabelPosition, FormRules } from './types'

export interface FormContext extends FormInstance {
  model: ComputedRef<object | undefined>
  rules: ComputedRef<FormRules>
  labelPosition: ComputedRef<FormLabelPosition>
  labelWidth: ComputedRef<string | number>
  autoLabelWidth: ComputedRef<number>
  showMessage: ComputedRef<boolean>
  register: (field: FormItemContext) => () => void
  registerLabelWidth: (width: Readonly<Ref<number>>) => () => void
  getInitialValue: (path: string) => unknown
  setInitialValue: (path: string, value: unknown) => void
  emitValidate: (prop: FormItemProp, valid: boolean, message: string) => void
}

interface FormFieldContext {
  controlId: ComputedRef<string | undefined>
  labelledBy: ComputedRef<string | undefined>
  describedBy: ComputedRef<string | undefined>
  invalid: ComputedRef<boolean>
  required: ComputedRef<boolean>
  isResetting: Readonly<Ref<boolean>>
  validate: FormItemContext['validate']
  registerControl: (id: Readonly<Ref<string>>) => () => void
}

export const formFieldKey: InjectionKey<FormFieldContext | null> = Symbol('form-field')
export const formKey: InjectionKey<FormContext> = Symbol('form')
