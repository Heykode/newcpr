<script setup lang="ts">
import type { AccountRow } from '../constants'
import type { BaseTableProps, BaseTableSort } from '@/components/base/BaseTable/columns'
import { ArrowDown, ArrowUp } from '@lucide/vue'
import { computed, shallowRef, watch } from 'vue'
import BaseEmpty from '@/components/base/BaseEmpty.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { cellDisplayValue, columnSortKey } from '@/components/base/BaseTable/columns'
import { derivedAccountStatus } from '../constants'
import AccountIdentityCell from './AccountIdentityCell.vue'

defineOptions({ inheritAttrs: false })
const props = withDefaults(defineProps<BaseTableProps<AccountRow> & {
  hasTotp?: (account: AccountRow) => boolean
}>(), {
  loading: false,
  selectedRowKeys: () => [],
  expandedRowKeys: () => [],
  emptyText: '暂无账号数据',
})
const emit = defineEmits<{ sortChange: [sort: BaseTableSort | undefined] }>()
const retainedRows = shallowRef<AccountRow[]>([])
watch([() => props.rows, () => props.loading], ([rows, loading]) => {
  if (rows.length || !loading)
    retainedRows.value = rows
}, { immediate: true })
const displayRows = computed(() => props.loading && !props.rows.length ? retainedRows.value : props.rows)
const sortOptions = computed(() => [
  { label: '默认排序', value: '' },
  ...props.columns.filter(column => column.sortable).map(column => ({
    label: column.label ?? column.key,
    value: columnSortKey(column),
  })),
])
const details = computed(() => props.columns.filter(column => ![
  'identity',
  'selection',
  'expander',
  'actions',
  'status',
  'planType',
  'usage',
  'capacity',
  'groups',
  'health',
].includes(column.key)))
const tones = {
  normal: 'border-l-cp-success',
  disabled: 'border-l-cp-border',
  error: 'border-l-cp-error',
  rate_limited: 'border-l-cp-warning',
  quota_exhausted: 'border-l-cp-warning',
}

function changeSort(key: string) {
  emit('sortChange', key ? { key, direction: props.sort?.direction ?? 'asc' } : undefined)
}
</script>

<template>
  <section data-account-mobile-list class="min-w-0" aria-label="账号列表" :aria-busy="loading">
    <div class="mb-3 flex min-w-0 flex-wrap items-center justify-between gap-2">
      <div class="flex min-h-9 items-center gap-2 text-cp-sm text-cp-text-secondary">
        <slot name="header-selection" />
        <span>当前页</span>
        <span v-if="selectedRowKeys.length" class="text-cp-primary-text">已选 {{ selectedRowKeys.length }}</span>
      </div>
      <div class="flex min-w-0 items-center gap-1">
        <BaseSelect
          class="w-32"
          aria-label="账号排序"
          :model-value="sort?.key ?? ''"
          :options="sortOptions"
          :disabled="loading"
          @update:model-value="changeSort"
        />
        <BaseIconButton
          :label="sort?.direction === 'desc' ? '切换为升序' : '切换为降序'"
          :disabled="!sort || loading"
          @click="sort && emit('sortChange', { key: sort.key, direction: sort.direction === 'asc' ? 'desc' : 'asc' })"
        >
          <ArrowDown v-if="sort?.direction === 'desc'" class="size-4" />
          <ArrowUp v-else class="size-4" />
        </BaseIconButton>
      </div>
    </div>
    <div v-loading="loading" class="relative grid min-h-24 min-w-0 gap-2">
      <article
        v-for="row in displayRows"
        :key="row.id"
        :data-account-card="row.id"
        :data-selected="selectedRowKeys.includes(row.id) || undefined"
        :aria-label="row.customName || row.email || '未命名账号'"
        class="min-w-0 rounded-lg border border-l-[3px] border-cp-border-secondary bg-cp-bg-container px-3 py-2"
        :class="[tones[derivedAccountStatus(row)], selectedRowKeys.includes(row.id) ? 'ring-2 ring-cp-primary' : undefined]"
      >
        <AccountIdentityCell :account="row" :has-totp="hasTotp?.(row)" mobile>
          <template #selection>
            <slot name="selection" :row="row" />
          </template>
          <template #actions>
            <slot name="actions" :row="row" />
          </template>
          <template #status>
            <div class="min-w-0 flex-1">
              <slot name="status" :row="row" />
            </div>
          </template>
        </AccountIdentityCell>
        <div class="my-1.5 min-w-0 border-y border-cp-border-secondary">
          <slot name="usage" :row="row" />
        </div>
        <div data-account-mobile-footer class="grid min-w-0 grid-cols-[auto_minmax(0,1fr)_32px] items-center gap-x-2">
          <div class="flex min-w-0 items-center gap-1.5 text-xs text-cp-text-secondary">
            <span>容量</span><slot name="capacity" :row="row" />
          </div>
          <div class="min-w-0" aria-label="账号分组">
            <slot name="groups" :row="row" />
          </div>
          <slot name="expander" :row="row" />
        </div>
        <section
          v-if="expandedRowKeys.includes(row.id)"
          :id="`account-details-${row.id}`"
          class="mt-3 min-w-0 border-t border-cp-border-secondary pt-3"
          aria-label="账号详细统计"
        >
          <dl class="m-0 grid min-w-0 grid-cols-[auto_minmax(0,1fr)] items-center gap-x-3 gap-y-3 text-cp-sm">
            <template v-for="column in details" :key="column.key">
              <dt class="text-cp-text-secondary">
                {{ column.label }}
              </dt>
              <dd :data-column-key="column.key" class="m-0 min-w-0 break-all text-right text-cp-text [&>div]:justify-end">
                <slot :name="column.key" :row="row">
                  {{ cellDisplayValue(column, row) }}
                </slot>
              </dd>
            </template>
          </dl>
          <slot name="expanded" :row="row" />
        </section>
      </article>
      <BaseEmpty v-if="!loading && !displayRows.length" :title="emptyText" surface="none" class="py-8" />
    </div>
  </section>
</template>
