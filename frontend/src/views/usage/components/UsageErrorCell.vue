<script setup lang="ts">
import type { UsageDisplayRecord } from '../utils/records'
import { CheckCheck, CircleHelp, Network, Server, Unplug } from '@lucide/vue'
import { computed } from 'vue'
import { t } from '@/i18n'
import { usageErrorOrigin, usageErrorSummary, usageErrorTitle } from '../utils/errors'

const props = defineProps<{ record: UsageDisplayRecord }>()

const originIcons = {
  proxy: Network,
  upstream: Server,
  connection: Unplug,
  unknown: CircleHelp,
}

const message = computed(() => props.record.errorMessage?.replace(/^OpenAI upstream failure: /, ''))
const code = computed(() => props.record.errorCode || props.record.errorKind)
const origin = computed(() => usageErrorOrigin(props.record))
const outcome = computed(() => usageErrorTitle({ logicalOutcome: props.record.outcome ?? 'failed', errorCode: props.record.errorCode, errorKind: props.record.errorKind }))
const summary = computed(() => message.value || code.value || t('usage.error.noDetails'))
</script>

<template>
  <div class="min-w-0 py-0.5">
    <div class="flex min-w-0 items-start gap-2">
      <span
        class="inline-flex size-5 shrink-0 items-center justify-center"
        :class="origin.className"
        role="img"
        :aria-label="origin.label"
        :title="origin.hint ? `${origin.label} · ${origin.hint}` : origin.label"
      >
        <component :is="originIcons[origin.kind ?? 'unknown']" class="size-3.5" aria-hidden="true" />
      </span>
      <span class="line-clamp-2 min-w-0 text-cp-sm font-medium wrap-anywhere text-cp-text" :title="summary" translate="no">{{ summary }}</span>
    </div>
    <div class="mt-1 flex min-w-0 flex-wrap items-center gap-x-2 gap-y-1 text-cp-xs text-cp-text-secondary">
      <span>{{ outcome }}</span>
      <code v-if="message && code && code !== message" class="truncate font-mono" :title="code" translate="no">{{ code }}</code>
      <span v-if="record.upstreamStatusCode != null" class="shrink-0 font-mono text-cp-xs text-cp-text-secondary" :title="t('usage.error.upstreamStatusCode')">
        {{ t('usage.error.upstreamHttpStatus', { status: record.upstreamStatusCode }) }}
      </span>
      <span v-if="record.clientStatusCode != null && record.clientStatusCode !== record.upstreamStatusCode" class="shrink-0 font-mono text-cp-xs text-cp-text-secondary" :title="t('usage.error.clientStatusCode')">
        {{ t('usage.error.clientHttpStatus', { status: record.clientStatusCode }) }}
      </span>
      <span v-if="record.recovered" class="inline-flex shrink-0 text-cp-success-text" role="img" :aria-label="t('usage.error.recovered')" :title="t('usage.error.recovered')">
        <CheckCheck class="size-4" :stroke-width="2.25" aria-hidden="true" />
      </span>
    </div>
    <span v-if="record.websocketCloseCode" class="mt-1 block font-mono text-cp-xs text-cp-warning-text">WS {{ record.websocketCloseCode }}</span>
    <span v-if="usageErrorSummary(record)" class="mt-1 block text-cp-xs text-cp-text-secondary">{{ usageErrorSummary(record) }}</span>
  </div>
</template>
