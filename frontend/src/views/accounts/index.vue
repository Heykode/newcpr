<script setup lang="ts">
import type { AccountRow } from './constants'
import { ChevronDown, ListTodo, RefreshCw } from '@lucide/vue'
import { useLocalStorage } from '@vueuse/core'

import { computed, onMounted, ref, shallowRef, useTemplateRef, watch } from 'vue'
import { getRelogin } from '@/api/modules/relogin'
import AccountTemplateMenu from '@/components/account-templates/AccountTemplateMenu.vue'
import AccountGroupMarks from '@/components/AccountGroupMarks.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import BaseTable from '@/components/base/BaseTable/index.vue'
import { toast } from '@/components/base/BaseToast'
import LastUsedAtCell from '@/components/LastUsedAtCell.vue'
import ReloginCountCell from '@/components/ReloginCountCell.vue'
import { useAccountGroupCatalog } from '@/composables/useAccountGroupCatalog'
import AccountBatchEditModal from './components/AccountBatchEditModal.vue'
import AccountCapacityCell from './components/AccountCapacityCell.vue'
import AccountConnectionTestModal from './components/AccountConnectionTestModal.vue'
import AccountCreateModal from './components/AccountCreateModal/index.vue'
import AccountEditModal from './components/AccountEditModal.vue'
import AccountFilters from './components/AccountFilters.vue'
import AccountHealthTimeline from './components/AccountHealthTimeline.vue'
import AccountIdentityCell from './components/AccountIdentityCell.vue'
import AccountImportTasks from './components/AccountImportTasks/index.vue'
import AccountOverviewCards from './components/AccountOverviewCards.vue'
import AccountPlanBadge from './components/AccountPlanBadge.vue'
import AccountQuotaForecastModal from './components/AccountQuotaForecastModal/index.vue'
import AccountQuotaPanel from './components/AccountQuotaPanel/index.vue'
import AccountQuotaSummaryCell from './components/AccountQuotaSummaryCell/index.vue'
import AccountSchedulingSwitch from './components/AccountSchedulingSwitch.vue'
import AccountStatusBadge from './components/AccountStatusBadge/index.vue'
import AccountTableActions from './components/AccountTableActions.vue'
import AccountUsagePanel from './components/AccountUsagePanel.vue'
import { useAccountBatchEditor } from './composables/useAccountBatchEditor'
import { useAccountConnectionTest } from './composables/useAccountConnectionTest'
import { useAccountEditor } from './composables/useAccountEditor'
import { useAccountImportTasks } from './composables/useAccountImportTasks'
import { useAccountListForecast } from './composables/useAccountListForecast'
import { useAccountMutations } from './composables/useAccountMutations'
import { useAccountRelogin } from './composables/useAccountRelogin'
import { useAccountsQuery } from './composables/useAccountsQuery'
import { useAccountsTable } from './composables/useAccountsTable'
import { useAccountSwipeSelect } from './composables/useAccountSwipeSelect'
import { accountColumnOptions, accountColumns, derivedAccountStatus, readAccountColumnKeys, writeAccountColumnKeys } from './constants'
import { accountHasReloginTotp, reloginTotpEmailSet } from './relogin-availability'

const selectedIds = ref<Set<string>>(new Set())
const applyingTemplate = shallowRef(false)
const forecastAccount = ref<AccountRow | null>(null)
const forecastOpen = ref(false)

function openQuotaForecast(account: AccountRow) {
  forecastAccount.value = account
  forecastOpen.value = true
}

const {
  loading,
  refreshing,
  accounts,
  loadAccounts,
  refreshAccounts: refreshAccountDirectory,
  refreshAccountsSilently,
  searchQuery,
  providerQuery,
  statusQuery,
  groupQuery,
  planTypeQuery,
  sort,
  accountSummary,
  accountPagination,
  replaceAccount: replaceAccountQuery,
  handlePageChange,
  handlePageSizeChange,
  handleSortChange,
} = useAccountsQuery()

const reloginTotpEmails = shallowRef<ReadonlySet<string>>(new Set())
let reloginAvailabilityGeneration = 0

async function loadReloginAvailability() {
  const generation = ++reloginAvailabilityGeneration
  try {
    const result = await getRelogin({ silent: true })
    if (generation === reloginAvailabilityGeneration)
      reloginTotpEmails.value = reloginTotpEmailSet(result.items)
  }
  catch {
    // Keep the last safe projection; account directory availability is independent.
  }
}

function hasReloginTotp(account: AccountRow) {
  return accountHasReloginTotp(account, reloginTotpEmails.value)
}

async function refreshAccounts() {
  const [result] = await Promise.all([
    refreshAccountDirectory(),
    loadReloginAvailability(),
  ])
  return result
}

onMounted(() => void loadReloginAvailability())
const {
  actions: reloginActions,
  readError: reloginReadError,
  actionError: reloginActionError,
  open: reloginOpen,
  selected: reloginSelected,
  confirmDisabled: reloginConfirmDisabled,
  submitting: reloginSubmitting,
  request: requestRelogin,
  confirm: confirmRelogin,
} = useAccountRelogin({ accounts, reload: refreshAccountsSilently })

async function applyForecastAccount(account: AccountRow) {
  if (forecastAccount.value?.id === account.id)
    forecastAccount.value = account
  await replaceAccountQuery(account)
}

const {
  groups,
  loading: groupsLoading,
  loadGroups,
} = useAccountGroupCatalog()

async function onTemplateApplied() {
  try {
    await Promise.all([loadAccounts({ silent: true }), loadGroups({ silent: true })])
  }
  catch {
    toast.warning('模板已应用，列表刷新失败，请手动刷新')
  }
}

const {
  open: importTasksOpen,
  tasks: importTasks,
  detail: importTaskDetail,
  selectedId: selectedImportTaskId,
  error: importTasksError,
  stopError: importTaskStopError,
  loading: importTasksLoading,
  stopping: importTaskStopping,
  activeCount: activeImportTaskCount,
  select: selectImportTask,
  created: importTaskCreated,
  refresh: refreshImportTasks,
  stop: stopImportTask,
} = useAccountImportTasks({
  reload: () => Promise.all([refreshAccountsSilently(), loadGroups({ silent: true })]),
})

const {
  showCreateModal,
  showDeleteModal,
  showSingleDeleteModal,
  showExportModal,
  pendingDeleteAccount,
  recoveringAccountIds,
  refreshingAccountIds,
  refreshingQuotaAccountIds,
  togglingAccountIds,
  togglingTurnStateAccountIds,
  deletingAccount,
  creatingAccount,
  authorizingOAuth,
  batchDeleting,
  exportingAccounts,
  reauthorizingAccount,
  createForm,
  handleCreate,
  handleAuthorizeOAuth,
  openCreateAccount,
  openReauthorizeAccount,
  requestDeleteAccount,
  handleDelete,
  handleBatchDelete,
  handleExportAccounts,
  confirmExportAccounts,
  handleRecover,
  handleRefresh,
  handleRefreshQuota,
  handleToggleEnabled,
  handleToggleTurnState,
} = useAccountMutations({
  accounts,
  selectedIds,
  reload: options => Promise.all([loadAccounts(options), loadGroups(options)]),
  replaceAccount,
  onImportTaskCreated: importTaskCreated,
})

const {
  showConnectionTestModal,
  testingAccount,
  connectionTestStatus,
  connectionTestModel,
  connectionTestLogs,
  connectionTestError,
  connectionTestStartedAt,
  connectionTestFinishedAt,
  connectionTestDurationMs,
  testingConnectionIds,
  loadingConnectionTestModels,
  refreshingConnectionTestModels,
  connectionTestSelectedModel,
  connectionTestStream,
  connectionTestPrompt,
  connectionTestModelOptions,
  connectionTestStatusView,
  openConnectionTest,
  handleRefreshConnectionTestModels,
  handleTestConnection,
} = useAccountConnectionTest({ reload: refreshAccountsSilently })

const {
  expandedAccountIds,
  allSelected,
  indeterminate,
  selectedRowKeys,
  expandedRowKeys,
  toggleSelection,
  toggleExpanded,
  toggleAll,
} = useAccountsTable(accounts, selectedIds)

const {
  showBatchEditModal,
  customName: batchCustomName,
  updateCustomName: batchUpdateCustomName,
  turnStateAvailable: batchTurnStateAvailable,
  schedulingEnabled: batchSchedulingEnabled,
  turnStateInjectionEnabled: batchTurnStateInjectionEnabled,
  concurrencyLimit: batchConcurrencyLimit,
  weight: batchWeight,
  modelAccess: batchModelAccess,
  updateModelAccess: batchUpdateModelAccess,
  catalogAccountId: batchCatalogAccountId,
  proxyMode: batchProxyMode,
  proxyId: batchProxyId,
  selectedGroupIds: batchGroupIds,
  updateEnabled: batchUpdateEnabled,
  updateTurnStateInjectionEnabled: batchUpdateTurnStateInjectionEnabled,
  updateConcurrencyLimit: batchUpdateConcurrencyLimit,
  updateWeight: batchUpdateWeight,
  updateGroups: batchUpdateGroups,
  updateProxy: batchUpdateProxy,
  hasUpdates: batchHasUpdates,
  saving: savingBatchEdit,
  open: openBatchEdit,
  save: saveBatchEdit,
} = useAccountBatchEditor({
  accounts,
  selectedIds,
  reloadAccounts: loadAccounts,
  reloadGroups: loadGroups,
})

const {
  showEditModal,
  customName: editingCustomName,
  editingAccount,
  schedulingEnabled,
  turnStateInjectionEnabled,
  concurrencyLimit: editingConcurrencyLimit,
  weight: editingWeight,
  modelAccess: editingModelAccess,
  proxyMode: editingProxyMode,
  proxyId: editingProxyId,
  selectedGroupIds: editingGroupIds,
  saving: savingAccountEdit,
  open: openAccountEdit,
  save: saveAccountEdit,
} = useAccountEditor({
  accounts,
  reloadAccounts: loadAccounts,
  reloadGroups: loadGroups,
})

const visibleColumnKeys = useLocalStorage<string[]>(
  'cpr.accounts.visible-columns',
  accountColumnOptions.map(column => column.key),
  { serializer: { read: readAccountColumnKeys, write: writeAccountColumnKeys } },
)
const visibleAccountColumns = computed(() => accountColumns.filter(column =>
  ['expander', 'selection', 'actions'].includes(column.key)
  || visibleColumnKeys.value.includes(column.key),
))
const { cache: forecastCache } = useAccountListForecast(accounts)

async function replaceAccount(account: AccountRow) {
  forecastCache.invalidate(account.id)
  forecastCache.syncAccount(account)
  const retained = await replaceAccountQuery(account)
  return retained
}
watch(visibleColumnKeys, (keys) => {
  const allowed = new Set(accountColumnOptions.map(column => column.key))
  if (keys.some(key => !allowed.has(key)))
    visibleColumnKeys.value = keys.filter(key => allowed.has(key))
}, { deep: true })
function toggleColumn(key: string) {
  visibleColumnKeys.value = visibleColumnKeys.value.includes(key)
    ? visibleColumnKeys.value.filter(value => value !== key)
    : [...visibleColumnKeys.value, key]
}

const accountTableRef = useTemplateRef<{
  getScrollElement: () => HTMLElement | undefined
  getTableElement: () => HTMLTableElement | undefined
}>('accountTable')
const { onMouseDown, isDragging, overlayStyle } = useAccountSwipeSelect({
  getScrollElement: () => accountTableRef.value?.getScrollElement(),
  getTableElement: () => accountTableRef.value?.getTableElement(),
  rowIds: computed(() => accounts.value.map(row => row.id)),
  selectedIds,
  disabled: loading,
  invalidationKey: computed(() => JSON.stringify([
    accountPagination.value.currentPage,
    accountPagination.value.pageSize,
    searchQuery.value,
    providerQuery.value,
    statusQuery.value,
    groupQuery.value,
    planTypeQuery.value,
    sort.value,
    visibleColumnKeys.value,
    expandedRowKeys.value,
  ])),
})
</script>

<template>
  <div class="flex min-h-0 w-full flex-col xl:h-full xl:overflow-hidden">
    <BasePageHeader
      class="[&>div:first-child]:basis-0"
      title="账号管理"
      description="维护账号池，查看可用性、配额与使用状态"
    >
      <template #actions>
        <BaseIconButton
          :label="activeImportTaskCount ? `导入任务：${activeImportTaskCount} 个进行中` : '导入任务'"
          :class="activeImportTaskCount ? 'text-cp-primary-text' : 'text-cp-text-secondary'"
          @click="importTasksOpen = true"
        >
          <ListTodo :size="19" />
        </BaseIconButton>
        <BaseIconButton
          class="text-cp-primary-text"
          label="刷新账号列表"
          :loading="refreshing"
          @click="refreshAccounts"
        >
          <template #loading>
            <RefreshCw class="animate-spin motion-reduce:animate-none" :size="19" />
          </template>
          <RefreshCw :size="19" />
        </BaseIconButton>
      </template>
    </BasePageHeader>

    <AccountOverviewCards :summary="accountSummary" :groups="groups" :groups-loading="groupsLoading" />

    <BaseCard
      class="mt-4 flex flex-col xl:h-[calc(100dvh-250px)] xl:min-h-125"
    >
      <template #header>
        <AccountFilters
          v-model:search="searchQuery"
          v-model:status="statusQuery"
          v-model:provider="providerQuery"
          v-model:group="groupQuery"
          v-model:plan-type="planTypeQuery"
          :plan-options="[
            { label: '全部套餐', value: '' },
            { label: 'Free', value: 'free' },
            { label: 'Plus', value: 'plus' },
            { label: 'Pro', value: 'pro' },
            { label: 'Team', value: 'team' },
            { label: 'Business', value: 'business' },
            { label: 'Enterprise', value: 'enterprise' },
          ]"
          :column-options="accountColumnOptions"
          :visible-column-keys="visibleColumnKeys"
          :groups="groups"
          :groups-loading="groupsLoading"
          :selected-count="selectedIds.size"
          :batch-deleting="batchDeleting"
          :exporting-accounts="exportingAccounts"
          :template-applying="applyingTemplate"
          @delete-selected="showDeleteModal = true"
          @export-selected="handleExportAccounts"
          @create="openCreateAccount"
          @edit-selected="openBatchEdit"
          @toggle-column="toggleColumn"
        >
          <template #account-templates>
            <AccountTemplateMenu :account-ids="[...selectedIds]" :disabled="batchDeleting" @applying="applyingTemplate = $event" @applied="onTemplateApplied" />
          </template>
        </AccountFilters>
      </template>

      <template #body>
        <div class="flex min-h-0 flex-col xl:h-full">
          <BaseTable
            ref="accountTable"
            class="account-table h-100! min-h-100 flex-none [--cp-table-row-height:72px] xl:h-auto! xl:min-h-0 xl:flex-1"
            :class="{ 'select-none': isDragging }"
            column-layout="content"
            :columns="visibleAccountColumns"
            :rows="accounts"
            :loading="loading"
            :selected-row-keys="selectedRowKeys"
            :expanded-row-keys="expandedRowKeys"
            :sort="sort"
            empty-text="暂无账号数据"
            @sort-change="handleSortChange"
            @mousedown="onMouseDown"
          >
            <template #expander="{ row }">
              <button
                type="button"
                class="inline-flex size-6 cursor-pointer items-center justify-center rounded-md border-0 bg-transparent text-cp-text-secondary transition hover:bg-cp-bg-text-hover hover:text-cp-text"
                :title="expandedAccountIds.has(row.id) ? '收起统计' : '展开统计'"
                @click.stop="toggleExpanded(row.id)"
              >
                <ChevronDown
                  class="size-3.5 transition-transform"
                  :class="expandedAccountIds.has(row.id) ? '' : '-rotate-90'"
                />
              </button>
            </template>

            <template #header-selection>
              <BaseCheckbox
                :model-value="allSelected"
                :indeterminate="indeterminate"
                label="选择当前页账号"
                @update:model-value="toggleAll"
              />
            </template>

            <template #selection="{ row }">
              <BaseCheckbox
                :model-value="selectedIds.has(row.id)"
                label="选择账号"
                @update:model-value="toggleSelection(row.id)"
              />
            </template>

            <template #identity="{ row }">
              <AccountIdentityCell :account="row" :has-totp="hasReloginTotp(row)" />
            </template>

            <template #status="{ row }">
              <div class="grid w-full min-w-0 justify-items-center gap-1.5">
                <AccountStatusBadge
                  align="center"
                  :status="derivedAccountStatus(row)"
                  :error-reason="row.errorReason"
                  :error-message="row.errorMessage"
                  :rate-limited-until="row.quota.rateLimitedUntil"
                  :next-refresh-at="row.nextRefreshAt"
                />
                <AccountSchedulingSwitch
                  :enabled="row.enabled"
                  :status="derivedAccountStatus(row)"
                  :loading="togglingAccountIds.has(row.id)"
                  @change="value => handleToggleEnabled(row, value)"
                />
              </div>
            </template>

            <template #planType="{ row }">
              <div class="flex w-full min-w-0 justify-center" :title="row.planTypeDisplay">
                <AccountPlanBadge data-account-plan-mark :plan-type="row.planType" :plan-type-display="row.planTypeDisplay" />
              </div>
            </template>

            <template #capacity="{ row }">
              <AccountCapacityCell
                :in-flight="row.inFlight"
                :effective-concurrency-limit="row.effectiveConcurrencyLimit"
                :concurrency-limit="row.concurrencyLimit"
              />
            </template>

            <template #health="{ row }">
              <AccountHealthTimeline :buckets="row.healthTimeline" />
            </template>

            <template #usage="{ row }">
              <AccountQuotaSummaryCell
                :account="row"
                @forecast-requested="openQuotaForecast"
              />
            </template>

            <template #groups="{ row }">
              <div class="flex w-full min-w-0 justify-center">
                <AccountGroupMarks :groups="row.groups" layout="stacked" />
              </div>
            </template>

            <template #lastUsedAt="{ row }">
              <LastUsedAtCell :value="row.usage.lastUsedAt" />
            </template>

            <template #reloginCount="{ row }">
              <ReloginCountCell :count="row.reloginCount" :last-relogin-at="row.lastReloginAt" />
            </template>

            <template #actions="{ row }">
              <AccountTableActions
                :account="row"
                :deleting="deletingAccount"
                :recovering="recoveringAccountIds.has(row.id)"
                :refreshing="refreshingAccountIds.has(row.id)"
                :toggling-turn-state="togglingTurnStateAccountIds.has(row.id)"
                :testing="testingConnectionIds.has(row.id)"
                :relogin="reloginActions[row.id]"
                :relogin-unavailable="Boolean(reloginReadError)"
                @edit="openAccountEdit"
                @delete="requestDeleteAccount"
                @recover="handleRecover"
                @refresh="handleRefresh"
                @reauthorize="openReauthorizeAccount"
                @test="openConnectionTest"
                @toggle-turn-state="handleToggleTurnState"
                @relogin="requestRelogin"
              />
            </template>

            <template #expanded="{ row }">
              <div class="grid items-stretch gap-3 p-4 lg:grid-cols-[1.05fr_2.45fr] xl:min-h-77">
                <AccountQuotaPanel
                  :account="row"
                  :refreshing="refreshingQuotaAccountIds.has(row.id)"
                  @account-updated="void replaceAccount($event)"
                  @refresh-quota="handleRefreshQuota"
                />
                <AccountUsagePanel
                  :account="row"
                  @forecast-requested="openQuotaForecast"
                />
              </div>
            </template>
          </BaseTable>
          <BaseTablePagination
            :pagination="accountPagination"
            :loading="loading"
            @page-change="handlePageChange"
            @page-size-change="handlePageSizeChange"
          />
        </div>
      </template>
    </BaseCard>

    <AccountQuotaForecastModal
      v-if="forecastAccount"
      v-model="forecastOpen"
      :account="forecastAccount"
      :cache="forecastCache"
      @account-updated="void applyForecastAccount($event)"
    />

    <Teleport to="body">
      <div
        v-if="isDragging && overlayStyle"
        class="account-selection-marquee pointer-events-none fixed z-50 rounded border border-cp-primary bg-cp-primary/10"
        :style="overlayStyle"
        aria-hidden="true"
      />
    </Teleport>

    <AccountConnectionTestModal
      v-model="showConnectionTestModal"
      v-model:selected-model="connectionTestSelectedModel"
      v-model:stream="connectionTestStream"
      v-model:prompt="connectionTestPrompt"
      :account="testingAccount"
      :duration-ms="connectionTestDurationMs"
      :error="connectionTestError"
      :finished-at="connectionTestFinishedAt"
      :loading-models="loadingConnectionTestModels"
      :refreshing-models="refreshingConnectionTestModels"
      :logs="connectionTestLogs"
      :model="connectionTestModel"
      :model-options="connectionTestModelOptions"
      :started-at="connectionTestStartedAt"
      :status="connectionTestStatus"
      :status-view="connectionTestStatusView"
      @refresh-models="handleRefreshConnectionTestModels()"
      @test="handleTestConnection()"
    />

    <AccountImportTasks
      v-model="importTasksOpen"
      :tasks="importTasks"
      :selected-id="selectedImportTaskId"
      :detail="importTaskDetail"
      :loading="importTasksLoading"
      :stopping="importTaskStopping"
      :error="importTasksError"
      :stop-error="importTaskStopError"
      @select="selectImportTask"
      @refresh="refreshImportTasks()"
      @stop="stopImportTask"
      @view-accounts="importTasksOpen = false; void refreshAccounts()"
    />

    <AccountCreateModal
      v-model="showCreateModal"
      v-model:form="createForm"
      :account="reauthorizingAccount"
      :groups="groups"
      :groups-loading="groupsLoading"
      :oauth-loading="authorizingOAuth"
      :reauthorizing="Boolean(reauthorizingAccount)"
      :saving="creatingAccount"
      @create="handleCreate"
      @generate-oauth="handleAuthorizeOAuth"
    />

    <AccountEditModal
      v-model:custom-name="editingCustomName"
      v-model="showEditModal"
      v-model:enabled="schedulingEnabled"
      v-model:turn-state-injection-enabled="turnStateInjectionEnabled"
      v-model:concurrency-limit="editingConcurrencyLimit"
      v-model:weight="editingWeight"
      v-model:model-access="editingModelAccess"
      v-model:proxy-mode="editingProxyMode"
      v-model:proxy-id="editingProxyId"
      v-model:selected-group-ids="editingGroupIds"
      :account="editingAccount"
      :groups="groups"
      :groups-loading="groupsLoading"
      :saving="savingAccountEdit"
      @save="saveAccountEdit"
    />

    <AccountBatchEditModal
      v-model:custom-name="batchCustomName"
      v-model:update-custom-name="batchUpdateCustomName"
      v-model="showBatchEditModal"
      v-model:enabled="batchSchedulingEnabled"
      v-model:turn-state-injection-enabled="batchTurnStateInjectionEnabled"
      v-model:concurrency-limit="batchConcurrencyLimit"
      v-model:weight="batchWeight"
      v-model:model-access="batchModelAccess"
      v-model:update-model-access="batchUpdateModelAccess"
      v-model:proxy-mode="batchProxyMode"
      v-model:proxy-id="batchProxyId"
      v-model:selected-group-ids="batchGroupIds"
      v-model:update-enabled="batchUpdateEnabled"
      v-model:update-turn-state-injection-enabled="batchUpdateTurnStateInjectionEnabled"
      v-model:update-concurrency-limit="batchUpdateConcurrencyLimit"
      v-model:update-weight="batchUpdateWeight"
      v-model:update-groups="batchUpdateGroups"
      v-model:update-proxy="batchUpdateProxy"
      :catalog-account-id="batchCatalogAccountId"
      :has-updates="batchHasUpdates"
      :turn-state-available="batchTurnStateAvailable"
      :selected-count="selectedIds.size"
      :groups="groups"
      :groups-loading="groupsLoading"
      :saving="savingBatchEdit"
      @save="saveBatchEdit"
    />

    <BaseConfirmModal
      v-model="reloginOpen"
      title="确认失效重登"
      confirm-text="重登并同步"
      :loading="reloginSubmitting"
      :confirm-disabled="reloginConfirmDisabled"
      @confirm="confirmRelogin"
    >
      <dl class="space-y-2 break-all">
        <div>
          <dt class="text-cp-text-tertiary">
            邮箱
          </dt><dd>{{ reloginSelected?.email }}</dd>
        </div>
        <div>
          <dt class="text-cp-text-tertiary">
            工作区
          </dt><dd>{{ reloginSelected?.action.target?.workspace_id }}</dd>
        </div>
      </dl>
      <p v-if="reloginActionError" role="alert" class="mt-3 break-words text-cp-error">
        {{ reloginActionError }}
      </p>
      <p v-else-if="reloginConfirmDisabled" role="alert" class="mt-3 text-cp-warning-text">
        状态已变化或暂不可用，请关闭后重新确认。
      </p>
    </BaseConfirmModal>

    <BaseConfirmModal
      v-model="showExportModal"
      title="确认导出敏感信息"
      description="导出文件可能包含账号凭据，请妥善保存。"
      confirm-text="确认导出"
      :loading="exportingAccounts"
      @confirm="confirmExportAccounts"
    >
      <p class="m-0">
        确定导出选中的 {{ selectedIds.size }} 个账号吗？
      </p>
    </BaseConfirmModal>

    <BaseConfirmModal
      v-model="showDeleteModal"
      title="确认删除"
      description="删除后该账号将不再参与调度，此操作不可撤销"
      destructive
      confirm-text="确认删除"
      :loading="batchDeleting"
      @confirm="handleBatchDelete"
    >
      <p class="m-0">
        确定要删除选中的 {{ selectedIds.size }} 个账号吗？此操作不可撤销
      </p>
    </BaseConfirmModal>

    <BaseConfirmModal
      v-model="showSingleDeleteModal"
      title="删除账号"
      description="删除后该账号将不再参与调度，此操作不可撤销"
      destructive
      confirm-text="确认删除"
      :loading="deletingAccount"
      @confirm="handleDelete"
    >
      <p class="m-0">
        确定要删除
        {{
          pendingDeleteAccount?.email
            || pendingDeleteAccount?.accountId
            || pendingDeleteAccount?.id
            || '该账号'
        }}
        吗？
      </p>
    </BaseConfirmModal>
  </div>
</template>

<style scoped>
.account-table :deep(th[data-column-key='status'] [data-sort-button]),
.account-table :deep(th[data-column-key='planType'] [data-sort-button]) {
  position: relative;
}

.account-table :deep(th[data-column-key='status'] [data-sort-indicator]),
.account-table :deep(th[data-column-key='planType'] [data-sort-indicator]) {
  position: absolute;
  inset-inline-start: calc(100% + 4px);
}

.account-table :deep([data-column-key='groups']),
.account-table :deep([data-column-key='lastUsedAt']),
.account-table :deep([data-column-key='weight']),
.account-table :deep([data-column-key='addedAt']),
.account-table :deep([data-column-key='accessTokenExpiresAtDisplay']) {
  padding-inline: 12px;
}

.account-table :deep(td[data-column-key='identity']) {
  cursor: crosshair;
}

.account-table :deep([data-swipe-select-ignore]) {
  cursor: text;
}

@media (max-width: 639px) {
  .account-table :deep(th.sticky),
  .account-table :deep(td.sticky) {
    position: static;
    box-shadow: none;
  }
}
</style>
