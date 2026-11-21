<script setup lang="ts">
import type { ProxyBatchItem } from '../utils/batch'
import { computed } from 'vue'
import { t } from '@/i18n'
import { proxyBatchStates, summarizeProxyBatch, summarizeProxyTests } from '../utils/batch'
import { locationDetectionMessage } from '../utils/location'

const props = defineProps<{ items?: ProxyBatchItem[], tests?: ProxyBatchItem[] }>()
const testItems = computed(() => props.tests ?? [])

function resultMessage(item: ProxyBatchItem, test?: ProxyBatchItem) {
  if (test && test.status !== 'succeeded')
    return test.message
  const detection = test?.record?.lastTest?.location
  if (detection?.status === 'failed' || detection?.status === 'conflict')
    return locationDetectionMessage(detection)
  return item.status === 'succeeded' ? '' : item.message
}

const rows = computed(() => {
  const tests = new Map(testItems.value.map(item => [item.key, item]))
  return (props.items ?? testItems.value).map((item) => {
    const test = props.items ? (item.record ? tests.get(item.record.id) : undefined) : item
    const status = (test ?? item).status
    const state = proxyBatchStates.value[status]
    return {
      item,
      test,
      label: status === 'succeeded'
        ? t(test ? 'proxies.batchTest.connectionIsNormal' : 'proxies.batchTest.addedStatus')
        : state.label,
      tone: state.tone,
      message: resultMessage(item, test),
      messageTone: status === 'succeeded' ? 'text-cp-warning-text' : state.tone,
    }
  })
})
const additions = computed(() => summarizeProxyBatch(props.items ?? []))
const connections = computed(() => summarizeProxyTests(testItems.value))
const attention = computed(() => additions.value.attention + connections.value.attention)
const skipped = computed(() => additions.value.skipped + connections.value.skipped)
</script>

<template>
  <section class="min-w-0 space-y-3" :aria-label="t('proxies.batchTest.resultsTitle')">
    <div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-cp-xs text-cp-text-secondary" role="status" aria-live="polite">
      <template v-if="items">
        <span>{{ t('proxies.batchTest.processed') }} <strong class="font-mono font-medium text-cp-text">{{ additions.completed }} / {{ items.length }}</strong></span>
        <span>{{ t('proxies.batchTest.added', { created: additions.succeeded }) }}</span>
        <span v-if="additions.failed">{{ t('proxies.batchTest.invalidFormat', { invalid: additions.failed }) }}</span>
      </template>
      <template v-if="testItems.length">
        <span>{{ t('proxies.batchTest.connected', { succeeded: connections.succeeded }) }}</span>
        <span>{{ t('proxies.batchTest.connectionFailed', { failed: connections.failed }) }}</span>
        <span v-if="connections.locationPending">{{ t('proxies.batchTest.locationPending', { locationPending: connections.locationPending }) }}</span>
      </template>
      <span v-if="attention">{{ t('proxies.batchTest.needsAttention', { attention }) }}</span>
      <span v-if="skipped">{{ t('proxies.batchTest.skipped', { skipped }) }}</span>
    </div>
    <div class="max-h-[min(26rem,50dvh)] overflow-auto" role="region" :aria-label="t('proxies.batchTest.scrollListOfProxyResults')" tabindex="0">
      <ol class="m-0 list-none space-y-1 p-0">
        <li v-for="row in rows" :key="row.item.key" class="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-1 rounded-md px-3 py-2.5 odd:bg-cp-fill-quaternary">
          <span class="min-w-0 truncate font-mono text-cp-xs text-cp-text" :title="row.item.endpoint">{{ row.item.endpoint || t('proxies.batchTest.lineNumber', { key: row.item.key }) }}</span>
          <span class="flex flex-wrap justify-end gap-x-2 text-cp-xs" :class="row.tone">
            <span v-if="items && row.test" class="text-cp-text-secondary">{{ t('proxies.batchTest.addedStatus') }}</span>
            <span>{{ row.label }}</span>
          </span>
          <div class="col-span-2 flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-cp-xs text-cp-text-secondary">
            <span class="max-w-full truncate" :title="row.item.name">{{ row.item.name }}</span>
            <span v-if="row.test?.record?.lastTest?.location?.status === 'detected'" class="max-w-full break-words">{{ locationDetectionMessage(row.test.record.lastTest.location) }}</span>
            <span v-if="row.test?.record?.lastTest" class="min-w-0 break-all font-mono">
              <span class="whitespace-nowrap">{{ row.test.record.lastTest.latencyMs }} ms</span><span v-if="row.test.record.lastTest.exitIpv4"> · IPv4 {{ row.test.record.lastTest.exitIpv4 }}</span><span v-if="row.test.record.lastTest.exitIpv6"> · IPv6 {{ row.test.record.lastTest.exitIpv6 }}</span>
            </span>
          </div>
          <span v-if="row.message" class="col-span-2 text-cp-xs leading-relaxed wrap-anywhere" :class="row.messageTone">{{ row.message }}</span>
        </li>
      </ol>
    </div>
  </section>
</template>
