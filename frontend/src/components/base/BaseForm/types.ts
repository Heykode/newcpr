import type { RuleItem, ValidateError, ValidateFieldsError } from '@rc-component/async-validator'
import type FormItem from './FormItem.vue'
import type Form from './index.vue'

export type FormValidationTrigger = string
export type FormLabelPosition = 'left' | 'right' | 'top'
export type FormItemProp = string | string[]
export type FormItemValidateState = '' | 'validating' | 'success' | 'error'
export type FormValidationResult = Promise<boolean>
export type FormValidateCallback = (isValid: boolean, invalidFields?: ValidateFieldsError) => void | Promise<void>
export interface FormValidateFailure {
  errors: ValidateError[] | null
  fields: ValidateFieldsError
}

export interface FormRule extends RuleItem {
  trigger?: FormValidationTrigger | FormValidationTrigger[]
}

type FormFieldPath<T> = T extends Date | RegExp | File | FileList | Blob
  ? never
  : T extends readonly (infer Item)[]
    ? `${number}` | `${number}.${FormFieldPath<Item>}`
    : T extends object
      ? { [Key in keyof T & (string | number)]: `${Key}` | `${Key}.${FormFieldPath<T[Key]>}` }[keyof T & (string | number)]
      : never

export type FormRules<T extends object = Record<string, unknown>> = Partial<Record<FormFieldPath<T>, FormRule | FormRule[]>>

export interface FormItemContext {
  $el: HTMLElement | undefined
  prop: FormItemProp | undefined
  propString: string
  fieldValue: unknown
  validateState: FormItemValidateState
  validateMessage: string
  validate: (trigger?: string, callback?: FormValidateCallback) => FormValidationResult
  resetField: () => Promise<void>
  clearValidate: () => void
  setInitialValue: (value: unknown) => void
  getInitialValue: () => unknown
  focus: () => void
  scrollIntoView: (options?: boolean | ScrollIntoViewOptions) => void
}

export interface FormInstance {
  validate: (callback?: FormValidateCallback) => FormValidationResult
  validateField: (props?: FormItemProp | FormItemProp[], callback?: FormValidateCallback) => FormValidationResult
  resetFields: (props?: FormItemProp | FormItemProp[]) => void
  clearValidate: (props?: FormItemProp | FormItemProp[]) => void
  scrollToField: (prop: FormItemProp) => void
  focusField: (prop: FormItemProp) => void
  fields: FormItemContext[]
  getField: (prop: FormItemProp) => FormItemContext | undefined
  setInitialValues: (model: object) => void
}
export type FormProps = InstanceType<typeof Form>['$props']
export type FormSlots = InstanceType<typeof Form>['$slots']
export type FormItemProps = InstanceType<typeof FormItem>['$props']
export type FormItemSlots = InstanceType<typeof FormItem>['$slots']
export type FormItemInstance = InstanceType<typeof FormItem>
