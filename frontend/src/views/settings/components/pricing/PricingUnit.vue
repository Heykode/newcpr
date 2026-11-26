<script setup lang="ts">
import { Info } from '@lucide/vue'
import { shallowRef, useId } from 'vue'

import { useI18n } from 'vue-i18n'
import BaseIconButton from '@/components/base/BaseIconButton.vue'
import BasePopover from '@/components/base/BasePopover.vue'

const { t } = useI18n({ useScope: 'global' })

const infoOpen = shallowRef(false)
const infoId = useId()
</script>

<template>
  <BasePopover v-model="infoOpen" placement="bottom-start" trigger="hover-click">
    <template #trigger>
      <BaseIconButton
        variant="link"
        size="sm"
        :label="t('settings.pricing.unitHelp')"
        :aria-expanded="infoOpen"
        :aria-controls="infoOpen ? infoId : undefined"
      >
        <Info class="size-3.5" aria-hidden="true" />
      </BaseIconButton>
    </template>
    <div :id="infoId" class="grid w-72 gap-2 p-3 text-cp-xs leading-relaxed text-cp-text-secondary">
      <p class="m-0 font-emphasis text-cp-text">
        {{ t('settings.pricing.unitPriceUnitUsdMTokens') }}
      </p>
      <p class="m-0">
        {{ t('settings.pricing.priorityHelp') }}
      </p>
      <p class="m-0">
        {{ t('settings.pricing.tableHelp') }}
      </p>
    </div>
  </BasePopover>
</template>
