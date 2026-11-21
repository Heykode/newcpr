<script setup lang="ts">
import type { OpenAiRequestCountry } from '@/api/modules/settings'
import { computed } from 'vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import HelpPopover from '@/components/HelpPopover.vue'
import { t } from '@/i18n'

defineProps<{ disabled: boolean }>()
const country = defineModel<OpenAiRequestCountry>({ required: true })
const modes = computed(() => (['follow_location', 'default', 'passthrough', 'custom'] as const).map(value => ({
  value,
  label: t(`settings.location.countryModes.${value}`),
  help: t(`settings.location.countryHints.${value}`),
})))

function setMode(mode: OpenAiRequestCountry['mode'] | undefined) {
  if (mode && mode !== country.value.mode)
    country.value = mode === 'custom' ? { mode, country: '' } : { mode }
}
</script>

<template>
  <BaseCard :description="t('settings.location.requestCountryDescription')">
    <template #title>
      <span class="inline-flex items-center gap-1.5">
        {{ t('settings.location.requestCountry') }}
        <HelpPopover :label="t('settings.location.countryHelp')">
          <p class="mb-2 font-semibold text-cp-text">
            {{ t('settings.location.requestCountryScope') }}
          </p>
          <dl class="grid grid-cols-[auto_1fr] gap-x-3 gap-y-2">
            <template v-for="mode in modes" :key="mode.value">
              <dt class="font-semibold text-cp-text">{{ mode.label }}</dt>
              <dd>{{ mode.help }}</dd>
            </template>
          </dl>
        </HelpPopover>
      </span>
    </template>
    <div class="grid gap-4 sm:grid-cols-2">
      <BaseFormItem :label="t('settings.location.countryMode')">
        <BaseSelect :model-value="country.mode" :aria-label="t('settings.location.countryMode')" :disabled="disabled" :options="modes" @update:model-value="setMode" />
      </BaseFormItem>
      <BaseFormItem v-if="country.mode === 'custom'" :label="t('settings.location.customCountryCode')" prop="openaiRequestCountry.country" :rules="{ required: true, pattern: /^[A-Z]{2}$/, message: t('settings.location.invalidCountryCode'), trigger: 'blur' }">
        <BaseInput v-model="country.country" :aria-label="t('settings.location.customCountryCode')" placeholder="US" :disabled="disabled" />
      </BaseFormItem>
    </div>
  </BaseCard>
</template>
