<script setup lang="ts">
import type { SelectOption } from '@/components/base/controls/select-types'
import { computed, shallowRef, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import { getBudgetReferenceAccounts } from '@/api'
import ZButton from '@/components/base/BaseButton.vue'
import ZSelect from '@/components/base/BaseSelect.vue'
import { useRequestState } from '@/composables/useRequestState'

const props = defineProps<{ keyId: string, disabled: boolean, active: boolean }>()
const accountId = defineModel<string>()
const { t } = useI18n({ useScope: 'global' })
const items = shallowRef<SelectOption[]>([])
const selected = shallowRef<SelectOption>()
const open = shallowRef(false)
const search = shallowRef('')
const cursor = shallowRef<string>()
const request = useRequestState()
const { error, loading } = request
const pending = shallowRef(false)
const options = computed(() => {
  if (!open.value && selected.value)
    return [selected.value]
  return error.value
    ? []
    : items.value
})

async function load() {
  if (!props.active || loading.value || !open.value)
    return
  const id = request.start()
  try {
    const page = await getBudgetReferenceAccounts(props.keyId, search.value.trim(), cursor.value, { signal: request.signal, silent: true })
    if (request.isCurrent(id)) {
      items.value = items.value.concat(page.items.map(account => ({
        value: account.id,
        label: account.name,
        description: account.email || account.provider,
      })))
      cursor.value = page.nextCursor ?? undefined
    }
  }
  catch (cause) {
    request.fail(id, cause)
  }
  finally {
    request.finish(id)
  }
}

function reset() {
  request.invalidate()
  pending.value = false
  error.value = ''
  cursor.value = undefined
  items.value = []
}

watch(() => props.active, (active) => {
  if (!active) {
    open.value = false
    request.invalidate()
  }
}, { flush: 'sync' })

function visibility(visible: boolean) {
  open.value = visible
  if (visible && pending.value)
    return
  reset()
  search.value = ''
  if (visible)
    void load()
}

function filter(value: string) {
  if (search.value === value)
    return
  search.value = value
  reset()
  accountId.value = undefined
  selected.value = undefined
  pending.value = true
}

function searchAccounts() {
  pending.value = false
  void load()
}

function choose(id: string | undefined) {
  selected.value = items.value.find(account => account.value === id)
  accountId.value = id
}

function loadMore(direction: string) {
  if (direction === 'bottom' && cursor.value && !error.value)
    void load()
}
</script>

<template>
  <ZSelect
    :model-value="accountId" :options="options" filterable remote :remote-method="searchAccounts" :disabled="disabled"
    :loading="pending || (loading && !items.length)" :aria-label="t('keys.alignment.account')" :placeholder="t('keys.alignment.select')"
    :empty-text="t('keys.alignment.noAccounts')" :no-match-text="t('keys.alignment.noAccounts')"
    @update:model-value="choose" @visible-change="visibility" @query-change="filter" @end-reached="loadMore"
  >
    <template v-if="error" #empty>
      <div role="status" class="flex items-center justify-between gap-2 p-2 text-cp-sm text-cp-text-secondary">
        {{ error }}
        <ZButton size="sm" @click="load">
          {{ t('common.actions.refresh') }}
        </ZButton>
      </div>
    </template>
  </ZSelect>
</template>
