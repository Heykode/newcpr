import type { MaybeRefOrGetter } from 'vue'
import type { BaseTableColumn, TableRow } from './columns'
import { useStorage } from '@vueuse/core'
import { computed, toValue } from 'vue'

export interface ConfigurableTableColumn<Row extends TableRow> extends BaseTableColumn<Row> {
  hideable?: boolean
  defaultHidden?: boolean
}

export interface TableColumnOption {
  key: string
  label: string
  visible: boolean
  disabled: boolean
  reorderable?: boolean
}

function readVisibility(value: string): Record<string, boolean> {
  try {
    const parsed: unknown = JSON.parse(value)
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed))
      return Object.fromEntries(Object.entries(parsed).filter(([, visible]) => typeof visible === 'boolean'))
  }
  catch {
    // Invalid local preferences must not prevent the table from rendering.
  }
  return {}
}

export function useTableColumns<Row extends TableRow>(
  source: MaybeRefOrGetter<ConfigurableTableColumn<Row>[]>,
  tableId: string,
) {
  // Store only overrides so newly introduced columns retain their own defaults.
  const overrides = useStorage<Record<string, boolean>>(
    `codex-proxy:table-columns:${tableId}`,
    {},
    undefined,
    {
      shallow: true,
      writeDefaults: false,
      serializer: { read: readVisibility, write: JSON.stringify },
    },
  )

  const order = useStorage<string[]>(`codex-proxy:table-column-order:${tableId}`, [], undefined, {
    shallow: true,
    writeDefaults: false,
    serializer: {
      read: (value) => {
        try {
          const parsed: unknown = JSON.parse(value)
          return Array.isArray(parsed) ? [...new Set(parsed.filter((item): item is string => typeof item === 'string'))] : []
        }
        catch { return [] }
      },
      write: JSON.stringify,
    },
  })
  const orderedColumns = computed(() => {
    const sourceColumns = toValue(source)
    const byKey = new Map(sourceColumns.map(column => [column.key, column]))
    const ordered = order.value.flatMap(key => byKey.has(key) ? [byKey.get(key)!] : [])
    ordered.push(...sourceColumns.filter(column => !order.value.includes(column.key)))
    return [...ordered.filter(column => column.fixed === 'left'), ...ordered.filter(column => !column.fixed), ...ordered.filter(column => column.fixed === 'right')]
  })
  const columnStates = computed(() => {
    const states = orderedColumns.value.map((column) => {
      const override = overrides.value[column.key]
      return {
        column,
        visible: column.hideable === false || (typeof override === 'boolean' ? override : !column.defaultHidden),
      }
    })
    if (!states.some(state => state.visible) && states[0])
      states[0].visible = true
    return states
  })
  const visibleColumns = computed(() => columnStates.value.filter(state => state.visible).map(state => state.column))
  const columnOptions = computed<TableColumnOption[]>(() => columnStates.value
    .filter(({ column }) => column.label)
    .map(({ column, visible }) => ({
      key: column.key,
      label: column.label!,
      visible,
      reorderable: !column.fixed && column.kind !== 'actions',
      disabled: column.hideable === false || (visible && visibleColumns.value.length === 1),
    })))

  function setColumnVisible(key: string, visible: boolean) {
    const option = columnOptions.value.find(option => option.key === key)
    const column = toValue(source).find(column => column.key === key)
    if (!column || !option || option.disabled || option.visible === visible)
      return

    const next = { ...overrides.value }
    if (visible === !column.defaultHidden)
      delete next[key]
    else next[key] = visible
    overrides.value = next
  }

  function setColumnOrder(keys: string[]) {
    const known = new Set(toValue(source).map(column => column.key))
    order.value = [...new Set(keys.filter(key => known.has(key)))]
  }

  function resetColumns() {
    order.value = []
    overrides.value = {}
  }

  return { visibleColumns, columnOptions, setColumnVisible, setColumnOrder, resetColumns }
}
