<script setup lang="ts">
import type { AccountGroup } from '@/api'
import { Download, MoreHorizontal, Pencil, Search, Settings2, Trash2, Upload } from '@lucide/vue'
import { computed } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
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
</script>

<template>
  <div
    class="flex w-full flex-col gap-3 xl:flex-row xl:flex-wrap xl:items-center"
    role="group"
    :aria-label="t('accounts.filters.label')"
  >
    <div
      class="grid w-full min-w-0 grid-cols-2 items-center gap-2 sm:grid-cols-[minmax(0,1fr)_minmax(0,1fr)_7.75rem] xl:flex xl:w-auto xl:flex-none xl:flex-wrap xl:gap-3"
    >
      <BaseInput
        v-model="search"
        :placeholder="t('accounts.filters.searchLabel')"
        class="col-span-2 min-w-0 sm:col-span-3 xl:w-80 xl:flex-none"
      >
        <template #prefix>
          <Search class="size-4.5 text-cp-text-tertiary" />
        </template>
      </BaseInput>

      <BaseSelect
        v-model="status"
        :options="accountStatusFilterOptions"
        :aria-label="t('accounts.filters.statusLabel')"
        class="w-full min-w-0 xl:w-40 xl:shrink-0"
      />

      <BaseSelect
        v-model="group"
        :options="groupOptions"
        :disabled="groupsLoading"
        :aria-label="t('accounts.filters.groupLabel')"
        class="w-full min-w-0 xl:w-40 xl:shrink-0"
      />

      <BaseSelect
        v-model="planType"
        :options="planOptions"
        aria-label="按套餐筛选"
        class="w-full min-w-0 xl:w-40 xl:shrink-0"
      />

      <BasePopover v-if="!hideColumns" placement="bottom-start">
        <template #trigger>
          <BaseButton variant="secondary" class="w-full min-w-0 xl:w-auto" title="列显示设置">
            <Settings2 class="size-4" />
            列显示
          </BaseButton>
        </template>
        <div class="grid w-52 gap-2 p-3">
          <p class="m-0 text-cp-sm font-heavy text-cp-text-secondary">
            显示列
          </p>
          <BaseCheckbox
            v-for="column in columnOptions"
            :key="column.key"
            :model-value="visibleColumnKeys.includes(column.key)"
            :label="`显示${column.label}`"
            show-label
            @update:model-value="emit('toggleColumn', column.key)"
          />
        </div>
      </BasePopover>

      <ProviderFilterSegmented
        v-model="provider"
        class="col-span-2 w-full sm:col-span-1 xl:w-31 xl:shrink-0"
      />
    </div>

    <div
      class="flex w-full min-w-0 max-w-full flex-wrap items-center justify-end gap-2 xl:ml-auto xl:w-auto xl:shrink-0 xl:self-end"
    >
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
