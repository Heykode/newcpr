<script setup lang="ts" generic="Multiple extends boolean = false, Value extends string | number = string">
import type { SelectEmits, SelectOption, SelectProps, SelectSize, SelectSlots, SelectValue } from './controls/select-types'
import { ChevronDown, LoaderCircle, X } from '@lucide/vue'
import { useEventListener } from '@vueuse/core'
import { computed, provide, useAttrs, useTemplateRef, watch } from 'vue'
import Empty from './BaseEmpty.vue'
import { useFormField } from './BaseForm/composables/useFormField'
import { formFieldKey } from './BaseForm/context'
import Scrollbar from './BaseScrollbar.vue'
import { clearButtonClass } from './controls/clear-button'
import { getInputClasses, inputFieldClass } from './controls/input-styles'
import { useLocale } from './controls/locale'
import SelectTags from './controls/SelectTags.vue'
import { useSelect } from './controls/useSelect'

defineOptions({
  inheritAttrs: false,
})

const props = withDefaults(
  defineProps<SelectProps<Multiple, Value>>(),
  {
    size: 'md',
    disabled: false,
    maxCollapseTags: 1,
  },
)

const emit = defineEmits<SelectEmits<Multiple, Value>>()

defineSlots<SelectSlots<Value>>()

const locale = useLocale()

const model = defineModel<SelectValue<Multiple, Value>>({ required: true })
const attrs = useAttrs()
const rootRef = useTemplateRef<HTMLElement>('rootRef')
const triggerRef = useTemplateRef<HTMLButtonElement | HTMLInputElement>('triggerRef')
const controlRef = useTemplateRef<HTMLElement>('controlRef')
const popoverRef = useTemplateRef<HTMLElement>('popoverRef')
const listboxRef = useTemplateRef<HTMLElement>('listboxRef')
const scrollbarRef = useTemplateRef<InstanceType<typeof Scrollbar>>('scrollbarRef')
const { controlId, invalid, describedBy, ariaRequired, rootAttrs, controlAttrs, validate } = useFormField(attrs)
const {
  selectId,
  open,
  search,
  activeIndex,
  popoverStyle,
  popoverMaxHeight,
  popoverPlacement,
  selectedValues,
  selectedOptions,
  selectionLabel,
  displayed,
  selectedOption,
  optionId,
  toggleMenu,
  openMenu,
  closeMenu,
  handleTriggerKeydown,
  removeOption,
  handleOptionFocus,
  chooseOption,
  handleEscape,
} = useSelect(props, model, { rootRef, triggerRef, controlRef, popoverRef, listboxRef, scrollbarRef }, {
  change(value) {
    emit('change', value)
    validate('change')
  },
  removeTag(value) {
    emit('removeTag', value)
  },
})
// 下拉面板和标签操作属于内部辅助控件，不继承选择字段的身份与校验状态
provide(formFieldKey, null)
const hasTags = computed(() => props.multiple && selectedOptions.value.length > 0)
const canClear = computed(() => props.clearable && selectedValues.value.length > 0 && !props.disabled)
const fieldAttrs = computed(() => ({
  ...controlAttrs.value,
  'disabled': props.disabled,
  'role': 'combobox',
  'aria-expanded': open.value,
  'aria-controls': open.value ? `${selectId}-listbox` : undefined,
  'aria-activedescendant': open.value && activeIndex.value >= 0 ? optionId(activeIndex.value) : undefined,
  'aria-describedby': describedBy.value,
  'aria-invalid': invalid.value || undefined,
  'aria-required': ariaRequired.value,
}) as const)
const query = computed({
  get: () => search.value,
  set(value: string) {
    void openMenu()
    search.value = value
    emit('queryChange', value)
  },
})
useEventListener(controlRef, 'click', (event) => {
  if (!props.filterable || props.disabled || !(event.target instanceof Element) || event.target.closest('button, .cp-popover'))
    return
  triggerRef.value?.focus()
  if (event.target.closest('[data-select-indicator]'))
    toggleMenu()
  else void openMenu()
})
const sizeConfig: Record<SelectSize, { option: string, icon: number }> = {
  sm: { option: 'h-8 px-2.5 text-xs', icon: 14 },
  md: { option: 'h-8.5 px-3 text-cp', icon: 16 },
  lg: { option: 'h-10 px-3.5 text-cp-lg', icon: 17 },
}

function optionClasses(option: SelectOption<Value>, index: number) {
  return [
    'flex w-full shrink-0 touch-manipulation items-center gap-2 rounded-cp-sm border-0 px-3 text-left font-emphasis leading-none outline-none transition-colors motion-reduce:transition-none',
    sizeConfig[props.size].option,
    option.disabled
      ? 'cursor-not-allowed bg-transparent text-cp-text-disabled'
      : selectedValues.value.includes(option.value)
        ? 'cursor-pointer bg-cp-control-item-bg-active text-cp-primary-text'
        : activeIndex.value === index
          ? 'cursor-pointer bg-cp-bg-text-hover text-cp-text'
          : 'cursor-pointer bg-transparent text-cp-text hover:bg-cp-bg-text-hover',
  ]
}
watch(open, visible => emit('visibleChange', visible))
function clear() {
  const value = (props.multiple ? [] : undefined) as SelectValue<Multiple, Value>
  model.value = value
  emit('change', value)
  emit('clear')
  validate('change')
  triggerRef.value?.focus()
}
defineExpose({ focus: () => triggerRef.value?.focus(), blur: () => triggerRef.value?.blur(), clear, open: openMenu, close: closeMenu })
</script>

<template>
  <div ref="rootRef" class="cp-select relative inline-block min-w-0 text-left" v-bind="rootAttrs">
    <div
      ref="controlRef"
      class="group/select relative flex w-full items-center gap-2 text-left font-emphasis leading-none"
      :class="getInputClasses(size, disabled, invalid, { open, inset: hasTags ? 'item' : 'text', trailing: true })"
      :style="{ anchorName: `--${selectId}` }"
    >
      <button
        v-if="!filterable"
        v-bind="fieldAttrs"
        :id="controlId"
        ref="triggerRef"
        type="button"
        class="absolute inset-0 size-full cursor-pointer rounded-cp border-0 bg-transparent p-0 text-inherit outline-none disabled:cursor-not-allowed"
        @click="toggleMenu"
        @focus="emit('focus', $event)"
        @blur="emit('blur', $event); validate('blur')"
        @keydown="handleTriggerKeydown"
      >
        <span class="sr-only">{{ selectionLabel || (placeholder ?? locale.select.placeholder) }}</span>
      </button>
      <div class="pointer-events-none relative flex min-w-0 flex-1 flex-wrap items-center gap-1">
        <SelectTags
          v-if="hasTags"
          :options="selectedOptions"
          :size="size === 'sm' ? 'sm' : 'md'"
          :disabled="disabled"
          :collapse="collapseTags"
          :tooltip="collapseTagsTooltip"
          :limit="maxCollapseTags"
          :menu-open="open"
          @remove="removeOption"
          @focus-control="triggerRef?.focus()"
        />
        <input
          v-if="filterable"
          v-bind="fieldAttrs"
          :id="controlId"
          ref="triggerRef"
          v-model="query"
          type="text"
          autocomplete="off"
          aria-autocomplete="list"
          :placeholder="hasTags ? undefined : selectedOption?.label ?? placeholder ?? locale.select.placeholder"
          class="pointer-events-auto w-0 min-w-[3ch] flex-[1_1_3ch] truncate p-0 placeholder:font-emphasis"
          :class="[inputFieldClass, selectedOption && !open && !disabled && 'placeholder:text-cp-text!', disabled && 'placeholder:text-cp-text-disabled!']"
          @focus="emit('focus', $event)"
          @blur="emit('blur', $event); validate('blur')"
          @keydown="handleTriggerKeydown"
        >
        <span
          v-else-if="!hasTags"
          aria-hidden="true"
          class="min-w-0 truncate"
          :class="[selectedOption?.description ? 'max-w-1/2 shrink-0' : 'flex-1', !disabled && !selectedOption && 'text-cp-text-quaternary']"
        >
          {{ selectedOption?.label ?? placeholder ?? locale.select.placeholder }}
        </span>
        <span
          v-if="selectedOption?.description && !(filterable && open)"
          :title="selectedOption.description"
          class="min-w-0 flex-1 truncate font-normal"
          :class="disabled ? 'text-cp-text-disabled' : 'text-cp-text-tertiary'"
        >
          {{ selectedOption.description }}
        </span>
      </div>
      <span
        data-select-indicator
        class="relative inline-grid size-(--field-row) shrink-0 place-items-center"
        :class="filterable && !disabled ? 'cursor-pointer' : 'pointer-events-none'"
      >
        <ChevronDown
          aria-hidden="true"
          class="pointer-events-none transition-transform motion-reduce:transition-none"
          :class="[disabled ? 'text-cp-text-disabled' : open ? 'rotate-180 text-cp-primary-text' : 'text-cp-text-quaternary', canClear && !open && 'group-focus-within/select:opacity-0 group-hover/select:opacity-0']"
          :size="sizeConfig[size].icon"
        />
        <button
          v-if="canClear && !open"
          type="button"
          :aria-label="locale.select.clear"
          :class="clearButtonClass"
          class="pointer-events-none absolute inset-0 opacity-0 group-focus-within/select:pointer-events-auto group-focus-within/select:opacity-100 group-hover/select:pointer-events-auto group-hover/select:opacity-100"
          @mousedown.prevent
          @click.stop="clear"
        >
          <X :size="14" />
        </button>
      </span>
    </div>

    <Teleport to="body">
      <Transition
        name="cp-slide-fade"
      >
        <div
          v-if="open"
          ref="popoverRef"
          data-cp-overlay
          :data-cp-placement="popoverPlacement"
          class="fixed z-50 rounded-cp-lg border-0 bg-cp-bg-elevated shadow-cp-popover"
          :style="popoverStyle"
        >
          <Scrollbar ref="scrollbarRef" :max-height="popoverMaxHeight" class="rounded-cp-lg">
            <div
              :id="`${selectId}-listbox`"
              ref="listboxRef"
              class="flex flex-col gap-1 p-1"
              role="listbox"
              :aria-labelledby="controlId"
              :aria-multiselectable="multiple || undefined"
              :aria-busy="loading || undefined"
            >
              <div v-if="displayed.length === 0" class="shrink-0">
                <slot name="empty" :search="search">
                  <Empty :title="loading ? locale.common.loading : search ? locale.select.noMatches : (emptyText ?? locale.select.empty)" size="sm" surface="none" role="status">
                    <template v-if="loading" #icon>
                      <LoaderCircle :size="18" class="animate-spin text-cp-text-quaternary motion-reduce:animate-none" />
                    </template>
                  </Empty>
                </slot>
              </div>

              <template v-else>
                <button
                  v-for="(option, index) in displayed"
                  :id="optionId(index)"
                  :key="option.value"
                  type="button"
                  role="option"
                  :aria-selected="selectedValues.includes(option.value)"
                  tabindex="-1"
                  :disabled="option.disabled"
                  :class="optionClasses(option, index)"
                  @mouseenter="activeIndex = option.disabled ? activeIndex : index"
                  @focus="handleOptionFocus(index)"
                  @mousedown.prevent
                  @click="chooseOption(option, index)"
                  @keydown.esc="handleEscape"
                >
                  <slot name="option" :option="option" :index="index" :selected="selectedValues.includes(option.value)" :disabled="Boolean(option.disabled)">
                    <span class="min-w-0 truncate" :class="option.description ? 'max-w-1/2 shrink-0' : 'flex-1'">{{ option.label }}</span>
                    <span
                      v-if="option.description"
                      :title="option.description"
                      class="min-w-0 flex-1 truncate font-normal"
                      :class="option.disabled ? 'text-cp-text-disabled' : 'text-cp-text-tertiary'"
                    >
                      {{ option.description }}
                    </span>
                  </slot>
                </button>
              </template>
            </div>
          </Scrollbar>
        </div>
      </Transition>
    </Teleport>
  </div>
</template>
