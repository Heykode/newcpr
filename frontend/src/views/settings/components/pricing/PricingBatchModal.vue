<script setup lang="ts">
import type { PricingRow } from './model'

import type { PricingChange } from '@/api'
import { computed, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BaseScrollbar from '@/components/base/BaseScrollbar.vue'
import { multiplierText, parseMultiplier } from './model'

defineProps<{ rows: PricingRow[], reset: boolean, saving: boolean }>()

defineEmits<{ confirm: [change: PricingChange] }>()

const { t } = useI18n({ useScope: 'global' })

const open = defineModel<boolean>({ required: true })
const multiplier = ref('1')
const bps = computed(() => parseMultiplier(multiplier.value))
watch(open, (value) => {
  if (value)
    multiplier.value = '1'
})
</script>

<template>
  <BaseModal v-model="open" :title="reset ? t('settings.pricing.clearOverrides') : t('settings.pricing.setMultipliers')" :description="t('settings.pricing.batch.description', { count: rows.length })" :dismissible="!saving" type="warning">
    <div class="grid gap-4">
      <p v-if="reset" class="m-0 text-cp leading-relaxed text-cp-text-secondary">
        {{ t('settings.pricing.batch.resetHelp') }}
      </p>
      <BaseFormItem v-else :label="t('settings.pricing.targetMultiplier')" :description="t('settings.pricing.batch.multiplierHelp')">
        <BaseInput v-model="multiplier" :aria-label="t('settings.pricing.targetMultiplier')" inputmode="decimal" :disabled="saving">
          <template #suffix>
            ×
          </template>
        </BaseInput>
      </BaseFormItem>
      <BaseScrollbar max-height="16rem" class="rounded-cp bg-cp-fill-quaternary">
        <ul class="m-0 list-none p-3 text-cp-sm">
          <li v-for="row in rows" :key="row.model" class="flex items-start justify-between gap-3 py-2">
            <span class="min-w-0 break-all font-mono">{{ row.model }}</span>
            <span class="shrink-0 font-mono text-cp-text-secondary">{{ multiplierText(row.effective.multiplierBps) }} → {{ reset ? '1×' : bps === undefined ? '—' : multiplierText(bps) }}</span>
          </li>
        </ul>
      </BaseScrollbar>
      <p v-if="!reset && bps === undefined" role="alert" class="m-0 text-cp-sm text-cp-error">
        {{ t('settings.pricing.enterLegalMultiplier') }}
      </p>
    </div>
    <template #footer>
      <BaseButton :disabled="saving" @click="open = false">
        {{ t('common.actions.cancel') }}
      </BaseButton>
      <BaseButton variant="primary" :loading="saving" :disabled="!rows.length || (!reset && bps === undefined)" @click="$emit('confirm', reset ? { action: 'reset' } : { action: 'multiplier', multiplierBps: bps! })">
        {{ reset ? t('settings.pricing.confirmClearOverride') : t('settings.pricing.confirmSettingMultiplier') }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
