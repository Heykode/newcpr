<script setup lang="ts">
import type { ReloginEntry } from '@/api/modules/relogin'
import { computed } from 'vue'
import { formatDateTime } from '@/utils/date'

const props = defineProps<{ row: ReloginEntry }>()
const detail = computed(() => {
  const egress = props.row.egress
  if (!egress)
    return '出口信息尚未读取'
  return [egress.label, egress.address, egress.exitIp ? `最近检测 IP：${egress.exitIp}` : '', egress.checkedAt ? `检测时间：${formatDateTime(egress.checkedAt)}` : '', egress.source === 'account' ? '跟随账号管理中的出口配置' : egress.source === 'import' ? '继承导入时选择的出口' : '未绑定账号出口，使用部署默认出口'].filter(Boolean).join('\n')
})
</script>

<template>
  <div class="min-w-0" :title="detail">
    <div class="flex min-w-0 items-center gap-2">
      <span class="min-w-0 break-words text-cp-sm" :class="row.egress?.kind === 'unavailable' ? 'text-cp-warning' : 'text-cp-text'">{{ row.egress?.label || '未读取' }}</span>
      <span v-if="row.egress?.latencyMs != null" class="shrink-0 whitespace-nowrap text-cp-xs tabular-nums text-cp-text-secondary">{{ row.egress.latencyMs }} ms</span>
    </div>
    <span v-if="row.egress?.exitIp || row.egress?.address" class="mt-1 block overflow-x-auto whitespace-nowrap font-mono text-cp-xs text-cp-text-secondary">{{ row.egress.exitIp || row.egress.address }}</span>
  </div>
</template>
