<script setup lang="ts">
import type { ResetInventory } from '@/api/modules/reset-credits'
import { computed } from 'vue'

const props = defineProps<{ inventory?: ResetInventory, supported: boolean }>()
const text = computed(() => {
  if (!props.supported)
    return '不支持'
  if (props.inventory?.pending)
    return '待确认'
  if (props.inventory?.error)
    return '查询失败'
  return props.inventory?.credits ? `${props.inventory.credits.availableCount} 次` : '未查询'
})
const title = computed(() => props.inventory
  ? `${props.inventory.error || text.value} · 查询于 ${new Date(props.inventory.checkedAt).toLocaleString()}`
  : text.value)
</script>

<template>
  <span :title="title" class="text-cp-sm tabular-nums" :class="inventory?.error || inventory?.pending ? 'text-cp-warning-text' : 'text-cp-text-secondary'">{{ text }}</span>
</template>
