import type { ComponentSize } from './types'

export type SelectSize = ComponentSize
export type SelectOptionValue = string | number
export interface SelectOption<Value extends SelectOptionValue = string> {
  label: string
  value: Value
  description?: string
  disabled?: boolean
}
export type SelectValue<Multiple extends boolean, Value extends SelectOptionValue = string> = Multiple extends true ? Value[] : Value | undefined

export interface SelectProps<Multiple extends boolean = false, Value extends SelectOptionValue = string> {
  options: SelectOption<Value>[]
  size?: ComponentSize
  disabled?: boolean
  placeholder?: string
  emptyText?: string
  multiple?: Multiple & boolean
  filterable?: boolean
  clearable?: boolean
  remote?: boolean
  collapseTags?: boolean
  collapseTagsTooltip?: boolean
  maxCollapseTags?: number
  loading?: boolean
}
export interface SelectInstance {
  focus: () => void
  blur: () => void
  clear: () => void
  open: () => Promise<void>
  close: () => void
}

export interface SelectEmits<Multiple extends boolean = false, Value extends SelectOptionValue = string> {
  change: [value: SelectValue<Multiple, Value>]
  clear: []
  removeTag: [value: Value]
  visibleChange: [visible: boolean]
  queryChange: [query: string]
  focus: [event: FocusEvent]
  blur: [event: FocusEvent]
}

export interface SelectSlots<Value extends SelectOptionValue = string> {
  option?: (props: { option: SelectOption<Value>, index: number, selected: boolean, disabled: boolean }) => unknown
  empty?: (props: { search: string }) => unknown
}
