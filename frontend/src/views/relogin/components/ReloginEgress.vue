<script setup lang="ts">
import type { ReloginEntry } from '@/api/modules/relogin'
import { computed } from 'vue'
import { formatDateTime } from '@/utils/date'
import { egressAddress, egressLabel } from '../maintenance'

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
    <span class="block truncate whitespace-nowrap font-mono text-cp-sm" :class="row.egress?.kind === 'unavailable' ? 'text-cp-warning' : 'text-cp-text'">{{ egressAddress(row) }}</span>
    <span class="mt-1 block truncate text-cp-xs text-cp-text-tertiary">{{ egressLabel(row) }}</span>
  </div>
</template>
