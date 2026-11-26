<script setup lang="ts">
import type { PricingRow } from './model'

import type { ModelPricing, PriceBand, TokenPrices } from '@/api'
import { Info } from '@lucide/vue'
import { computed, ref, shallowRef, useId, watch } from 'vue'
import { useI18n } from 'vue-i18n'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BasePopover from '@/components/base/BasePopover.vue'
import BaseSegmented from '@/components/base/BaseSegmented.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import ProviderIconGroup from '@/components/ProviderIconGroup.vue'
import { bands, parseMultiplier, validPrice } from './model'
import PricingRateFields from './PricingRateFields.vue'

const props = defineProps<{ row?: PricingRow, provider: string, supportedBands: PriceBand[], saving: boolean }>()

const emit = defineEmits<{ save: [model: string, pricing: ModelPricing] }>()

const { t } = useI18n({ useScope: 'global' })

const open = defineModel<boolean>({ required: true })
const model = ref('')
const multiplier = ref('1')
const draft = ref<Partial<Record<PriceBand, TokenPrices>>>({})
const selectedBand = ref<PriceBand>('standard')
const validation = ref('')
const helpOpen = shallowRef(false)
const helpId = useId()
const base = computed(() => props.row?.base.bands[selectedBand.value])
const current = computed(() => draft.value[selectedBand.value])
const bps = computed(() => parseMultiplier(multiplier.value))
const sourceOptions = computed(() => [
  {
    label: t('settings.pricing.sourcePrice'),
    value: 'inherit',
    disabled: selectedBand.value === 'standard' && !props.row?.base.bands.standard,
  },
  {
    label: t('settings.rotation.customized'),
    value: 'custom',
  },
])
const priceMode = computed({
  get: () => current.value ? 'custom' : 'inherit',
  set: value => toggleBand(value === 'custom'),
})
const bandOptions = computed(() => bands.value
  .filter(band => props.supportedBands.includes(band.value))
  .map(band => ({
    ...band,
    label: band.value === 'standard' && props.row?.base.bands.image
      ? t('settings.pricing.textToken')
      : band.label,
  })))

watch(open, (value) => {
  if (!value)
    return
  model.value = props.row?.model ?? ''
  multiplier.value = String((props.row?.custom?.multiplierBps ?? 10_000) / 10_000)
  draft.value = JSON.parse(JSON.stringify(props.row?.custom?.bands ?? {}))
  selectedBand.value = props.row?.effective.bands.image ? 'image' : 'standard'
  if (!props.row?.effective.bands[selectedBand.value])
    toggleBand(true)
  validation.value = ''
  helpOpen.value = false
})
watch([model, multiplier, draft], () => {
  validation.value = ''
}, { deep: true })
function toggleBand(enabled: boolean) {
  if (enabled) {
    draft.value[selectedBand.value] = {
      ...(base.value ?? {
        input: '',
        output: '',
        cacheRead: '',
        cacheWrite: '',
      }),
    }
  }
  else {
    delete draft.value[selectedBand.value]
  }
}
function updatePrice(field: keyof TokenPrices, value: string) {
  if (current.value && !props.saving)
    current.value[field] = value
}
function submit() {
  if (!model.value || new TextEncoder().encode(model.value).length > 128 || /\s/.test(model.value) || [...model.value].some(char => char.charCodeAt(0) < 32 || char.charCodeAt(0) === 127)) {
    validation.value = t('settings.pricing.validation.modelId')
    return
  }
  if (bps.value === undefined) {
    validation.value = t('settings.pricing.multiplierRangeIsWithUpToFour')
    return
  }
  for (const [band, rates] of Object.entries(draft.value)) {
    if (Object.values(rates).some(price => !validPrice(price))) {
      validation.value = t('settings.pricing.validation.tierPrices')
      selectedBand.value = band as PriceBand
      return
    }
  }
  if (!props.row?.base.bands.standard && !draft.value.standard) {
    validation.value = t('settings.pricing.validation.standardTier')
    selectedBand.value = 'standard'
    return
  }
  validation.value = ''
  emit('save', model.value, { multiplierBps: bps.value, bands: draft.value })
}
</script>

<template>
  <BaseModal v-model="open" :title="row ? t('settings.pricing.editModelPricing') : t('settings.pricing.addModelPricing')" size="lg" :dismissible="!saving">
    <template #description>
      <span class="inline-flex items-center gap-2 align-middle text-cp-sm font-normal">
        <ProviderIconGroup :provider="provider" size="sm" />
        <span>{{ t('settings.pricing.unitLabel') }}</span>
      </span>
    </template>
    <div class="grid gap-5">
      <div class="grid grid-cols-[minmax(0,1fr)_6.5rem] gap-3 sm:grid-cols-[minmax(0,1fr)_8rem] sm:gap-4">
        <BaseFormItem :label="t('settings.pricing.upstreamModelId')">
          <BaseInput v-model="model" :disabled="saving || !!row" :aria-label="t('settings.pricing.upstreamModelId')" :placeholder="t('settings.pricing.forExampleGpt')" class="font-mono" />
        </BaseFormItem>
        <BaseFormItem :label="t('settings.pricing.multiplier')">
          <BaseInput v-model="multiplier" :aria-label="t('settings.pricing.customizedMultiplier')" class="font-mono" inputmode="decimal" :disabled="saving">
            <template #suffix>
              ×
            </template>
          </BaseInput>
        </BaseFormItem>
      </div>
      <section :aria-label="t('settings.pricing.gearPrice')">
        <div class="mb-5 flex flex-wrap items-center justify-between gap-3">
          <BaseSelect :model-value="selectedBand" :options="bandOptions" :aria-label="t('settings.pricing.priceRange')" class="min-w-0 flex-1 sm:max-w-56" :disabled="saving" @update:model-value="selectedBand = $event as PriceBand" />
          <BaseSegmented v-model="priceMode" :label="t('settings.pricing.unitPriceSource')" :options="sourceOptions" :disabled="saving" />
        </div>
        <PricingRateFields v-if="current || base" class="rounded-cp-lg bg-cp-fill-quaternary p-4" :base="base" :custom="current" :multiplier-bps="bps" :disabled="saving" @change="updatePrice" />
        <div v-else class="grid justify-items-center gap-3 py-6 text-cp-sm text-cp-text-tertiary">
          <p class="m-0">
            {{ t('settings.pricing.tier.sourceUnavailable') }}
          </p>
          <BaseButton size="sm" :disabled="saving" @click="toggleBand(true)">
            {{ t('settings.pricing.fillInUnitPrice') }}
          </BaseButton>
        </div>
      </section>
      <p v-if="validation" role="alert" class="m-0 rounded-cp bg-cp-error-container p-3 text-cp-sm text-cp-error-on-container">
        {{ validation }}
      </p>
    </div>
    <template #footer>
      <BasePopover v-model="helpOpen" class="mr-auto self-center" placement="top-start" trigger="hover-click">
        <template #trigger>
          <BaseButton variant="link" :aria-expanded="helpOpen" :aria-controls="helpOpen ? helpId : undefined">
            <template #icon>
              <Info class="size-3.5" aria-hidden="true" />
            </template>
            {{ t('settings.pricing.pricingInstructions') }}
          </BaseButton>
        </template>
        <div :id="helpId" class="grid w-72 gap-2 p-3 text-cp-xs leading-relaxed text-cp-text-secondary">
          <p class="m-0">
            {{ t('settings.pricing.modelIdUsesNameActuallySentTo') }}
          </p>
          <p class="m-0">
            {{ t('settings.pricing.customTierHelp') }}
          </p>
          <p class="m-0">
            {{ t('settings.pricing.estimateHelp') }}
          </p>
        </div>
      </BasePopover>
      <BaseButton :disabled="saving" @click="open = false">
        {{ t('common.actions.cancel') }}
      </BaseButton><BaseButton variant="primary" :loading="saving" @click="submit">
        {{ t('common.actions.save') }}
      </BaseButton>
    </template>
  </BaseModal>
</template>
