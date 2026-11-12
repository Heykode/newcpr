<script setup lang="ts">
import type { Account, OutboundProxyRecord } from '@/api'
import { Search } from '@lucide/vue'
import { computed } from 'vue'
import AccountGroupMarks from '@/components/AccountGroupMarks.vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseTablePagination from '@/components/base/BaseTable/BaseTablePagination.vue'
import { defineTableColumns } from '@/components/base/BaseTable/columns'
import BaseTable from '@/components/base/BaseTable/index.vue'
import ProviderIconGroup from '@/components/ProviderIconGroup.vue'
import { t } from '@/i18n'
import AccountPlanBadge from '@/views/accounts/components/AccountPlanBadge.vue'
import { useProxyAccountAssignment } from '../composables/useProxyAccountAssignment'

const props = defineProps<{ proxy: OutboundProxyRecord | null }>()
const emit = defineEmits<{ assigned: [] }>()
const open = defineModel<boolean>({ required: true })

const {
  accounts,
  loading,
  error,
  search,
  pagination,
  setPage,
  setPageSize,
  selectedIds,
  selectedRowKeys,
  allSelected,
  indeterminate,
  toggleSelection,
  toggleAll,
  tooManySelected,
  maximumAccounts,
  assigning,
  assign,
} = useProxyAccountAssignment({
  isOpen: () => open.value,
  proxy: () => props.proxy,
  onAssigned: () => emit('assigned'),
})
const columns = computed(() => defineTableColumns<Account>([
  {
    key: 'selection',
    kind: 'selection',
    size: 'xs',
    align: 'center',
  },
  {
    key: 'identity',
    label: t('accounts.account'),
    kind: 'identity',
    size: '2xl',
  },
  {
    key: 'provider',
    label: t('proxies.accounts.platformType'),
    kind: 'custom',
    size: 'md',
    align: 'center',
  },
  {
    key: 'plan',
    label: t('proxies.accounts.subscription'),
    kind: 'custom',
    size: 'sm',
    align: 'center',
  },
  {
    key: 'groups',
    label: t('groups.group'),
    kind: 'custom',
    size: 'lg',
    align: 'center',
  },
  {
    key: 'outboundProxy',
    label: t('proxies.assignment.currentProxy'),
    kind: 'custom',
    size: '2xl',
  },
]))
const emptyText = computed(() => error.value || (search.value.trim()
  ? t('proxies.accounts.noMatches')
  : t('proxies.assignment.noAccounts')))

async function confirmAssignment() {
  if (await assign())
    open.value = false
}
</script>

<template>
  <BaseModal v-model="open" :title="t('proxies.assignment.title')" :description="proxy?.name" size="xl" :dismissible="!assigning">
    <div class="flex min-h-0 flex-col gap-3">
      <div class="flex min-w-0 items-center gap-2">
        <BaseInput v-model="search" class="min-w-0 flex-1 sm:w-80 sm:flex-none" :disabled="assigning" :aria-label="t('proxies.assignment.searchAccounts')" :placeholder="t('proxies.accounts.searchAccountNameOrEmail')">
          <template #prefix>
            <Search class="size-4.5 text-cp-text-tertiary" />
          </template>
        </BaseInput>
        <div v-if="selectedIds.size" class="ml-auto flex shrink-0 gap-2">
          <BaseButton variant="destructive" :disabled="assigning" @click="selectedIds = new Set()">
            {{ t('common.actions.reset') }}
          </BaseButton>
          <BaseButton variant="primary" :loading="assigning" :disabled="!proxy?.enabled || tooManySelected" @click="confirmAssignment">
            {{ t('proxies.assignment.assignAction', { count: selectedIds.size }) }}
          </BaseButton>
        </div>
      </div>
      <BaseTable class="min-h-0 shrink-0 [--cp-table-row-height:56px]" :style="{ height: 'min(50dvh, 28rem)' }" :columns="columns" :rows="accounts" :loading="loading" :empty-text="emptyText" :selected-row-keys="selectedRowKeys">
        <template #header-selection>
          <BaseCheckbox :model-value="allSelected" :indeterminate="indeterminate" :disabled="loading || assigning" :label="t('proxies.assignment.selectPage')" :show-label="false" @update:model-value="toggleAll" />
        </template>
        <template #selection="{ row }">
          <BaseCheckbox :model-value="selectedIds.has(row.id)" :disabled="loading || assigning || row.outboundProxyId === proxy?.id" :label="t('proxies.assignment.selectAccount', { name: row.name })" :show-label="false" @update:model-value="toggleSelection(row.id)" />
        </template>
        <template #identity="{ row }">
          <div class="min-w-0">
            <div class="truncate text-cp font-heavy text-cp-text" :title="row.name">
              {{ row.name }}
            </div>
            <div v-if="row.email && row.email !== row.name" class="mt-0.5 truncate font-mono text-cp-xs text-cp-text-secondary" :title="row.email">
              {{ row.email }}
            </div>
          </div>
        </template>
        <template #provider="{ row }">
          <ProviderIconGroup :provider="row.provider" :authentication-kind="row.authenticationKind" />
        </template>
        <template #plan="{ row }">
          <AccountPlanBadge :plan-type="row.planType" :plan-type-display="row.planTypeDisplay || '—'" />
        </template>
        <template #groups="{ row }">
          <div class="flex justify-center">
            <AccountGroupMarks :groups="row.groups" />
          </div>
        </template>
        <template #outboundProxy="{ row }">
          <span v-if="row.outboundProxyId === proxy?.id" class="text-cp-success-text">{{ t('proxies.assignment.alreadyAssigned') }}</span>
          <span v-else class="block truncate text-cp-xs text-cp-text-secondary" :title="row.outboundProxyEndpoint ?? undefined">
            {{ row.outboundProxyEndpoint || t('accounts.proxy.deploymentDefault') }}
          </span>
        </template>
      </BaseTable>
      <p v-if="tooManySelected" class="m-0 text-cp-sm text-cp-error-text" role="alert">
        {{ t('proxies.assignment.selectionLimit', { count: maximumAccounts }) }}
      </p>
      <p v-if="proxy && !proxy.enabled" class="m-0 text-cp-sm text-cp-warning-text" role="alert">
        {{ t('proxies.assignment.enableFirst') }}
      </p>
    </div>
    <template #footer>
      <BaseTablePagination :pagination="pagination" class="mt-0! w-full" :loading="loading || assigning" @page-change="setPage" @page-size-change="setPageSize" />
    </template>
  </BaseModal>
</template>
