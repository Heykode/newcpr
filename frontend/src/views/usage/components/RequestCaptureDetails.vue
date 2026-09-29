<script setup lang="ts">
import { Download, RefreshCw } from '@lucide/vue'
import { computed } from 'vue'
import { captureExportUrl } from '@/api/modules/request-capture'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { formatDateTime } from '@/utils/date'
import { useRequestCaptureDetails } from '../composables/useRequestCaptureDetails'
import UsageDetailCodePanel from './UsageDetailCodePanel.vue'

const props = defineProps<{ requestId: string }>()
const { records, selectedId, selected, offset, page, loading, reading, error, readError, refresh, retry }
  = useRequestCaptureDetails(() => props.requestId)
const options = computed(() => records.value.map(record => ({
  value: record.id,
  label: `${formatDateTime(record.createdAt)} · ${(record.bytes / 1048576).toFixed(2)} MiB${record.incomplete ? ' · 材料不完整' : ''}`,
})))
</script>

<template>
  <section class="mt-4 min-w-0 border-t border-cp-border pt-4" aria-label="详细采集材料">
    <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
      <h3 class="m-0 text-cp-sm font-heavy text-cp-text-secondary">
        详细采集材料
      </h3>
      <div class="flex items-center gap-2">
        <BaseIconButton label="刷新关联采集" :disabled="loading || reading" @click="refresh">
          <RefreshCw class="size-4" />
        </BaseIconButton>
        <a v-if="selected" :href="captureExportUrl('record', selected.id)" download aria-label="导出详细采集材料" title="导出详细采集材料" class="p-2 text-cp-text-secondary"><Download class="size-4" /></a>
      </div>
    </div>
    <p v-if="loading" role="status" class="text-cp-sm text-cp-text-secondary">
      正在加载关联材料…
    </p>
    <p v-else-if="error" role="alert" class="text-cp-sm text-cp-error-text">
      {{ error }}
    </p>
    <p v-else-if="!records.length" class="text-cp-sm text-cp-text-secondary">
      暂无详细采集材料。可能未启用、尚未收尾、已过期或采集被跳过。
    </p>
    <template v-else>
      <BaseSelect v-if="records.length > 1" v-model="selectedId" class="mb-3 w-full" aria-label="关联采集记录" :options="options" />
      <p v-if="selected?.incomplete" role="status" class="text-cp-xs text-cp-warning-text">
        材料不完整，不代表上游没有返回内容。
      </p>
      <p v-if="reading" role="status" class="text-cp-sm text-cp-text-secondary">
        正在读取采集正文…
      </p>
      <div v-else-if="readError" role="alert" class="text-cp-sm text-cp-error-text">
        {{ readError }}
        <BaseButton variant="soft" size="sm" @click="retry">
          重试
        </BaseButton>
      </div>
      <UsageDetailCodePanel v-else-if="page" title="采集正文（已脱敏）" max-height="360px" :content="page.text" />
      <div class="mt-3 flex flex-wrap items-center gap-2">
        <BaseButton variant="soft" size="sm" :disabled="reading || offset === 0" @click="offset = 0">
          第一页
        </BaseButton>
        <BaseButton variant="soft" size="sm" :disabled="reading || page?.nextOffset == null" @click="offset = page?.nextOffset ?? 0">
          下一页
        </BaseButton>
        <span class="text-cp-xs text-cp-text-secondary">偏移 {{ offset }} 字节</span>
      </div>
    </template>
  </section>
</template>
