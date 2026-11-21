<script setup lang="ts">
import type { ApiKey } from '@/api'
import { ref, shallowRef, watch } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'

import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import BaseTable from '@/components/base/BaseTable/index.vue'
import ClientImportModal from '@/components/ClientImportModal.vue'
import LastUsedAtCell from '@/components/LastUsedAtCell.vue'
import { useAccountGroupCatalog } from '@/composables/useAccountGroupCatalog'
import { usePageSelection } from '@/composables/usePageSelection'
import { t } from '@/i18n'
import ApiKeyActions from './components/ApiKeyActions.vue'
import ApiKeyBatchEditModal from './components/ApiKeyBatchEditModal.vue'
import ApiKeyBudgetAlignment from './components/ApiKeyBudgetAlignment.vue'
import ApiKeyBudgetCell from './components/ApiKeyBudgetCell.vue'
import ApiKeyBudgetResetModal from './components/ApiKeyBudgetResetModal.vue'
import ApiKeyCreateModal from './components/ApiKeyCreateModal.vue'
import ApiKeyFilters from './components/ApiKeyFilters.vue'
import ApiKeyIdentityCell from './components/ApiKeyIdentityCell.vue'
import ApiKeyPrefixCell from './components/ApiKeyPrefixCell.vue'
import ApiKeyScopeCell from './components/ApiKeyScopeCell.vue'
import ApiKeyStatisticsModal from './components/ApiKeyStatisticsModal.vue'
import ApiKeyStatusBadge from './components/ApiKeyStatusBadge.vue'
import ApiKeyUseModal from './components/ApiKeyUseModal.vue'
import { useApiKeyMutations } from './composables/useApiKeyMutations'
import { useApiKeysQuery } from './composables/useApiKeysQuery'
import { useApiKeyUse } from './composables/useApiKeyUse'
import { apiKeyColumns } from './constants'

const showBatchEdit = shallowRef(false)
const statisticsKey = shallowRef<ApiKey | null>(null)
const alignmentKey = shallowRef<ApiKey | null>(null)
const showAlignment = shallowRef(false)
const alignmentBusy = shallowRef(false)
const alignment = ref<InstanceType<typeof ApiKeyBudgetAlignment>>()
const selectedIds = ref<Set<string>>(new Set())
const {
  loading,
  apiKeys,
  loadApiKeys,
  searchQuery,
  sort,
  apiKeyPagination,
  handlePageChange,
  handlePageSizeChange,
  handleSortChange,
} = useApiKeysQuery()

const {
  groups,
  loading: loadingGroups,
  loadGroups,
} = useAccountGroupCatalog({ immediate: false })

function openAlignment(key: ApiKey) {
  alignmentKey.value = key
  showAlignment.value = true
}
function batchSaved(ids: string[]) {
  selectedIds.value = new Set([...selectedIds.value].filter(id => !ids.includes(id)))
  void loadApiKeys()
}
watch(showBatchEdit, open => open && void loadGroups())

const {
  showBudgetResetModal,
  pendingBudgetKey,
  budgetPeriod,
  resettingBudget,
  requestBudgetReset,
  handleBudgetReset,
  showFormModal,
  showDeleteModal,
  showSingleDeleteModal,
  showKeyModal,
  showAllAccountsConfirm,
  createdKey,
  createdKeyName,
  editingKey,
  pendingDeleteKey,
  savingKey,
  deletingKey,
  batchDeleting,
  deletingCount,
  updatingStatusKeyIds,
  revealingKeyIds,
  copyingKeyIds,
  configurationKeyIds,
  importingKeyIds,
  form,
  openCreate,
  openEdit,
  clearForm,
  clearCreatedKey,
  requestSave,
  confirmAllAccountsScope,
  requestDeleteKey,
  handleDelete,
  handleBatchDelete,
  handleToggleStatus,
  copyToClipboard,
  revealPlaintextKey,
  copyApiKey,
} = useApiKeyMutations({ selectedIds, reload: loadApiKeys })

const { allSelected, indeterminate, selectedRowKeys, toggleSelection, toggleAll } = usePageSelection(
  apiKeys,
  selectedIds,
)

const {
  showUseKeyModal,
  showImportModal,
  confirmImport,
  clearImport,
  selectedUseKey,
  openAiBaseUrl,
  importCreatedKeyToCcs,
  openUseKeyModal,
  clearUseKey,
  importToCcs,
} = useApiKeyUse({
  createdKey,
  createdKeyName,
  revealPlaintextKey,
})

watch(
  showFormModal,
  open => open && void loadGroups(),
)
</script>

<template>
  <div class="flex h-full min-h-0 w-full flex-col overflow-hidden">
    <ApiKeyBatchEditModal v-model="showBatchEdit" :ids="[...selectedIds]" :groups="groups" :group-loading="loadingGroups" @saved="batchSaved" />
    <ApiKeyStatisticsModal v-if="statisticsKey" :api-key="statisticsKey" @close="statisticsKey = null" />
    <BaseModal v-model="showAlignment" title="对齐本期预算窗口" description="本期沿用参考账号的日或周窗口，到期恢复密钥自身周期" :dismissible="!alignmentBusy" @after-leave="!showAlignment && (alignmentKey = null)">
      <ApiKeyBudgetAlignment v-if="alignmentKey" :key="alignmentKey.id" ref="alignment" :active="showAlignment" :api-key="alignmentKey" @busy="alignmentBusy = $event" @saved="showAlignment = false; loadApiKeys()" />
      <template #footer>
        <BaseButton :disabled="alignmentBusy" @click="showAlignment = false">
          取消
        </BaseButton>
        <BaseButton variant="primary" :loading="alignmentBusy" :disabled="!alignment?.canSubmit" @click="alignment?.submit()">
          确认对齐
        </BaseButton>
      </template>
    </BaseModal>
    <BasePageHeader
      :title="t('navigation.apiKeys')"
      :description="t('keys.page.description')"
    />

    <BaseCard
      class="mt-5 flex h-[calc(100dvh-196px)] min-h-125 flex-col"
    >
      <template #header>
        <ApiKeyFilters
          v-model:search="searchQuery"
          :batch-deleting="batchDeleting"
          :selected-count="selectedIds.size"
          @create="openCreate"
          @edit-selected="showBatchEdit = true"
          @delete-selected="showDeleteModal = true"
        />
      </template>

      <template #body>
        <div class="flex h-full min-h-0 flex-col">
          <BaseTable
            class="min-h-0 flex-1"
            :columns="apiKeyColumns"
            :rows="apiKeys"
            :loading="loading"
            :selected-row-keys="selectedRowKeys"
            :sort="sort"
            :empty-text="t('keys.page.noApiKeyYet')"
            @sort-change="handleSortChange"
          >
            <template #header-selection>
              <BaseCheckbox
                :model-value="allSelected"
                :indeterminate="indeterminate"
                :label="t('keys.page.selectCurrentPageKey')"
                @update:model-value="toggleAll"
              />
            </template>
            <template #selection="{ row }">
              <BaseCheckbox
                :model-value="selectedIds.has(row.id)"
                :label="t('keys.page.selectKey')"
                @update:model-value="toggleSelection(row.id)"
              />
            </template>
            <template #identity="{ row }">
              <ApiKeyIdentityCell :api-key="row" />
            </template>
            <template #prefix="{ row }">
              <ApiKeyPrefixCell
                :prefix="row.prefix"
                :revealing="copyingKeyIds.has(row.id)"
                @copy="copyApiKey(row)"
              />
            </template>
            <template #scope="{ row }">
              <ApiKeyScopeCell :api-key="row" />
            </template>
            <template #budget="{ row }">
              <ApiKeyBudgetCell :api-key="row" />
            </template>
            <template #limits="{ row }">
              <dl class="m-0 grid grid-cols-[auto_minmax(0,1fr)] gap-x-2 gap-y-1 text-xs tabular-nums">
                <dt class="text-cp-text-tertiary">
                  {{ t('keys.page.concurrency') }}
                </dt>
                <dd class="m-0 truncate text-cp-text" :title="String(row.maxConcurrency || '∞')">
                  {{ row.maxConcurrency || '∞' }}
                </dd>
                <dt class="text-cp-text-tertiary">
                  RPM
                </dt>
                <dd class="m-0 truncate text-cp-text" :title="String(row.requestsPerMinute || '∞')">
                  {{ row.requestsPerMinute || '∞' }}
                </dd>
              </dl>
            </template>
            <template #enabled="{ row }">
              <ApiKeyStatusBadge :api-key="row" />
            </template>
            <template #lastUsedAt="{ row }">
              <LastUsedAtCell :value="row.lastUsedAt" />
            </template>
            <template #actions="{ row }">
              <ApiKeyActions
                :api-key="row"
                :configuration-loading="configurationKeyIds.has(row.id)"
                :import-loading="importingKeyIds.has(row.id)"
                :deleting="deletingKey"
                :revealing="revealingKeyIds.has(row.id)"
                :updating-status="updatingStatusKeyIds.has(row.id)"
                @edit="openEdit"
                @statistics="statisticsKey = $event"
                @align-budget="openAlignment"
                @reset-budget="requestBudgetReset"
                @delete="requestDeleteKey"
                @import-ccs="importToCcs"
                @toggle="handleToggleStatus"
                @use="openUseKeyModal"
              />
            </template>
          </BaseTable>
          <BaseTablePagination
            :pagination="apiKeyPagination"
            :loading="loading"
            @page-change="handlePageChange"
            @page-size-change="handlePageSizeChange"
          />
        </div>
      </template>
    </BaseCard>

    <ApiKeyBudgetResetModal
      v-model="showBudgetResetModal"
      v-model:period="budgetPeriod"
      :api-key="pendingBudgetKey"
      :loading="resettingBudget"
      @confirm="handleBudgetReset"
      @after-leave="!showBudgetResetModal && (pendingBudgetKey = null)"
    />

    <ApiKeyCreateModal
      v-model="showFormModal"
      v-model:created-open="showKeyModal"
      v-model:form="form"
      :groups="groups"
      :group-loading="loadingGroups"
      :editing="Boolean(editingKey)"
      :created-key="createdKey"
      :saving="savingKey"
      @copy="copyToClipboard"
      @save="requestSave"
      @after-leave="clearForm"
      @created-after-leave="clearCreatedKey"
      @import-ccs="importCreatedKeyToCcs"
    />

    <ClientImportModal v-model="showImportModal" @select="confirmImport" @after-leave="clearImport" />

    <ApiKeyUseModal
      v-model="showUseKeyModal"
      :api-key="selectedUseKey"
      :api-base-url="openAiBaseUrl"
      @copy="copyToClipboard"
      @after-leave="clearUseKey"
    />

    <BaseConfirmModal
      v-model="showAllAccountsConfirm"
      title="允许访问全部账号"
      description="保存后也可访问未分组和以后新增的账号"
      confirm-text="允许访问"
      :loading="savingKey"
      @confirm="confirmAllAccountsScope"
    />

    <BaseConfirmModal
      v-model="showDeleteModal"
      :title="t('common.actions.delete')"
      :description="t('keys.page.batchDeleteWarning')"
      destructive
      :confirm-text="t('common.actions.delete')"
      :loading="batchDeleting"
      @confirm="handleBatchDelete"
    >
      <p class="m-0">
        确定删除选中的 {{ deletingCount }} 个 API Key 吗？
      </p>
    </BaseConfirmModal>

    <BaseConfirmModal
      v-model="showSingleDeleteModal"
      :title="t('keys.page.deleteApiKey')"
      :description="t('keys.page.deleteWarning')"
      destructive
      :confirm-text="t('common.actions.delete')"
      :loading="deletingKey"
      @confirm="handleDelete"
      @after-leave="!showSingleDeleteModal && (pendingDeleteKey = null)"
    >
      <p class="m-0">
        确定删除 {{ pendingDeleteKey?.name || pendingDeleteKey?.prefix || '该 API Key' }} 吗？
      </p>
    </BaseConfirmModal>
  </div>
</template>
