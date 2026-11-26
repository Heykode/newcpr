<script setup lang="ts">
import type { ProviderRequestProfiles } from '@/api'
import type { BuiltinProvider } from '@/providers/catalog'

import { useI18n } from 'vue-i18n'
import BaseCard from '@/components/base/BaseCard.vue'
import ProviderRequestProfilesEditor from '@/components/request-profile/index.vue'

withDefaults(defineProps<{ provider: BuiltinProvider, active?: boolean, disabled?: boolean }>(), {
  active: true,
  disabled: false,
})

const { t } = useI18n({ useScope: 'global' })

const model = defineModel<ProviderRequestProfiles>({ required: true })
</script>

<template>
  <BaseCard :title="t('settings.clientProfile.clientIdentity')" :description="t('settings.clientProfile.identityDescription')">
    <ProviderRequestProfilesEditor
      v-model="model"
      :provider="provider"
      :active="active"
      :disabled="disabled"
    />
  </BaseCard>
</template>
