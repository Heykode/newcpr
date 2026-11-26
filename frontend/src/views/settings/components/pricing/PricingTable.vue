<script setup lang="ts">
import type { PricingRow } from './model'

import type { BaseTableColumn as TableColumn } from '@/components/base/BaseTable/columns'

import { Pencil, Trash2 } from '@lucide/vue'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseTable from '@/components/base/BaseTable/index.vue'
import { effectivePrice, multiplierText, sourceLabels } from './model'
import PricingUnit from './PricingUnit.vue'

defineProps<{ rows: PricingRow[], selected: string[], loading: boolean, disabled: boolean }>()

defineEmits<{ toggle: [model: string, checked: boolean], togglePage: [checked: boolean], edit: [row: PricingRow], delete: [row: PricingRow] }>()

const { t } = useI18n({ useScope: 'global' })

const columns = computed<TableColumn<PricingRow>[]>(() => ([
  { key: 'select', kind: 'selection' },
  {
    key: 'model',
    label: t('settings.pricing.model'),
    kind: 'identity',
  },
  {
    key: 'input',
    label: t('settings.pricing.input'),
    kind: 'numeric',
  },
  {
    key: 'output',
    label: t('settings.pricing.output'),
    kind: 'numeric',
  },
  {
    key: 'cacheRead',
    label: t('settings.pricing.cacheRead'),
    kind: 'numeric',
  },
  {
    key: 'multiplier',
    label: t('settings.pricing.multiplier'),
    kind: 'numeric',
    align: 'center',
  },
  {
    key: 'actions',
    label: t('common.fields.actions'),
    kind: 'actions',
  },
]))

function summaryBand(row: PricingRow) {
  return row.effective.bands.image ? 'image' : 'standard'
}
</script>

<template>
  <BaseTable class="[--cp-table-row-height:50px]" :columns="columns" :rows="rows" row-key="model" :loading="loading" :selected-row-keys="selected" :empty-text="t('settings.pricing.noMatchingModel')" show-header-when-empty>
    <template #header-select>
      <BaseCheckbox show-label :aria-label="t('settings.pricing.selectModelOnThisPage')" :disabled="disabled || !rows.length" :model-value="!!rows.length && rows.every(row => selected.includes(row.model))" :indeterminate="rows.some(row => selected.includes(row.model)) && !rows.every(row => selected.includes(row.model))" @update:model-value="$emit('togglePage', $event)" />
    </template>
    <template #header-model>
      <div class="flex items-center gap-3">
        <span>{{ t('settings.pricing.model') }}</span>
        <PricingUnit />
      </div>
    </template>
    <template #select="{ row }">
      <BaseCheckbox show-label :aria-label="t('settings.pricing.select', { model: row.model })" :disabled="disabled" :model-value="selected.includes(row.model)" @update:model-value="$emit('toggle', row.model, $event)" />
    </template>
    <template #model="{ row }">
      <div class="min-w-0 py-1.5">
        <div class="flex min-w-0 flex-wrap items-baseline gap-x-3 gap-y-1">
          <span class="truncate font-mono font-semibold" :title="row.model">{{ row.model }}</span>
          <span class="shrink-0 text-cp-xs" :class="row.source === 'custom' ? 'text-cp-primary-text' : 'text-cp-text-tertiary'">
            {{ sourceLabels[row.source] }}<span v-if="row.effective.bands.image"> {{ t('settings.pricing.image') }}</span>
          </span>
        </div>
        <div class="mt-1 flex flex-wrap gap-x-3 gap-y-1 text-cp-xs text-cp-text-secondary sm:hidden">
          <span>{{ t('settings.pricing.input') }} <span class="font-mono">{{ effectivePrice(row.effective.bands[summaryBand(row)]?.input, row.effective.multiplierBps) }}</span></span>
          <span>{{ t('settings.pricing.output') }} <span class="font-mono">{{ effectivePrice(row.effective.bands[summaryBand(row)]?.output, row.effective.multiplierBps) }}</span></span>
        </div>
      </div>
    </template>
    <template v-for="field in (['input', 'output', 'cacheRead'] as const)" :key="field" #[field]="{ row }">
      <div class="py-1 font-mono tabular-nums">
        <div v-if="effectivePrice(row.base.bands[summaryBand(row)]?.[field]) !== effectivePrice(row.effective.bands[summaryBand(row)]?.[field], row.effective.multiplierBps)" class="text-cp-xs text-cp-text-tertiary line-through">
          {{ effectivePrice(row.base.bands[summaryBand(row)]?.[field]) }}
        </div>
        <div :class="row.source === 'custom' ? 'text-cp-primary-text' : 'text-cp-text'">
          {{ effectivePrice(row.effective.bands[summaryBand(row)]?.[field], row.effective.multiplierBps) }}
        </div>
      </div>
    </template>
    <template #multiplier="{ row }">
      <span class="font-mono tabular-nums">{{ multiplierText(row.effective.multiplierBps) }}</span>
    </template>
    <template #actions="{ row }">
      <div class="flex items-center gap-1">
        <BaseIconButton size="sm" :label="t('settings.pricing.editPrice')" :disabled="disabled" @click="$emit('edit', row)">
          <Pencil class="size-3.5 text-cp-link" aria-hidden="true" />
        </BaseIconButton>
        <BaseIconButton v-if="row.canDelete" size="sm" :label="t('settings.pricing.deletePriceList')" :disabled="disabled" @click="$emit('delete', row)">
          <Trash2 class="size-3.5 text-cp-error" aria-hidden="true" />
        </BaseIconButton>
      </div>
    </template>
  </BaseTable>
</template>
