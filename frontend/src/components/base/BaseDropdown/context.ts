import type { InjectionKey } from 'vue'

export const dropdownSelectKey: InjectionKey<() => void> = Symbol('dropdownSelect')
