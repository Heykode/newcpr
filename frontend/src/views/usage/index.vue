<script setup lang="ts">
import type { UsageTimeRange } from './composables/useUsageTimeRange'
import type { UsageFilterParams } from '@/api'
import { Eye } from '@lucide/vue'

import { computed, watch } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseTableColumnSettings from '@/components/base/BaseTable/BaseTableColumnSettings.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { useTableColumns } from '@/components/base/BaseTable/useTableColumns'
import ProviderFilterSegmented from '@/components/ProviderFilterSegmented.vue'
import OpsErrorPanel from './components/OpsErrorPanel.vue'
import UsageFilters from './components/UsageFilters.vue'
import UsageInsightsGrid from './components/UsageInsightsGrid.vue'
import UsageQueryFilters from './components/UsageQueryFilters.vue'
import UsageRecordDetailModal from './components/UsageRecordDetailModal.vue'
import UsageRecordsTable from './components/UsageRecordsTable.vue'
import UsageSummaryCards from './components/UsageSummaryCards.vue'
import { useUsageFilters } from './composables/useUsageFilters'
import { useUsageRecordDetail } from './composables/useUsageRecordDetail'
import { useUsageRecordsTable } from './composables/useUsageRecordsTable'
import { useUsageTimeRange } from './composables/useUsageTimeRange'
import { usageRecordColumns, usageTimeRangeOptions } from './constants'

const { visibleColumns, columnOptions, setColumnVisible, resetColumns } = useTableColumns(usageRecordColumns, 'usage-records')
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
  { label: '成功记录', value: 'success' },
  { label: '错误排查', value: 'errors' },
]
const { timeRange, timeRangeParams, refreshTimeRangeEnd, latestTimeRangeParams }
  = useUsageTimeRange((draft.value.timeRange || 'today') as UsageTimeRange)

const {
  currentPage,
  searchQuery,
  providerQuery,
  usagePagination,
  loading,
  analyticsLoading,
  diagnosticLoading,
  error: tableError,
  records,
  summary,
  insights,
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
})

const { showDetailModal, selectedUsageRecord, handleViewDetail } = useUsageRecordDetail()

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
    <BasePageHeader title="使用统计" description="查看请求用量、性能趋势与调用错误记录">
      <template #actions>
        <BaseSelect v-model="timeRange" :options="usageTimeRangeOptions" class="w-34" />
        <ProviderFilterSegmented
          v-model="providerQuery"
          :disabled="refreshingList"
          class="w-31 shrink-0"
        />
      </template>
    </BasePageHeader>

    <UsageQueryFilters v-model="draft" :range="timeRangeParams" :errors="recordView === 'errors'" :error="filterError" />
    <UsageSummaryCards :summary="summary" />
    <UsageInsightsGrid
      v-model:diagnostic-dimension="diagnosticDimension"
      :overview="insights.overview"
      :diagnostics="insights.diagnostics"
      :loading="analyticsLoading"
      :diagnostic-loading="diagnosticLoading"
      @diagnostic-page-change="handleDiagnosticPageChange"
    />

    <BaseCard
      class="mt-5 flex flex-col"
    >
      <template #header>
        <div class="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 class="m-0 text-xl leading-[1.15] font-heavy text-cp-text">
              请求明细
            </h2>
            <p
              class="mt-1.75 mb-0 text-cp leading-[1.15] font-emphasis text-cp-text-secondary"
            >
              成功请求与失败请求明细
            </p>
          </div>
          <BaseSegmented v-model="recordView" label="请求明细类型" :options="recordViewOptions" class="w-52" />
        </div>
      </template>

      <template #body>
        <div
          v-show="recordView === 'success'"
          class="grid min-h-130 min-w-0 flex-1 grid-rows-[auto_minmax(0,1fr)] gap-3"
        >
          <UsageFilters
            v-model:search="searchQuery"
            hide-search
            :loading="loading"
            :refreshing="refreshingList"
            @refresh="refreshUsageRecords"
          >
            <template #actions>
              <BaseTableColumnSettings
                label="成功记录显示列"
                :options="columnOptions"
                @change="setColumnVisible"
                @reset="resetColumns"
              />
            </template>
          </UsageFilters>

          <div class="flex min-h-0 min-w-0 flex-col">
            <p v-if="tableError" role="alert" class="text-cp-sm text-cp-error-text">
              {{ tableError }}
            </p>
            <UsageRecordsTable
              class="min-h-0 flex-1"
              :columns="visibleColumns"
              :rows="records"
              :loading="loading"
              empty-text="暂无使用记录"
              @filter="applyFilter"
            >
              <template #actions="{ row }">
                <div class="flex items-center justify-start">
                  <BaseIconButton
                    variant="ghost"
                    size="sm"
                    label="查看使用记录详情"
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

    <UsageRecordDetailModal v-model="showDetailModal" :record="selectedUsageRecord" />
  </div>
</template>
