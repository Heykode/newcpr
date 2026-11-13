import type { Ref } from 'vue'
import type Scrollbar from '../BaseScrollbar.vue'
import type { SelectOption, SelectOptionValue, SelectProps, SelectValue } from './select-types'
import type { SelectPositionElements } from './useSelectPosition'
import { onClickOutside } from '@vueuse/core'
import { computed, nextTick, ref, useId, watch } from 'vue'
import { useLocale } from './locale'
import { useSelectPosition } from './useSelectPosition'

interface SelectElements extends SelectPositionElements {
  rootRef: Ref<HTMLElement | null>
  triggerRef: Ref<HTMLButtonElement | HTMLInputElement | null>
  popoverRef: Ref<HTMLElement | null>
  scrollbarRef: Ref<InstanceType<typeof Scrollbar> | null>
}

export function useSelect<Multiple extends boolean, Value extends SelectOptionValue>(
  props: Readonly<SelectProps<Multiple, Value>>,
  model: Ref<SelectValue<Multiple, Value>>,
  elements: SelectElements,
  callbacks: { change: (value: SelectValue<Multiple, Value>) => void, removeTag: (value: Value) => void },
) {
  const { rootRef, triggerRef, popoverRef, listboxRef, scrollbarRef } = elements
  const search = ref('')
  const open = ref(false)
  const activeIndex = ref(-1)
  const selectId = `cp-select-${useId()}`
  const {
    popoverStyle,
    popoverMaxHeight,
    popoverPlacement,
    updatePopoverPosition,
  } = useSelectPosition(open, selectId, elements)

  const selectedValues = computed<Value[]>(() => Array.isArray(model.value) ? model.value as Value[] : model.value === undefined ? [] : [model.value as Value])
  const selectedOptions = computed(() => selectedValues.value.map(value => props.options.find(option => option.value === value) ?? { value, label: String(value) }))
  const locale = useLocale()
  const selectionLabel = computed(() => new Intl.ListFormat(locale.value.name).format(selectedOptions.value.map(option => option.label)))
  const displayed = computed(() => props.loading
    ? []
    : props.options.filter(option =>
        props.remote || !props.filterable || `${option.label} ${option.description ?? ''}`.toLocaleLowerCase().includes(search.value.toLocaleLowerCase()),
      ))
  const selectedOption = computed(() => props.options.find(option => option.value === model.value))

  function optionId(index: number) {
    return `${selectId}-option-${index}`
  }

  function enabledIndexes() {
    return displayed.value.flatMap((option, index) => (option.disabled ? [] : [index]))
  }

  function selectedIndex() {
    return displayed.value.findIndex(option => selectedValues.value.includes(option.value))
  }

  function setActiveToSelected() {
    const selected = selectedIndex()
    if (selected >= 0 && !displayed.value[selected]?.disabled) {
      activeIndex.value = selected
      return
    }

    activeIndex.value = enabledIndexes()[0] ?? -1
  }

  function scrollActiveIntoView() {
    const wrap = scrollbarRef.value?.wrapRef
    const option = listboxRef.value?.children[activeIndex.value]
    if (!open.value || !wrap || !option)
      return

    // 只滚动菜单自身，避免键盘定位选项时带动弹窗或页面滚动。
    const wrapRect = wrap.getBoundingClientRect()
    const optionRect = option.getBoundingClientRect()
    if (optionRect.top < wrapRect.top)
      wrap.scrollTop -= wrapRect.top - optionRect.top
    else if (optionRect.bottom > wrapRect.bottom)
      wrap.scrollTop += optionRect.bottom - wrapRect.bottom
  }

  async function openMenu() {
    if (props.disabled || open.value)
      return

    open.value = true
    if (props.filterable)
      triggerRef.value?.focus()
    setActiveToSelected()
    await nextTick()
    updatePopoverPosition()
    await nextTick()
    scrollActiveIntoView()
  }

  function closeMenu() {
    open.value = false
    search.value = ''
  }

  function toggleMenu() {
    if (open.value) {
      closeMenu()
      return
    }

    void openMenu()
  }

  function moveActive(delta: number) {
    const indexes = enabledIndexes()
    if (indexes.length === 0)
      return

    const current = indexes.indexOf(activeIndex.value)
    const next = current === -1 ? (delta > 0 ? 0 : indexes.length - 1) : current + delta
    activeIndex.value = indexes[(next + indexes.length) % indexes.length]
    // 只有键盘导航主动定位；鼠标悬停不能把半露出的选项滚入视口。
    scrollActiveIntoView()
  }

  function handleOptionFocus(index: number) {
    const option = displayed.value[index]
    if (!option || option.disabled)
      return

    activeIndex.value = index
    scrollActiveIntoView()
  }

  function chooseOption(option: SelectOption<Value>, index: number) {
    if (props.disabled || option.disabled)
      return

    const value = (props.multiple
      ? selectedValues.value.includes(option.value)
        ? selectedValues.value.filter(value => value !== option.value)
        : [...selectedValues.value, option.value]
      : option.value) as SelectValue<Multiple, Value>
    model.value = value
    callbacks.change(value)
    activeIndex.value = index
    if (!props.multiple) {
      closeMenu()
      triggerRef.value?.focus()
    }
  }

  function chooseActive() {
    const option = displayed.value[activeIndex.value]
    if (!option)
      return

    chooseOption(option, activeIndex.value)
  }

  function removeOption(value: Value) {
    if (props.disabled || props.options.find(option => option.value === value)?.disabled)
      return
    const next = selectedValues.value.filter(item => item !== value) as SelectValue<Multiple, Value>
    model.value = next
    callbacks.change(next)
    callbacks.removeTag(value)
  }

  function handleEscape(event: KeyboardEvent) {
    if (!open.value)
      return

    // 先关闭下拉层，避免同一次按键继续关闭外层弹窗。
    event.preventDefault()
    event.stopPropagation()
    closeMenu()
    triggerRef.value?.focus()
  }

  function handleTriggerKeydown(event: KeyboardEvent) {
    if (props.disabled || event.isComposing || event.keyCode === 229)
      return

    if (event.key === 'ArrowDown') {
      event.preventDefault()
      if (!open.value) {
        void openMenu()
        return
      }
      moveActive(1)
      return
    }

    if (event.key === 'ArrowUp') {
      event.preventDefault()
      if (!open.value) {
        void openMenu()
        return
      }
      moveActive(-1)
      return
    }

    const searching = event.target instanceof HTMLInputElement
    if (event.key === 'Enter' || (event.key === ' ' && !searching)) {
      event.preventDefault()
      if (!open.value) {
        void openMenu()
        return
      }
      chooseActive()
      return
    }

    if (event.key === 'Escape')
      handleEscape(event)
    if (event.key === 'Tab')
      closeMenu()
  }

  watch(
    () => [displayed.value, props.size],
    async () => {
      if (!open.value)
        return
      setActiveToSelected()
      await nextTick()
      updatePopoverPosition()
      await nextTick()
      scrollActiveIntoView()
    },
  )

  watch(model, async () => {
    await nextTick()
    updatePopoverPosition()
  })
  watch(() => props.loading, async (loading) => {
    if (loading || !open.value)
      return
    await nextTick()
    // 重试操作结束后其按钮会卸载，将键盘焦点交还给仍存在的选择控件。
    if (open.value && (document.activeElement === document.body || popoverRef.value?.contains(document.activeElement)))
      triggerRef.value?.focus()
  })
  watch(() => props.disabled, (disabled) => {
    if (disabled)
      closeMenu()
  })

  onClickOutside(rootRef, closeMenu, { ignore: [popoverRef] })

  return {
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
  }
}
