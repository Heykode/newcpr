<script setup lang="ts" generic="Value extends string | number">
import type { SelectOption } from './select-types'
import { computed, nextTick } from 'vue'
import Tag from '../BaseTag.vue'
import { inputTagClass } from './input-styles'
import { useLocale } from './locale'
import TagOverflow from './TagOverflow.vue'

const props = defineProps<{
  options: SelectOption<Value>[]
  size: 'sm' | 'md'
  disabled?: boolean
  collapse?: boolean
  tooltip?: boolean
  limit: number
  menuOpen: boolean
}>()

const emit = defineEmits<{ remove: [value: Value], focusControl: [] }>()

const locale = useLocale()

const visible = computed(() => props.collapse ? props.options.slice(0, Math.max(1, props.limit)) : props.options)
const hidden = computed(() => props.options.slice(visible.value.length))

async function remove(value: Value, event: MouseEvent) {
  emit('remove', value)
  if (event.detail === 0) {
    await nextTick()
    emit('focusControl')
  }
}
</script>

<template>
  <div class="contents">
    <Tag
      v-for="option in visible"
      :key="option.value"
      :size="size"
      :closable="!disabled && !option.disabled"
      :disabled="disabled"
      :close-label="locale.select.remove(option.label)"
      class="min-w-0 [&_button]:pointer-events-auto"
      :class="inputTagClass"
      @close="remove(option.value, $event)"
    >
      <span class="block max-w-48 truncate" :title="option.label">{{ option.label }}</span>
    </Tag>
    <TagOverflow
      v-if="hidden.length"
      :items="hidden"
      :size="size"
      :tooltip="Boolean(tooltip)"
      :disabled="disabled"
      :blocked="menuOpen"
      :more-label="locale.select.more(hidden.length)"
      :panel-label="locale.select.remaining"
      :close-label="locale.select.remove"
      :fallback-title="hidden.map(option => option.label).join('、')"
      @remove="emit('remove', $event)"
      @focus-control="emit('focusControl')"
    />
  </div>
</template>
