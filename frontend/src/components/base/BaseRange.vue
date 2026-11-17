<script setup lang="ts">
import type { CSSProperties } from 'vue'
import type { ComponentSize } from './controls/types'
import { computed, useAttrs, useTemplateRef } from 'vue'
import { useFormField } from './BaseForm/composables/useFormField'
import { useComponentSize } from './controls/size'

const props = withDefaults(
  defineProps<{
    label: string
    size?: ComponentSize
    validateEvent?: boolean
    min?: number
    max?: number
    step?: number
    unit?: string
    disabled?: boolean
  }>(),
  {
    validateEvent: true,
    min: 0,
    max: 100,
    step: 1,
    unit: undefined,
    disabled: false,
  },
)
const emit = defineEmits<{ input: [value: number], change: [value: number], focus: [event: FocusEvent], blur: [event: FocusEvent] }>()

const componentSize = useComponentSize(() => props.size)

const model = defineModel<number>({ required: true })
const attrs = useAttrs()
const inputRef = useTemplateRef<HTMLInputElement>('input')
const { controlId, describedBy, invalid, ariaRequired, validate } = useFormField(attrs, () => props.validateEvent)
defineExpose({ focus: () => inputRef.value?.focus(), blur: () => inputRef.value?.blur() })

const rangeStyle = computed<CSSProperties>(() => {
  const span = props.max - props.min
  const progress = span > 0 ? ((model.value - props.min) / span) * 100 : 0
  return {
    '--base-range-progress': `${Math.min(100, Math.max(0, progress))}%`,
  }
})

const valueText = computed(() => `${model.value}${props.unit ?? ''}`)

function updateModel(event: Event) {
  const value = (event.target as HTMLInputElement).valueAsNumber
  if (Number.isFinite(value)) {
    model.value = value
    emit('input', value)
  }
}
</script>

<template>
  <input
    :id="controlId"
    ref="input"
    :value="model"
    type="range"
    :min="min"
    :max="max"
    :step="step"
    :disabled="disabled"
    :aria-label="typeof attrs['aria-label'] === 'string' ? attrs['aria-label'] : label"
    :aria-describedby="describedBy"
    :aria-invalid="invalid || undefined"
    :aria-required="ariaRequired"
    :aria-valuetext="valueText"
    class="base-range w-full"
    :class="{ sm: 'h-4 [--thumb:12px]', md: 'h-5 [--thumb:14px]', lg: 'h-6 [--thumb:18px]' }[componentSize]"
    :style="rangeStyle"
    @input="updateModel"
    @change="emit('change', ($event.target as HTMLInputElement).valueAsNumber); validate('change')"
    @focus="emit('focus', $event)"
    @blur="emit('blur', $event); validate('blur')"
  >
</template>

<style scoped>
.base-range {
  appearance: none;
  cursor: pointer;
  background: transparent;
  outline: none;
}

.base-range[aria-invalid='true']:enabled {
  --slider-accent: var(--cp-color-error);
}

.base-range::-webkit-slider-runnable-track {
  height: 4px;
  border-radius: 999px;
  background: linear-gradient(
    to right,
    var(--slider-accent, var(--cp-color-primary)) 0 var(--base-range-progress),
    var(--cp-color-fill-secondary) var(--base-range-progress) 100%
  );
}

.base-range::-webkit-slider-thumb {
  width: var(--thumb);
  height: var(--thumb);
  margin-top: calc((4px - var(--thumb)) / 2);
  appearance: none;
  border: 2px solid var(--cp-color-bg-container);
  border-radius: 999px;
  background: var(--slider-accent, var(--cp-color-primary));
  transition:
    box-shadow 150ms ease,
    transform 150ms ease;
}

.base-range::-moz-range-track {
  height: 4px;
  border-radius: 999px;
  background: var(--cp-color-fill-secondary);
}

.base-range::-moz-range-progress {
  height: 4px;
  border-radius: 999px;
  background: var(--slider-accent, var(--cp-color-primary));
}

.base-range::-moz-range-thumb {
  width: calc(var(--thumb) - 4px);
  height: calc(var(--thumb) - 4px);
  border: 2px solid var(--cp-color-bg-container);
  border-radius: 999px;
  background: var(--slider-accent, var(--cp-color-primary));
  transition:
    box-shadow 150ms ease,
    transform 150ms ease;
}

.base-range:hover::-webkit-slider-thumb {
  transform: scale(1.08);
}

.base-range:hover::-moz-range-thumb {
  transform: scale(1.08);
}

.base-range:focus-visible::-webkit-slider-thumb {
  box-shadow: 0 0 0 3px var(--cp-color-primary-container-hover);
}

.base-range:focus-visible::-moz-range-thumb {
  box-shadow: 0 0 0 3px var(--cp-color-primary-container-hover);
}

.base-range:disabled {
  cursor: not-allowed;
  opacity: 0.5;
}

@media (prefers-reduced-motion: reduce) {
  .base-range::-webkit-slider-thumb {
    transition: none;
  }

  .base-range::-moz-range-thumb {
    transition: none;
  }
}
</style>
