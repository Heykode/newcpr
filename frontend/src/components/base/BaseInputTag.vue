<script setup lang="ts">
import type { ComponentSize } from './controls/types'
import { X } from '@lucide/vue'
import { moveArrayElement, useSortable } from '@vueuse/integrations/useSortable'
import { computed, nextTick, shallowRef, useAttrs, useTemplateRef } from 'vue'
import { useFormField } from './BaseForm/composables/useFormField'
import Tag from './BaseTag.vue'
import { clearButtonClass } from './controls/clear-button'
import { getInputClasses, inputTagClass } from './controls/input-styles'
import { useLocale } from './controls/locale'
import TagOverflow from './controls/TagOverflow.vue'
import { useTagFit } from './controls/useTagFit'

defineOptions({ inheritAttrs: false })

const props = withDefaults(defineProps<{
  size?: ComponentSize
  placeholder?: string
  max?: number
  disabled?: boolean
  readonly?: boolean
  clearable?: boolean
  collapseTags?: boolean
  collapseTagsTooltip?: boolean
  maxCollapseTags?: number
  draggable?: boolean
  trigger?: 'Enter' | 'Space'
  saveOnBlur?: boolean
}>(), { size: 'md', saveOnBlur: true, trigger: 'Enter' })

const emit = defineEmits<{
  change: [tags: string[]]
  addTag: [tag: string]
  removeTag: [tag: string, index: number]
  dragTag: [oldIndex: number, newIndex: number, position: 'before' | 'after']
  clear: []
  focus: [event: FocusEvent]
  blur: [event: FocusEvent]
}>()
defineSlots<{ prefix?: () => unknown, suffix?: () => unknown }>()
const model = defineModel<string[]>({ default: () => [] })
const locale = useLocale()
const { controlId, invalid, describedBy, required, ariaRequired, rootAttrs, controlAttrs, validate } = useFormField(useAttrs())
const input = useTemplateRef<HTMLInputElement>('input')
const draft = shallowRef('')
const editable = computed(() => !props.disabled && !props.readonly)
const full = computed(() => props.max !== undefined && model.value.length >= props.max)

const tagSize = computed(() => props.size === 'sm' ? 'sm' : 'md')
const list = useTemplateRef<HTMLElement>('list')
const samples = useTemplateRef<HTMLElement>('samples')
const candidates = computed(() => props.collapseTags ? model.value.slice(0, Math.max(1, props.maxCollapseTags ?? model.value.length)) : [])
const fit = useTagFit(list, samples, candidates, () => model.value.length)
const visible = computed(() => props.collapseTags ? candidates.value.slice(0, fit.value) : model.value)
const hidden = computed(() => model.value.slice(visible.value.length).map((label, i) => ({ label, value: visible.value.length + i })))
const canDrag = computed(() => props.draggable && editable.value && visible.value.length > 1)
const orderedTags = computed({ get: () => model.value, set: update })
useSortable(computed(() => canDrag.value ? list.value : null), orderedTags, {
  watchElement: true,
  draggable: '[data-input-tag]',
  filter: 'button,input',
  preventOnFilter: false,
  ghostClass: 'opacity-40',
  onMove: event => event.related.hasAttribute('data-input-tag'),
  onUpdate(event) {
    const { oldIndex, newIndex } = event
    if (oldIndex === undefined || newIndex === undefined || oldIndex === newIndex)
      return
    // 排序库先改 DOM，交回 Vue 后再通知使用方，隐藏项仍保留在数组末尾
    moveArrayElement(orderedTags, oldIndex, newIndex, event)
    void nextTick(() => emit('dragTag', oldIndex, newIndex, oldIndex < newIndex ? 'after' : 'before'))
  },
})

function focus() {
  input.value?.focus()
}

function focusFromSurface(event: MouseEvent) {
  if (event.target === event.currentTarget || event.target === list.value) {
    event.preventDefault()
    focus()
  }
}

function update(tags: string[]) {
  model.value = tags
  emit('change', tags)
  void validate('change')
}

function commit() {
  const tag = draft.value.trim()
  if (!editable.value || full.value || !tag)
    return
  update([...model.value, tag])
  draft.value = ''
  emit('addTag', tag)
}

function remove(index: number, restoreFocus = true) {
  const tag = model.value[index]
  if (!editable.value || tag === undefined)
    return
  if (restoreFocus)
    focus()
  update(model.value.filter((_, i) => i !== index))
  emit('removeTag', tag, index)
}

function clear() {
  if (!editable.value)
    return
  focus()
  draft.value = ''
  update([])
  emit('clear')
}

function handleKeydown(event: KeyboardEvent) {
  // 部分输入法先结束组合再派发确认键，此时 keyCode 仍为 229
  if (!editable.value || event.isComposing || event.keyCode === 229)
    return
  if (event.key === (props.trigger === 'Space' ? ' ' : 'Enter')) {
    event.preventDefault()
    event.stopPropagation()
    commit()
  }
  else if (event.key === 'Backspace' && !draft.value && model.value.length) {
    event.preventDefault()
    remove(model.value.length - 1)
  }
}

function handleFocus(event: FocusEvent) {
  // 删除和清空按钮也属于控件，内部切换焦点不提交草稿
  if (event.relatedTarget instanceof Node && (event.currentTarget as HTMLElement).contains(event.relatedTarget))
    return
  if (event.type === 'focusin') {
    emit('focus', event)
    return
  }
  if (props.saveOnBlur)
    commit()
  draft.value = ''
  emit('blur', event)
  void validate('blur')
}

defineExpose({ focus, commit, clear })
</script>

<template>
  <!-- eslint-disable-next-line vue-a11y/no-static-element-interactions -- 点击控件留白将焦点交给原生输入框，键盘直接访问输入框 -->
  <div
    v-bind="rootAttrs"
    class="cp-input-tag group/input relative inline-flex items-center gap-2"
    :class="getInputClasses(size, disabled, invalid, { inset: model.length && !$slots.prefix ? 'item' : 'text', trailing: Boolean(clearable && editable && (model.length || draft)) })"
    @mousedown="focusFromSurface"
    @focusin="handleFocus"
    @focusout="handleFocus"
  >
    <span v-if="$slots.prefix" class="inline-flex shrink-0 text-cp-text-quaternary"><slot name="prefix" /></span>
    <div ref="list" class="flex min-w-0 flex-1 items-center gap-1" :class="collapseTags ? 'flex-nowrap' : 'flex-wrap'">
      <Tag
        v-for="(tag, index) in visible"
        :key="index"
        data-input-tag
        :class="[inputTagClass, collapseTags && 'shrink-0 whitespace-nowrap', canDrag && 'cursor-grab select-none active:cursor-grabbing']"
        :size="tagSize"
        :closable="editable"
        :disabled="disabled"
        @close="remove(index)"
      >
        {{ tag }}
      </Tag>
      <TagOverflow
        v-if="hidden.length"
        :items="hidden"
        :size="tagSize"
        :tooltip="collapseTagsTooltip"
        :disabled="disabled"
        :readonly="readonly"
        :teleported="false"
        :more-label="locale.input.moreTags?.(hidden.length) ?? locale.select.more(hidden.length)"
        :panel-label="locale.input.remainingTags ?? locale.select.remaining"
        @remove="remove($event, false)"
        @focus-control="focus"
      />
      <input
        v-bind="controlAttrs"
        :id="controlId"
        ref="input"
        v-model="draft"
        type="text"
        class="h-(--field-row) min-w-0 flex-[1_1_5ch] border-0 bg-transparent p-0 text-inherit outline-none placeholder:text-cp-text-quaternary disabled:cursor-not-allowed"
        :disabled="disabled"
        :readonly="readonly || full"
        :required="required && !model.length"
        :aria-required="ariaRequired"
        :aria-invalid="invalid || undefined"
        :aria-describedby="describedBy"
        :placeholder="model.length ? undefined : placeholder"
        @keydown="handleKeydown"
      >
    </div>
    <!-- 限制测量区的布局范围，不让隐藏标签撑宽页面或参与键盘交互。 -->
    <div v-if="collapseTags && model.length" inert aria-hidden="true" class="pointer-events-none invisible absolute size-0 overflow-hidden">
      <div ref="samples" class="flex w-max items-center whitespace-nowrap">
        <div class="flex w-max">
          <Tag v-for="(tag, index) in candidates" :key="index" :size="tagSize" :closable="editable" :disabled="disabled">
            {{ tag }}
          </Tag>
        </div>
        <div class="flex w-max tabular-nums">
          <Tag v-for="digits in String(model.length).length" :key="digits" :size="tagSize">
            +{{ '0'.repeat(digits) }}
          </Tag>
        </div>
        <span class="w-[5ch]" />
      </div>
    </div>
    <span v-if="$slots.suffix" class="inline-flex shrink-0 text-cp-text-quaternary"><slot name="suffix" /></span>
    <button
      v-if="clearable && editable && (model.length || draft)"
      type="button"
      :class="clearButtonClass"
      class="pointer-events-none opacity-0 group-focus-within/input:pointer-events-auto group-focus-within/input:opacity-100 group-hover/input:pointer-events-auto group-hover/input:opacity-100"
      :aria-label="locale.input.clear"
      @mousedown.prevent
      @click="clear"
    >
      <X :size="14" />
    </button>
  </div>
</template>
