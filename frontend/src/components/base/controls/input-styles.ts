import type { ComponentSize } from './types'

// 单行高度由主题决定，文字沿用输入框的内缩，标签块与步进按钮使用四周等距的留白
const sizeClasses: Record<ComponentSize, string> = {
  sm: '[--field-inline:0.75rem] [--field-row:1.25rem] [--field-height:var(--cp-control-height-sm)] text-xs',
  md: '[--field-inline:0.875rem] [--field-row:1.5rem] [--field-height:var(--cp-control-height)] text-cp',
  lg: '[--field-inline:1rem] [--field-row:1.5rem] [--field-height:var(--cp-control-height-lg)] text-cp-lg',
}

export function getInputClasses(size: ComponentSize, disabled: boolean, invalid: boolean, { open = false, inset = 'text', trailing = false }: { open?: boolean, inset?: 'text' | 'item', trailing?: boolean } = {}) {
  return [
    sizeClasses[size],
    'min-h-(--field-height) min-w-0 rounded-cp border-0 [--field-pad:max(2px,calc((var(--field-height)-var(--field-row))/2))] py-(--field-pad) outline-none transition-[background-color,box-shadow,color] duration-160 motion-reduce:transition-none',
    inset === 'item' ? 'ps-(--field-pad)' : 'ps-(--field-inline)',
    trailing || inset === 'item' ? 'pe-(--field-pad)' : 'pe-(--field-inline)',
    disabled
      ? 'cursor-not-allowed bg-cp-bg-container-disabled text-cp-text-disabled shadow-none'
      : invalid
        ? [
            'text-cp-text',
            open
              ? 'bg-(--cp-input-error-active-bg) shadow-cp-input-error-active'
              : 'bg-cp-error-container shadow-cp-input hover:not-focus-within:bg-(--cp-input-error-active-bg) hover:not-focus-within:shadow-cp-input-error-active focus-within:bg-(--cp-input-error-active-bg) focus-within:shadow-cp-input-error-active',
          ]
        : open
          ? 'bg-(--cp-input-active-bg) text-cp-text shadow-cp-input-active'
          : 'bg-(--cp-input-bg) text-cp-text shadow-cp-input hover:not-focus-within:bg-(--cp-input-hover-bg) hover:not-focus-within:shadow-cp-input-hover focus-within:bg-(--cp-input-active-bg) focus-within:shadow-cp-input-active',
  ]
}

export const inputTagClass = 'bg-(--cp-input-tag-bg)!'

export const inputFieldClass = 'h-(--field-row) min-w-0 flex-1 border-0 bg-transparent font-emphasis leading-[1.15] text-cp-text outline-0 placeholder:font-normal placeholder:text-cp-text-quaternary disabled:cursor-not-allowed disabled:text-cp-text-disabled [&[type=number]]:appearance-[textfield] [&[type=number]::-webkit-inner-spin-button]:m-0 [&[type=number]::-webkit-inner-spin-button]:appearance-none [&[type=number]::-webkit-outer-spin-button]:m-0 [&[type=number]::-webkit-outer-spin-button]:appearance-none'
