<script setup lang="ts">
import type { UsageListRecord } from '@/api'
import { RefreshCw } from '@lucide/vue'
import { computed, ref } from 'vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import { useUsageTurnState } from '../composables/useUsageTurnState'

const props = defineProps<{ record: Pick<UsageListRecord, 'id' | 'turnState'> }>()
const open = ref(false)
const summary = computed(() => props.record.turnState)
const { value, loading, failed, retry } = useUsageTurnState(
  () => props.record.id,
  () => open.value && summary.value?.injected === true,
)
const returned = computed(() => {
  if (summary.value?.returnedChars == null)
    return '未返回'
  return `返回 ${summary.value.returnedChars} 字符`
})
const changed = computed(() => summary.value?.returnedChars != null
  && summary.value.returnedChars !== summary.value.chars)
</script>

<template>
  <span v-if="!summary" class="text-cp-xs text-cp-text-quaternary" title="此请求未采集 State 注入信息">未记录</span>
  <span v-else-if="!summary.injected" class="text-cp-text-quaternary" title="此请求未附加托管 State；不代表没有客户端透传 State">—</span>
  <BasePopover v-else v-model="open" trigger="hover-click" placement="top-start" :hover-delay="150" class="max-w-full">
    <template #trigger>
      <button
        type="button"
        class="flex max-w-full min-w-0 flex-col items-start gap-1 text-left"
        :aria-label="`State 注入 ${summary.chars} 字符，${returned}`"
        :aria-expanded="open"
        aria-haspopup="dialog"
      >
        <code class="block max-w-full truncate font-mono text-cp-xs text-cp-text">{{ summary.preview }}</code>
        <span class="whitespace-nowrap text-cp-xs tabular-nums" :class="changed ? 'text-cp-warning-text' : 'text-cp-text-secondary'">
          {{ summary.chars }} 字符 · {{ summary.returnedChars == null ? '无回显' : `回 ${summary.returnedChars}` }}
        </span>
      </button>
    </template>
    <div role="dialog" aria-label="State 注入详情" class="w-96 max-w-[calc(100vw-32px)] space-y-3 p-3">
      <div class="flex items-center justify-between gap-3 text-cp-sm">
        <strong class="font-heavy">State 注入</strong>
        <span class="text-cp-text-secondary">{{ summary.chars }} 字符</span>
      </div>
      <p class="text-cp-xs text-cp-text-secondary">
        最终尝试 · {{ summary.transport === 'websocket' ? 'WS 请求帧' : 'HTTP 请求头' }}
        · {{ returned }}
        <template v-if="summary.returnedSame != null">
          · {{ summary.returnedSame ? '值相同' : '值不同' }}
        </template>
      </p>
      <div class="h-40 overflow-y-auto">
        <div v-if="loading" role="status" class="text-cp-sm text-cp-text-secondary">
          读取中
        </div>
        <div v-else-if="failed" role="alert" class="flex items-center gap-2 text-cp-sm text-cp-error-text">
          读取失败
          <BaseIconButton label="重试读取 State" size="sm" variant="ghost" @click.stop="retry">
            <RefreshCw class="size-3.5" />
          </BaseIconButton>
        </div>
        <code v-else-if="value" class="block whitespace-pre-wrap break-all font-mono text-cp-xs leading-relaxed text-cp-text select-text">{{ value }}</code>
        <p v-else class="text-cp-xs text-cp-text-secondary">
          完整 State 未记录
        </p>
      </div>
      <p class="text-cp-xs text-cp-text-quaternary">
        敏感值；附加或长度相同不代表上游已接受。
      </p>
    </div>
  </BasePopover>
</template>
