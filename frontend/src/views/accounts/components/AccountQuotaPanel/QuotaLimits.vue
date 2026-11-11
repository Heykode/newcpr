<script setup lang="ts">
import type { Account } from '@/api'
import { SlidersHorizontal } from '@lucide/vue'
import { computed, ref, shallowRef } from 'vue'
import { useI18n } from 'vue-i18n'
import { batchUpdateAccounts, getAccountQuota } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import { useAsyncAction } from '@/composables/useAsyncAction'

const props = defineProps<{ account: Account }>()
const emit = defineEmits<{ accountUpdated: [account: Account] }>()
const { t } = useI18n({ useScope: 'global' })
const open = shallowRef(false)
const draft = ref<Record<string, string>>({})
const { loading, run } = useAsyncAction()
const rows = computed(() => {
  const labels = new Map(props.account.quota.windows.filter(w => w.accountWide).map(w => [
    w.key,
    w.labelDisplay,
  ]))
  for (const key of Object.keys(props.account.quotaLimits)) {
    if (!labels.has(key))
      labels.set(key, key)
  }
  return Array.from(labels, ([key, label]) => ({ key, label }))
})
const invalid = computed(() => Object.values(draft.value).some(v => v !== '' && !/^(?:[1-9]\d?|100)$/.test(v)))

function edit() {
  draft.value = Object.fromEntries(Object.entries(props.account.quotaLimits).map(([key, value]) => [
    key,
    String(value),
  ]))
  open.value = true
}

async function save() {
  if (invalid.value)
    return
  const accountId = props.account.id
  const limits = Object.fromEntries(Object.entries(draft.value).filter(([, value]) => value !== '').map(([key, value]) => [
    key,
    Number(value),
  ]))
  await run(async () => {
    await batchUpdateAccounts({
      accountIds: [accountId],
      quotaLimits: limits,
    })
    const { account } = await getAccountQuota({ accountId })
    if (props.account.id === accountId) {
      emit('accountUpdated', account)
      open.value = false
    }
  })
}
</script>

<template>
  <BaseIconButton :label="t('accounts.quotaLimits.title')" aria-haspopup="dialog" :pressed="open" @click="edit">
    <SlidersHorizontal class="size-3.5" />
  </BaseIconButton>
  <BaseModal v-model="open" :title="t('accounts.quotaLimits.title')" :description="t('accounts.quotaLimits.help')" size="sm" :dismissible="!loading">
    <div class="grid gap-3">
      <p v-if="!rows.length" class="m-0 text-cp-sm text-cp-text-secondary">
        {{ t('accounts.quotaLimits.empty') }}
      </p>
      <BaseFormItem v-for="row in rows" :key="row.key" :label="row.label">
        <BaseInput v-model="draft[row.key]" type="number" min="1" max="100" step="1" :aria-label="row.label" :placeholder="t('accounts.quotaLimits.unlimited')" :disabled="loading">
          <template #suffix>
            %
          </template>
        </BaseInput>
      </BaseFormItem>
      <p v-if="invalid" class="m-0 text-cp-xs text-cp-error" role="alert">
        {{ t('accounts.quotaLimits.invalid') }}
      </p>
    </div>
    <template #footer>
      <BaseButton :disabled="loading" @click="open = false">
        {{ t('common.actions.cancel') }}
      </BaseButton>
      <BaseButton variant="primary" :disabled="!rows.length || invalid" :loading="loading" @click="save">
        {{ t('common.actions.save') }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
