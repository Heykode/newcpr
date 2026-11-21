import type { Ref } from 'vue'
import type { UsageDisplayRecord } from '../utils/records'
import type { ConfigurableTableColumn } from '@/components/base/BaseTable/useTableColumns'
import { computed } from 'vue'
import { useTableColumns } from '@/components/base/BaseTable/useTableColumns'
import { usageRecordColumns } from '../constants'

export function useUsageRecordColumns(status: Ref<'success' | 'error' | 'all'>, tableWidth: Readonly<Ref<number>>) {
  const columns = computed(() => usageRecordColumns.map(column => column.key === 'createdAtDisplay'
    ? { ...column, width: 128 }
    : column.key === 'accountEmail'
      ? { ...column, width: 280, hideable: true }
      : column.key === 'actions' ? { ...column, fixed: 'right' as const } : column))
  const usage = useTableColumns(() => status.value === 'all'
    ? columns.value.flatMap(column => column.key === 'route'
        ? [column, { key: 'errorMessage', label: '最终错误', kind: 'custom' as const, size: '4xl' as const, hideable: false }]
        : [column])
    : columns.value, 'usage-records')
  const errors = useTableColumns(() => {
    const time = columns.value.find(column => column.key === 'createdAtDisplay')!
    const actionWidth = 72
    return [
      {
        key: 'errorMessage',
        label: '错误',
        kind: 'custom',
        fixed: 'left',
        minWidth: tableWidth.value ? Math.min(370, Math.max(96, (tableWidth.value - actionWidth) / 2)) : 370,
        grow: 1,
        hideable: false,
      },
      time,
      ...columns.value.filter(column => column.key !== time.key).map(column => column.key === 'actions'
        ? { ...column, width: actionWidth }
        : column),
    ] satisfies ConfigurableTableColumn<UsageDisplayRecord>[]
  }, 'usage-error-records')
  return computed(() => status.value === 'error' ? errors : usage)
}
