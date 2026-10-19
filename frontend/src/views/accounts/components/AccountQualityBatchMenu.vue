<script setup lang="ts">
import { ChevronDown, ClipboardCheck, Pause, Play } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseMenuItem from '@/components/base/BaseMenuItem.vue'
import BasePopover from '@/components/base/BasePopover.vue'

defineProps<{ selectedCount: number, disabled: boolean, loading: boolean }>()
const emit = defineEmits<{ change: [enabled: boolean] }>()
</script>

<template>
  <BasePopover class="w-full xl:w-auto" :disabled="disabled || loading">
    <template #trigger="{ open }">
      <BaseButton class="w-full whitespace-nowrap xl:w-auto" :disabled="disabled" :loading="loading" :aria-expanded="open" aria-label="质量检测">
        <ClipboardCheck class="size-4" />质量检测<ChevronDown class="size-3.5" />
      </BaseButton>
    </template>
    <template #default="{ close }">
      <div class="w-48 max-w-[calc(100vw-16px)] p-1.5" role="group" aria-label="批量质量检测">
        <p class="px-3 py-2 text-cp-xs text-cp-text-secondary">
          已选 {{ selectedCount }} 个账号
        </p>
        <p v-if="selectedCount > 1000" class="px-3 pb-2 text-cp-xs text-cp-warning">
          每次最多选择 1000 个账号
        </p>
        <BaseMenuItem :disabled="disabled || loading || !selectedCount || selectedCount > 1000" @click="close(); emit('change', true)">
          <template #icon>
            <Play class="size-3.5 text-cp-success" />
          </template>
          批量开启
        </BaseMenuItem>
        <BaseMenuItem :disabled="disabled || loading || !selectedCount || selectedCount > 1000" @click="close(); emit('change', false)">
          <template #icon>
            <Pause class="size-3.5 text-cp-text-secondary" />
          </template>
          批量关闭
        </BaseMenuItem>
      </div>
    </template>
  </BasePopover>
</template>
