<script setup lang="ts">
import type { OutboundProxyTest } from '@/api'
import { Wifi } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import { t } from '@/i18n'

defineProps<{ result?: OutboundProxyTest | null, inOption?: boolean }>()
</script>

<template>
  <BasePopover v-if="result?.success" class="shrink-0" :trigger="inOption ? 'hover' : 'hover-click'" placement="top-end">
    <template #trigger="{ open }">
      <!-- 下拉项本身是按钮，内部仅展示状态，避免嵌套按钮干扰选中操作 -->
      <span v-if="inOption" class="inline-flex items-center gap-1 text-cp-xs whitespace-nowrap text-cp-success-text tabular-nums">
        <Wifi class="size-3.5" aria-hidden="true" />
        {{ result.latencyMs }} ms
      </span>
      <BaseButton v-else variant="link" size="sm" class="gap-1! px-0! no-underline! tabular-nums" :aria-label="t('accounts.proxy.connected', { latencyMs: result.latencyMs })" :aria-expanded="open">
        <template #icon>
          <Wifi class="size-3.5" aria-hidden="true" />
        </template>
        {{ result.latencyMs }} ms
      </BaseButton>
    </template>
    <div class="grid max-w-80 gap-1 px-3 py-2 font-mono text-cp-xs break-all">
      <span v-if="result.exitIpv4">IPv4 · {{ result.exitIpv4 }}</span>
      <span v-if="result.exitIpv6">IPv6 · {{ result.exitIpv6 }}</span>
      <span v-if="!result.exitIpv4 && !result.exitIpv6">IP · {{ result.exitIp || t('common.values.unknown') }}</span>
    </div>
  </BasePopover>
</template>
