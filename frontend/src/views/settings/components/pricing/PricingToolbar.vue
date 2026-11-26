<script setup lang="ts">
import { CircleAlert, Download, Percent, Plus, RotateCcw, Search, X } from '@lucide/vue'
import { computed, shallowRef, useId } from 'vue'
import { useI18n } from 'vue-i18n'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'

import BaseInput from '@/components/base/BaseInput.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import ProviderSelect from '@/components/ProviderSelect.vue'
import { sourceLabels } from './model'

const props = defineProps<{
  selectedCount: number
  disabled: boolean
  saving: boolean
  syncing: boolean
  syncedAtDisplay?: string | null
  providers: string[]
}>()

defineEmits<{ add: [], sync: [], setMultiplier: [], reset: [], clear: [] }>()

const { t } = useI18n({ useScope: 'global' })

const search = defineModel<string>('search', { required: true })
const provider = defineModel<string>('provider', { required: true })
const source = defineModel<string>('source', { required: true })
const syncInfoOpen = shallowRef(false)
const syncInfoId = useId()
const syncTime = computed(() => props.syncedAtDisplay ?? t('settings.pricing.notSyncedYet'))
</script>

<template>
  <div class="mb-3 shrink-0">
    <div class="flex flex-wrap items-center gap-2">
      <BaseInput v-model="search" class="w-full sm:w-72" :placeholder="t('settings.pricing.searchModel')" :aria-label="t('settings.pricing.searchModelId')">
        <template #prefix>
          <Search class="size-4" />
        </template>
      </BaseInput>
      <div v-if="providers.length" class="min-w-0 max-w-full overflow-x-auto">
        <ProviderSelect
          v-model="provider"
          :aria-label="t('settings.pricing.modelProvider')"
          :providers="providers"
          :disabled="saving"
        />
      </div>
      <BaseSelect v-model="source" class="w-32" :aria-label="t('settings.pricing.priceSource')" :options="[{ label: t('settings.pricing.allSources'), value: 'all' }, { label: sourceLabels.custom, value: 'custom' }, { label: sourceLabels.synced, value: 'synced' }, { label: sourceLabels.builtin, value: 'builtin' }]" />
      <div v-if="selectedCount" class="ml-auto flex w-full flex-wrap items-center justify-end gap-2 sm:w-auto" role="group" :aria-label="t('settings.pricing.batchOperation')">
        <span class="inline-flex h-cp-control w-full items-center gap-1 whitespace-nowrap text-cp text-cp-text-secondary sm:mr-1 sm:w-auto" aria-live="polite">
          {{ t('settings.pricing.selectedCountLabel') }} <span class="font-mono tabular-nums">{{ selectedCount }}</span> {{ t('settings.pricing.item') }}
        </span>
        <BaseButton :disabled="disabled || selectedCount > 500" @click="$emit('setMultiplier')">
          <template #icon>
            <Percent class="size-4" />
          </template>
          {{ t('settings.pricing.setMultiplier') }}
        </BaseButton>
        <BaseButton :disabled="disabled || selectedCount > 500" @click="$emit('reset')">
          <template #icon>
            <RotateCcw class="size-4" />
          </template>
          {{ t('settings.pricing.clearOverwrite') }}
        </BaseButton>
        <BaseIconButton :label="t('settings.pricing.deselect')" variant="ghost" :disabled="disabled" @click="$emit('clear')">
          <X class="size-4" />
        </BaseIconButton>
      </div>
      <div v-else class="ml-auto flex items-center gap-2">
        <div class="flex items-center gap-1">
          <BasePopover v-model="syncInfoOpen" placement="bottom-end" trigger="hover-click">
            <template #trigger>
              <BaseIconButton variant="link" :label="t('settings.pricing.synchronizationInstructions')" size="sm" :aria-expanded="syncInfoOpen" :aria-controls="syncInfoOpen ? syncInfoId : undefined">
                <CircleAlert class="size-3.5" />
              </BaseIconButton>
            </template>
            <div :id="syncInfoId" class="grid w-64 gap-2 p-3 text-cp-xs leading-relaxed text-cp-text-secondary">
              <p class="m-0">
                {{ t('settings.pricing.sourceHelp') }}
              </p>
              <p class="m-0 text-cp-text-tertiary">
                {{ t('settings.pricing.lastSynced', { syncTime }) }}
              </p>
            </div>
          </BasePopover>
          <BaseButton :loading="syncing" :disabled="disabled" :aria-label="t('settings.pricing.syncPriceList')" @click="$emit('sync')">
            <template #icon>
              <Download class="size-4" />
            </template>
            {{ t('settings.pricing.sync.action') }}
          </BaseButton>
        </div>
        <BaseButton variant="primary" :disabled="disabled" @click="$emit('add')">
          <template #icon>
            <Plus class="size-4" />
          </template>
          {{ t('settings.pricing.addModel') }}
        </BaseButton>
      </div>
    </div>
  </div>
</template>
