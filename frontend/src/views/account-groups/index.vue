<script setup lang="ts">
import BaseCard from '@/components/base/BaseCard.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseConfirmModal from '@/components/base/BaseConfirmModal.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import BaseTable from '@/components/base/BaseTable/index.vue'
import { usePageSelection } from '@/composables/usePageSelection'
import { t } from '@/i18n'
import AccountGroupActions from './components/AccountGroupActions.vue'
import AccountGroupFilters from './components/AccountGroupFilters.vue'
import AccountGroupFormModal from './components/AccountGroupFormModal.vue'
import AccountGroupMetricsCell from './components/AccountGroupMetricsCell.vue'
import { useAccountGroups } from './composables/useAccountGroups'
import { accountGroupColumns } from './constants'

const {
  groups,
  loading,
  pagination,
  searchQuery,
  showFormModal,
  showDeleteModal,
  showBatchDeleteModal,
  showDisableModal,
  selectedIds,
  editingGroup,
  pendingDeleteGroup,
  pendingDisableGroup,
  form,
  saving,
  deleting,
  batchDeleting,
  deletingCount,
  disabling,
  updatingStatusGroupIds,
  referencedKeyNames,
  openCreate,
  openEdit,
  clearForm,
  save,
  requestToggle,
  confirmDisable,
  requestDelete,
  confirmDelete,
  confirmBatchDelete,
  handlePageChange,
  handlePageSizeChange,
} = useAccountGroups()

const { allSelected, indeterminate, selectedRowKeys, toggleSelection, toggleAll }
  = usePageSelection(groups, selectedIds)
</script>

<template>
  <div class="flex h-full min-h-0 w-full flex-col overflow-hidden">
    <BasePageHeader
      :title="t('navigation.groups')"
      :description="t('groups.page.description')"
    />

    <BaseCard
      class="mt-5 flex h-[calc(100dvh-136px)] min-h-125 flex-col"
    >
      <template #header>
        <AccountGroupFilters
          v-model:search="searchQuery"
          :batch-deleting="batchDeleting"
          :selected-count="selectedIds.size"
          @create="openCreate"
          @delete-selected="showBatchDeleteModal = true"
        />
      </template>

      <template #body>
        <div class="flex h-full min-h-0 flex-col">
          <BaseTable
            class="min-h-0 flex-1"
            :columns="accountGroupColumns"
            :rows="groups"
            :loading="loading"
            :selected-row-keys="selectedRowKeys"
            empty-text="暂无分组，请点击创建分组创建"
          >
            <template #header-selection>
              <BaseCheckbox
                :model-value="allSelected"
                :indeterminate="indeterminate"
                :label="t('groups.page.selectCurrentPageGroup')"
                @update:model-value="toggleAll"
              />
            </template>
            <template #selection="{ row }">
              <BaseCheckbox
                :model-value="selectedIds.has(row.id)"
                :label="t('groups.page.selectGroup')"
                @update:model-value="toggleSelection(row.id)"
              />
            </template>
            <template #identity="{ row }">
              <div class="grid min-w-0 gap-1">
                <strong class="truncate text-cp text-cp-text">
                  {{ row.name }}
                </strong>
                <span class="truncate text-cp-xs font-emphasis text-cp-text-quaternary">
                  {{ row.description || '未填写描述' }}
                </span>
              </div>
            </template>

            <template #color="{ row }">
              <span
                class="mx-auto block size-4 rounded-sm"
                :style="{ backgroundColor: row.color }"
                :title="row.color"
                :aria-label="t('groups.page.colorLabel', { color: row.color })"
              />
            </template>

            <template #enabled="{ row }">
              <span
                class="inline-flex h-6 items-center rounded-lg px-2 text-cp-xs font-bold"
                :class="row.enabled
                  ? 'bg-cp-success-container text-cp-success-on-container'
                  : 'bg-cp-fill-tertiary text-cp-text-quaternary'"
              >
                {{ row.enabled ? t('common.status.enabled') : t('common.status.disabled') }}
              </span>
            </template>

            <template #accountCount="{ row }">
              <AccountGroupMetricsCell :group="row" kind="accounts" />
            </template>

            <template #allowImageGeneration="{ row }">
              <span :class="row.allowImageGeneration ? 'text-cp-text-secondary' : 'text-cp-warning-text'">
                {{ row.allowImageGeneration ? '允许' : '禁止' }}
              </span>
            </template>
            <template #fastMode="{ row }">
              <span class="text-cp-xs" :class="row.fastMode === 'enabled' ? 'text-cp-warning-text' : 'text-cp-text-secondary'">
                {{ row.fastMode === 'enabled' ? '开启' : row.fastMode === 'disabled' ? '关闭' : '默认' }}
              </span>
            </template>

            <template #capacity="{ row }">
              <AccountGroupMetricsCell :group="row" kind="capacity" />
            </template>

            <template #usage="{ row }">
              <AccountGroupMetricsCell :group="row" kind="usage" />
            </template>

            <template #actions="{ row }">
              <AccountGroupActions
                :group="row"
                :deleting="deleting"
                :updating-status="updatingStatusGroupIds.has(row.id)"
                @edit="openEdit"
                @toggle="requestToggle"
                @delete="requestDelete"
              />
            </template>
          </BaseTable>
          <BaseTablePagination
            :pagination="pagination"
            :loading="loading"
            @page-change="handlePageChange"
            @page-size-change="handlePageSizeChange"
          />
        </div>
      </template>
    </BaseCard>

    <AccountGroupFormModal
      v-model="showFormModal"
      v-model:form="form"
      :group="editingGroup"
      :saving="saving"
      @save="save"
      @after-leave="clearForm"
    />

    <BaseConfirmModal
      v-model="showBatchDeleteModal"
      :title="t('common.actions.delete')"
      :description="t('groups.page.batchDeleteWarning')"
      destructive
      :confirm-text="t('common.actions.delete')"
      :loading="batchDeleting"
      @confirm="confirmBatchDelete"
    >
      <p class="m-0">
        确定删除选中的 {{ deletingCount }} 个分组吗？账号本身不会被删除。
      </p>
    </BaseConfirmModal>

    <BaseConfirmModal
      v-model="showDisableModal"
      :title="t('groups.page.disableAccountGrouping')"
      description="禁用后，使用该分组的 API 密钥将无法再使用其中的账号。"
      :confirm-text="t('groups.page.confirmDisable')"
      :loading="disabling"
      @confirm="confirmDisable"
      @after-leave="!showDisableModal && (pendingDisableGroup = null)"
    >
      <p class="m-0">
        确定禁用“{{ pendingDisableGroup?.name }}”吗？
      </p>
      <p v-if="referencedKeyNames.length > 0" class="mt-2 mb-0 text-cp-warning-text">
        将影响 {{ referencedKeyNames.length }} 个 API 密钥：{{ referencedKeyNames.join('、') }}
      </p>
      <p v-else-if="pendingDisableGroup?.clientKeyCount" class="mt-2 mb-0 text-cp-warning-text">
        将影响 {{ pendingDisableGroup.clientKeyCount }} 个 API 密钥。
      </p>
    </BaseConfirmModal>

    <BaseConfirmModal
      v-model="showDeleteModal"
      :title="t('groups.page.deleteAccountGroup')"
      :description="t('groups.page.deleteWarning')"
      destructive
      :confirm-text="t('common.actions.delete')"
      :loading="deleting"
      @confirm="confirmDelete"
      @after-leave="!showDeleteModal && (pendingDeleteGroup = null)"
    >
      <p class="m-0">
        确定删除“{{ pendingDeleteGroup?.name || '该分组' }}”吗？账号本身不会被删除。
      </p>
    </BaseConfirmModal>
  </div>
</template>
