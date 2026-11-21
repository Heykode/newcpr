<script setup lang="ts">
import type { AccountGroup } from '@/api'
import { Download, MoreHorizontal, Pencil, RotateCcw, Search, Settings2, Trash2, Upload, X } from '@lucide/vue'
import { computed } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'

import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseMenuItem from '@/components/base/BaseMenuItem.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import ProviderFilterSegmented from '@/components/ProviderFilterSegmented.vue'
import { t } from '@/i18n'
import { accountStatusFilterOptions } from '../constants'

const props = defineProps<{
  selectedCount: number
  hideColumns?: boolean
  batchDeleting: boolean
  exportingAccounts: boolean
  templateApplying?: boolean
  groups: AccountGroup[]
  groupsLoading: boolean
  planOptions: { label: string, value: string }[]
  columnOptions: { key: string, label: string }[]
  visibleColumnKeys: string[]
}>()

const emit = defineEmits<{
  deleteSelected: []
  exportSelected: []
  create: []
  editSelected: []
  toggleColumn: [key: string]
}>()

const search = defineModel<string>('search', { required: true })
const status = defineModel<string>('status', { required: true })
const provider = defineModel<string>('provider', { required: true })
const group = defineModel<string>('group', { required: true })
const planType = defineModel<string>('planType', { required: true })
const groupOptions = computed(() => [
  { label: '全部分组', value: '' },
  { label: '未分组账号', value: 'ungrouped' },
  ...props.groups.map(group => ({
    label: group.enabled ? group.name : `${group.name}（已禁用）`,
    value: group.id,
  })),
])
const activeFilters = computed(() => [
  { key: 'search', label: '搜索', value: search.value },
  { key: 'status', label: '状态', value: status.value, text: accountStatusFilterOptions.find(item => item.value === status.value)?.label },
  { key: 'group', label: '分组', value: group.value, text: groupOptions.value.find(item => item.value === group.value)?.label },
  { key: 'plan', label: '套餐', value: planType.value, text: props.planOptions.find(item => item.value === planType.value)?.label },
  { key: 'provider', label: '平台', value: provider.value, text: provider.value === 'openai' ? 'Codex' : 'xAI' },
].filter(item => item.value))
function clearFilter(key: string) {
  if (key === 'search')
    search.value = ''
  if (key === 'status')
    status.value = ''
  if (key === 'group')
    group.value = ''
  if (key === 'plan')
    planType.value = ''
  if (key === 'provider')
    provider.value = ''
}
function resetFilters() {
  for (const filter of activeFilters.value) clearFilter(filter.key)
}
</script>

<template>
  <div
    class="flex w-full flex-col gap-3"
    role="group"
    :aria-label="t('accounts.filters.label')"
  >
    <div class="account-filter-fields">
      <BaseFormItem label="搜索账号" class="account-filter-search">
        <BaseInput v-model="search" :placeholder="t('accounts.filters.searchLabel')" :aria-label="t('accounts.filters.searchLabel')" clearable>
          <template #prefix>
            <Search class="size-4 text-cp-text-tertiary" />
          </template>
        </BaseInput>
      </BaseFormItem>
      <BaseFormItem label="账号状态">
        <BaseSelect v-model="status" :options="accountStatusFilterOptions" :aria-label="t('accounts.filters.statusLabel')" />
      </BaseFormItem>
      <BaseFormItem label="账号分组">
        <BaseSelect v-model="group" :options="groupOptions" :disabled="groupsLoading" :aria-label="t('accounts.filters.groupLabel')" />
      </BaseFormItem>
      <BaseFormItem label="账号套餐" class="account-filter-plan">
        <BaseSelect v-model="planType" :options="planOptions" aria-label="按套餐筛选" />
      </BaseFormItem>
      <BaseFormItem label="平台" class="account-filter-provider">
        <ProviderFilterSegmented v-model="provider" class="w-full" />
      </BaseFormItem>
    </div>
    <div v-if="activeFilters.length" class="flex flex-wrap items-center gap-2" aria-label="已选账号筛选">
      <span v-for="filter in activeFilters" :key="filter.key" class="inline-flex max-w-full items-center gap-1 rounded-cp-sm bg-cp-primary-container px-2 text-cp-xs text-cp-primary-on-container">
        <span class="min-w-0 break-all">{{ filter.label }}：{{ filter.text || filter.value }}</span>
        <BaseIconButton :label="`清除${filter.label}筛选`" size="sm" @click="clearFilter(filter.key)"><X :size="12" /></BaseIconButton>
      </span>
      <BaseButton variant="link" size="sm" aria-label="重置账号筛选" @click="resetFilters">
        <RotateCcw :size="13" />重置筛选
      </BaseButton>
    </div>
    <div
      class="flex w-full min-w-0 flex-wrap items-center gap-2 border-t border-cp-border-secondary pt-3"
      role="group"
      aria-label="账号操作"
    >
      <span class="mr-auto text-cp-xs text-cp-text-secondary" aria-live="polite">{{ selectedCount ? `已选择 ${selectedCount} 个账号` : '账号工具' }}</span>
      <BasePopover v-if="!hideColumns" placement="bottom-start">
        <template #trigger>
          <BaseButton variant="secondary" size="sm" title="列显示设置">
            <Settings2 class="size-3.5" />列显示
          </BaseButton>
        </template>
        <div class="grid w-52 gap-2 p-3">
          <p class="m-0 text-cp-sm font-semibold text-cp-text-secondary">
            显示列
          </p>
          <BaseCheckbox v-for="column in columnOptions" :key="column.key" :model-value="visibleColumnKeys.includes(column.key)" :label="`显示${column.label}`" show-label @update:model-value="emit('toggleColumn', column.key)" />
        </div>
      </BasePopover>
      <slot name="account-templates" />
      <BasePopover v-if="selectedCount > 0" placement="bottom-end">
        <template #trigger="{ open }">
          <BaseIconButton label="批量操作" variant="ghost" :aria-expanded="open">
            <MoreHorizontal class="size-4.5" />
          </BaseIconButton>
        </template>
        <template #default="{ close }">
          <div class="w-44 p-1">
            <BaseMenuItem :disabled="templateApplying" @click="(close(), emit('editSelected'))">
              <template #icon>
                <Pencil class="size-3.5" />
              </template>
              {{ t('accounts.filters.editSelected', { count: selectedCount }) }}
            </BaseMenuItem>
            <BaseMenuItem :loading="exportingAccounts" @click="(close(), emit('exportSelected'))">
              <template #icon>
                <Download class="size-3.5" />
              </template>
              导出选中 ({{ selectedCount }})
            </BaseMenuItem>
            <BaseMenuItem tone="destructive" :disabled="templateApplying" :loading="batchDeleting" @click="(close(), emit('deleteSelected'))">
              <template #icon>
                <Trash2 class="size-3.5" />
              </template>
              {{ t('accounts.filters.deleteSelected', { count: selectedCount }) }}
            </BaseMenuItem>
          </div>
        </template>
      </BasePopover>
      <BaseButton
        variant="primary"
        class="whitespace-nowrap xl:w-auto"
        @click="emit('create')"
      >
        <Upload class="size-4" />
        {{ t('accounts.filters.importAccount') }}
      </BaseButton>
    </div>
  </div>
</template>

<style scoped>
.account-filter-fields {
  display: flex;
  flex-wrap: wrap;
  align-items: end;
  gap: 12px;
}
.account-filter-fields > * {
  flex: 1 1 140px;
  min-width: 0;
  margin: 0;
}
.account-filter-fields > .account-filter-search {
  flex: 2 1 260px;
}
.account-filter-fields > .account-filter-provider {
  flex: 0 1 130px;
}
.account-filter-fields > .account-filter-plan {
  flex-basis: 230px;
}
@media (max-width: 600px) {
  .account-filter-fields > .account-filter-search {
    flex-basis: 100%;
  }
  .account-filter-fields > * {
    flex-basis: 130px;
  }
}
</style>
