<script setup lang="ts">
import type { UsageViewModel } from '../utils/records'
import { computed } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import { t } from '@/i18n'
import { usageAccountText, usageTransportType } from '../utils/records'
import RequestDiagnosticsPanel from './RequestDiagnosticsPanel.vue'
import UsageDetailFieldGrid from './UsageDetailFieldGrid.vue'
import UsageStatusCodeBadge from './UsageStatusCodeBadge.vue'

const props = defineProps<{ record: UsageViewModel | null }>()
defineEmits<{ fullDetail: [] }>()
const open = defineModel<boolean>({ default: false })
const context = computed(() => [
  { label: '账号', value: props.record ? usageAccountText(props.record) : null, wrap: true, fullWidth: true },
  { label: '时间', value: props.record?.createdAtDisplay },
  { label: '总耗时', value: props.record?.latencyMsDisplay },
  { label: '请求模型', value: props.record?.requestedModel },
  { label: '上游模型', value: props.record?.upstreamModel },
  { label: '端点', value: props.record?.route },
  { label: '上游传输', value: usageTransportType(props.record?.upstreamTransport) },
  { label: '客户端状态码', value: props.record?.clientStatusCode },
  { label: '上游状态码', value: props.record?.upstreamStatusCode },
  { label: '请求 ID', value: props.record?.requestId, wrap: true, fullWidth: true },
  { label: '上游请求 ID', value: props.record?.upstreamRequestId, wrap: true, fullWidth: true },
])
const errorMessage = computed(() => {
  const message = props.record?.message.trim()
  return message && message !== props.record?.logicalOutcome ? message : '未记录具体错误消息'
})
</script>

<template>
  <BaseModal v-model="open" :title="t('usage.error.errorDetails')" tone="danger" size="xl">
    <div v-if="record" class="grid min-w-0 gap-4">
      <section class="min-w-0 border-l-2 border-cp-error px-3">
        <div class="flex flex-wrap items-center gap-3">
          <strong>{{ record.logicalOutcome === 'incomplete' ? '响应未完成' : '请求失败' }}</strong>
          <UsageStatusCodeBadge :status-code="record.statusCode" />
        </div>
        <p class="mb-0 whitespace-pre-wrap break-all font-mono text-cp-sm text-cp-error-text">
          {{ errorMessage }}
        </p>
      </section>
      <UsageDetailFieldGrid :items="context" />
      <section v-if="record.attempts?.length || record.attemptsComplete === false" class="min-w-0">
        <h3 class="text-cp-sm">
          尝试链路
        </h3>
        <p v-if="record.attemptsComplete === false" class="text-cp-warning-text">
          {{ t('usage.error.attemptRecordMayBeIncomplete') }}
        </p>
        <ol class="m-0 grid list-none gap-3 p-0">
          <li v-for="attempt in record.attempts" :key="attempt.id" class="min-w-0 border-b border-cp-border pb-3">
            <div class="flex flex-wrap items-center gap-3">
              <strong>尝试 {{ attempt.attemptIndex }}</strong>
              <span>{{ ({ succeeded: '成功', failed: '失败', cancelled: '取消', incomplete: '未完成', running: '进行中' } as Record<string, string>)[attempt.outcome] || attempt.outcome }}</span>
              <UsageStatusCodeBadge :status-code="attempt.statusCode" />
            </div>
            <UsageDetailFieldGrid
              :items="[
                { label: '账号', value: attempt.accountEmail || attempt.accountName || attempt.accountId, wrap: true },
                { label: '上游传输', value: usageTransportType(attempt.transport) },
                { label: '错误码', value: attempt.providerErrorCode, wrap: true },
                { label: '失败分类', value: attempt.failureClass, wrap: true },
                { label: '发送状态', value: attempt.sendState },
                { label: '触发方式', value: attempt.trigger },
              ]"
            />
          </li>
        </ol>
      </section>
      <RequestDiagnosticsPanel :request-id="record.requestId" :active="open" />
    </div>
    <template #footer>
      <BaseButton variant="soft" @click="$emit('fullDetail')">
        完整详情
      </BaseButton>
      <BaseButton variant="primary" @click="open = false">
        {{ t('common.actions.close') }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
