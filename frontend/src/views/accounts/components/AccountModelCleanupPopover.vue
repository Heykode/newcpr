<script setup lang="ts">
import { BrushCleaning } from '@lucide/vue'
import { useEventListener } from '@vueuse/core'
import { computed, nextTick, ref, useId, watch } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BasePopover from '@/components/base/BasePopover.vue'

const props = defineProps<{ models: string[], modelCount: number, disabled: boolean }>()
const emit = defineEmits<{ remove: [ids: string[]], closed: [] }>()
const open = ref(false)
const content = ref<HTMLDivElement>()
const trigger = ref<InstanceType<typeof BaseIconButton>>()
const id = useId()
const selected = ref<string[]>([])
const selectedModels = computed(() => props.models.filter(id => selected.value.includes(id)))
const wouldEmpty = computed(() => selectedModels.value.length === props.modelCount)

watch(open, async (opened) => {
  selected.value = []
  await nextTick()
  if (opened) {
    content.value?.querySelector<HTMLInputElement>('input')?.focus()
  }
  else {
    if (!props.disabled)
      (trigger.value?.$el as HTMLElement | undefined)?.focus()
    emit('closed')
  }
})
watch([() => props.models, () => props.disabled], () => {
  open.value = false
})

function select(id: string, checked: boolean) {
  selected.value = checked ? [...selected.value, id] : selected.value.filter(value => value !== id)
}
function remove() {
  if (props.disabled || !selectedModels.value.length || wouldEmpty.value)
    return
  emit('remove', selectedModels.value)
  open.value = false
}
function keydown(event: KeyboardEvent) {
  if (!open.value)
    return
  if (event.key === 'Escape') {
    event.preventDefault()
    event.stopPropagation()
    open.value = false
  }
  if (event.key === 'Tab' && content.value?.contains(event.target as Node)) {
    const controls = [...content.value.querySelectorAll<HTMLElement>('input:not(:disabled), button:not(:disabled), [tabindex="0"]')]
    const first = controls[0]
    const last = controls.at(-1)
    if (event.shiftKey ? document.activeElement === first : document.activeElement === last) {
      event.preventDefault()
      ;(event.shiftKey ? last : first)?.focus()
    }
  }
}
useEventListener(content, 'keydown', keydown)
</script>

<template>
  <BasePopover v-model="open" placement="bottom-end" :disabled="disabled">
    <template #trigger>
      <BaseIconButton ref="trigger" label="清理目录缺失模型" size="sm" :aria-expanded="open" :aria-controls="open ? id : undefined" aria-haspopup="dialog" :pressed="open" :disabled="disabled" @keydown="keydown">
        <BrushCleaning class="size-3.5" aria-hidden="true" />
      </BaseIconButton>
    </template>
    <div :id="id" ref="content" class="flex max-h-[calc(100dvh-1.5rem)] w-72 max-w-[calc(100vw-2rem)] flex-col gap-3 overflow-y-auto p-3" role="dialog" aria-label="清理目录缺失模型" tabindex="-1">
      <p class="m-0 text-cp-sm font-emphasis">
        清理目录缺失模型
      </p>
      <div class="grid max-h-48 gap-2 overflow-y-auto" role="group" aria-label="选择要移除的模型">
        <BaseCheckbox v-for="modelId in models" :key="modelId" :model-value="selected.includes(modelId)" :label="modelId" :title="modelId" show-label class="min-w-0 break-all" @update:model-value="select(modelId, $event)" />
      </div>
      <p v-if="wouldEmpty" class="m-0 text-cp-xs text-cp-warning-text" role="status">
        白名单至少保留一个模型
      </p>
      <div class="flex justify-end gap-2">
        <BaseButton size="sm" @click="open = false">
          取消
        </BaseButton>
        <BaseButton size="sm" variant="primary" :disabled="disabled || !selectedModels.length || wouldEmpty" @click="remove">
          移除所选{{ selectedModels.length ? ` (${selectedModels.length})` : '' }}
        </BaseButton>
      </div>
    </div>
  </BasePopover>
</template>
