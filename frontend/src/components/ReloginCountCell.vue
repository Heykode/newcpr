<script setup lang="ts">
import { computed } from 'vue'
import { formatDateTime } from '@/utils/date'

const props = defineProps<{
  count: number | null
  lastReloginAt: string | null
}>()

const title = computed(() => {
  if (props.count == null)
    return '未确定对应号池账号，请确认工作区'
  const latest = props.lastReloginAt
    ? `最近成功重登：${formatDateTime(props.lastReloginAt, '—', 'Asia/Shanghai')} (UTC+8)`
    : '暂无成功重登记录'
  return `${latest}；自启用统计起，仅计成功重登并更新号池`
})
</script>

<template>
  <span class="inline-block max-w-full wrap-anywhere tabular-nums" :class="count === 0 || count == null ? 'text-cp-text-tertiary' : 'text-cp-text'" :title="title">
    {{ count ?? '—' }}
  </span>
</template>
