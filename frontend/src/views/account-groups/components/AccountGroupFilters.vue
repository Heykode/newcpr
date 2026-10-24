<script setup lang="ts">
import { Plus, Search, Trash2 } from '@lucide/vue'
import BaseButton from '@/components/base/BaseButton.vue'

import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import { t } from '@/i18n'
import { accountGroupStatusOptions } from '../constants'

defineProps<{
  batchDeleting: boolean
  selectedCount: number
}>()
const emit = defineEmits<{
  create: []
  deleteSelected: []
}>()
const search = defineModel<string>('search', { required: true })
const status = defineModel<string>('status', { required: true })
</script>

<template>
  <div class="flex w-full flex-col gap-3 sm:flex-row sm:items-center">
    <BaseInput
      v-model="search"
      class="sm:w-80"
      :aria-label="t('groups.filters.searchAccountGroup')"
      :placeholder="t('groups.filters.searchGroupName')"
    >
      <template #prefix>
        <Search class="size-4.5 text-cp-text-tertiary" />
      </template>
    </BaseInput>
    <BaseSelect
      v-model="status"
      :options="accountGroupStatusOptions"
      :aria-label="t('groups.filters.filterByGroupStatus')"
      class="w-40"
    />
    <div class="flex shrink-0 items-center justify-end gap-2 sm:ml-auto">
      <BaseButton
        v-if="selectedCount > 0"
        variant="destructive"
        :disabled="batchDeleting"
        @click="emit('deleteSelected')"
      >
        <template #icon>
          <Trash2 class="size-4" />
        </template>
        删除选中 ({{ selectedCount }})
      </BaseButton>
      <BaseButton variant="primary" @click="emit('create')">
        <template #icon>
          <Plus class="size-4" />
        </template>
        {{ t('groups.filters.createGroup') }}
      </BaseButton>
    </div>
  </div>
</template>
