<script setup lang="ts">
import type { UsageTimeRange } from './composables/useUsageTimeRange'
import type { UsageErrorOrigin, UsageFilterParams } from '@/api'
import { Eye } from '@lucide/vue'
import { useElementSize } from '@vueuse/core'
import { computed, shallowRef, useTemplateRef, watch } from 'vue'

import BaseCard from '@/components/base/BaseCard.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTableColumnSettings from '@/components/base/BaseTable/BaseTableColumnSettings.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import ProviderFilterSegmented from '@/components/ProviderFilterSegmented.vue'
import { t } from '@/i18n'
import DetailedCaptureControl from './components/DetailedCaptureControl.vue'
import OpsErrorPanel from './components/OpsErrorPanel.vue'
import UsageErrorDetailModal from './components/UsageErrorDetailModal.vue'
import UsageFilters from './components/UsageFilters.vue'
import UsageInsightsGrid from './components/UsageInsightsGrid.vue'
import UsageQueryFilters from './components/UsageQueryFilters.vue'
import UsageRecordDetailModal from './components/UsageRecordDetailModal.vue'
import UsageRecordsTable from './components/UsageRecordsTable.vue'
import UsageSummaryCards from './components/UsageSummaryCards.vue'
import { useUsageFilters } from './composables/useUsageFilters'
import { useUsageRecordColumns } from './composables/useUsageRecordColumns'
import { useUsageRecordDetail } from './composables/useUsageRecordDetail'
import { useUsageRecordsTable } from './composables/useUsageRecordsTable'
import { useUsageTimeRange } from './composables/useUsageTimeRange'

const { draft, error: filterError, filters, errorFilters } = useUsageFilters()
const recordView = computed({
  get: () => draft.value.view === 'errors' ? 'errors' : 'success',
  set: value => draft.value = { ...draft.value, view: value },
})
const provider = computed({
  get: () => draft.value.provider || '',
  set: value => draft.value = { ...draft.value, provider: value },
})
const recordViewOptions = [
  { label: '请求记录', value: 'success' },
  { label: '错误排查', value: 'errors' },
]
const recordStatus = shallowRef<'success' | 'error' | 'all'>('success')
const errorOrigin = shallowRef<UsageErrorOrigin | 'all'>('all')
const errorOriginOptions = [
  { label: '全部来源', value: 'all' },
  { label: '上游响应', value: 'upstream' },
  { label: '代理处理', value: 'proxy' },
  { label: '上游连接', value: 'connection' },
  { label: '未知来源', value: 'unknown' },
]
const tableArea = useTemplateRef<HTMLElement>('tableArea')
const { width: tableWidth } = useElementSize(tableArea)
const columns = useUsageRecordColumns(recordStatus, tableWidth)
const recordStatusOptions = [
  { label: '成功', value: 'success' },
  { label: '错误', value: 'error' },
  { label: '全部', value: 'all' },
]
const { timeRange, timeRangeParams, refreshTimeRangeEnd, latestTimeRangeParams }
  = useUsageTimeRange((draft.value.timeRange || 'today') as UsageTimeRange)

const {
  currentPage,
  searchQuery,
  providerQuery,
  usagePagination,
  loading,
  error: tableError,
  records,
  summary,
  health,
  performance,
  cost,
  diagnostics,
  performanceMetric,
  costMetric,
  refreshingList,
  diagnosticDimension,
  loadUsageRecords,
  refreshUsageRecords,
  handlePageChange,
  handlePageSizeChange,
  handleDiagnosticPageChange,
} = useUsageRecordsTable({
  timeRangeParams,
  latestTimeRangeParams,
  active: computed(() => recordView.value === 'success'),
  filters,
  provider,
  recordStatus,
  errorOrigin,
})

const { showDetailModal, selectedUsageRecord, detailIsError, detailLoading, detailError, handleViewDetail, reloadDetail } = useUsageRecordDetail()
const fullDetail = shallowRef(false)
watch([showDetailModal, () => selectedUsageRecord.value?.requestId], () => {
  fullDetail.value = false
})
const errorDetail = computed(() => !fullDetail.value && detailIsError.value)

function applyFilter(value: UsageFilterParams) {
  draft.value = {
    ...draft.value,
    ...(value.accountIds ? { accountId: '', accountSearch: '' } : {}),
    ...Object.fromEntries(Object.entries(value).map(([key, value]) => [key, String(value ?? '')])),
  }
}
watch(() => draft.value.timeRange, (value) => {
  timeRange.value = value === '7d' || value === '30d' ? value : 'today'
})
watch(timeRange, () => {
  if ((draft.value.timeRange || 'today') !== timeRange.value)
    draft.value = { ...draft.value, timeRange: timeRange.value, startTime: '', endTime: '' }
  refreshTimeRangeEnd()
  currentPage.value = 1
  void loadUsageRecords()
})
</script>

<template>
  <div class="w-full">
    <BasePageHeader title="使用统计" :description="t('usage.page.description')">
      <template #actions>
        <ProviderFilterSegmented
          v-model="providerQuery"
          :disabled="refreshingList"
          class="w-31 shrink-0"
        />
      </template>
    </BasePageHeader>

    <BaseCard padding="compact" class="mb-4 overflow-visible!">
      <UsageQueryFilters v-model="draft" :range="timeRangeParams" :errors="recordView === 'errors'" :error="filterError" />
    </BaseCard>

    <UsageSummaryCards :summary="summary.data" :loading="summary.loading" :error="summary.error" />
    <UsageInsightsGrid
      v-model:diagnostic-dimension="diagnosticDimension"
      v-model:performance-metric="performanceMetric"
      v-model:cost-metric="costMetric"
      :health="health"
      :performance="performance"
      :cost="cost"
      :diagnostics="diagnostics"
      @diagnostic-page-change="handleDiagnosticPageChange"
    />

    <BaseCard
      class="mt-5 flex flex-col"
    >
      <template #header>
        <div class="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 class="m-0 text-base leading-snug font-semibold text-cp-text">
              {{ t('usage.page.requestDetails') }}
            </h2>
            <p
              class="mt-1 mb-0 text-cp-sm leading-relaxed text-cp-text-secondary"
            >
              {{ t('usage.page.recordsDescription') }}
            </p>
          </div>
          <div class="flex min-w-0 flex-wrap items-center gap-3">
            <BaseSegmented v-model="recordView" :label="t('usage.filters.requestDetailType')" :options="recordViewOptions" class="w-52" />
            <DetailedCaptureControl />
          </div>
        </div>
      </template>

      <template #body>
        <div
          v-show="recordView === 'success'"
          class="flex min-h-130 min-w-0 flex-1 flex-col gap-3"
        >
          <UsageFilters
            v-model:search="searchQuery"
            hide-search
            :loading="loading"
            :refreshing="refreshingList"
            @refresh="refreshUsageRecords"
          >
            <template #actions>
              <BaseSegmented v-model="recordStatus" label="请求状态" :options="recordStatusOptions" class="w-48" />
              <BaseSelect v-if="recordStatus === 'error'" v-model="errorOrigin" label="错误来源" :options="errorOriginOptions" class="w-36" />
              <BaseTableColumnSettings
                label="请求记录显示列"
                :options="columns.columnOptions.value"
                reorderable
                @change="columns.setColumnVisible"
                @reset="columns.resetColumns"
                @reorder="columns.setColumnOrder"
              />
            </template>
          </UsageFilters>

          <div ref="tableArea" class="flex min-h-0 min-w-0 flex-1 flex-col">
            <p v-if="tableError" role="alert" class="text-cp-sm text-cp-error-text">
              {{ tableError }}
            </p>
            <UsageRecordsTable
              class="min-h-0 flex-1"
              :columns="columns.visibleColumns.value"
              :column-layout="recordStatus === 'error' ? 'content' : 'proportional'"
              :rows="records"
              :loading="loading"
              :empty-text="t('usage.noUsageRecords')"
              @filter="applyFilter"
            >
              <template #actions="{ row }">
                <div class="flex items-center justify-start">
                  <BaseIconButton
                    variant="ghost"
                    size="sm"
                    :label="t('usage.page.viewUsageRecordDetails')"
                    @click="handleViewDetail(row)"
                  >
                    <Eye class="size-3.5" />
                  </BaseIconButton>
                </div>
              </template>
            </UsageRecordsTable>
            <BaseTablePagination
              :pagination="usagePagination"
              :loading="loading"
              @page-change="handlePageChange"
              @page-size-change="handlePageSizeChange"
            />
          </div>
        </div>

        <div v-show="recordView === 'errors'" class="min-h-130 min-w-0 flex-1">
          <OpsErrorPanel
            :time-range-params="timeRangeParams"
            :latest-time-range-params="latestTimeRangeParams"
            :provider="providerQuery"
            :active="recordView === 'errors'"
            :filters="errorFilters"
            @filter="applyFilter"
          />
        </div>
      </template>
    </BaseCard>

    <UsageErrorDetailModal v-if="errorDetail" v-model="showDetailModal" :record="selectedUsageRecord" :loading="detailLoading" :error="detailError" @retry="reloadDetail" @full-detail="fullDetail = true" />
    <UsageRecordDetailModal v-else v-model="showDetailModal" :record="selectedUsageRecord" :loading="detailLoading" :error="detailError" @retry="reloadDetail" />
  </div>
</template>
