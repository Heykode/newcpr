<script setup lang="ts">
import type { ComponentSize } from './controls/types'
import { Eye, EyeOff, X } from '@lucide/vue'
import { computed, nextTick, shallowRef, useAttrs, useTemplateRef, watch } from 'vue'
import { useFormField } from './BaseForm/composables/useFormField'
import { clearButtonClass } from './controls/clear-button'
import { getInputClasses, inputFieldClass } from './controls/input-styles'
import { useLocale } from './controls/locale'

defineOptions({ inheritAttrs: false })

const props = withDefaults(
  defineProps<{
    placeholder?: string
    type?: string
    disabled?: boolean
    clearable?: boolean
    showPassword?: boolean
    size?: ComponentSize
  }>(),
  {
    placeholder: '',
    type: 'text',
    disabled: false,
    size: 'md',
  },
)

const emit = defineEmits<{
  input: [value: string]
  change: [value: string]
  clear: []
  focus: [event: FocusEvent]
  blur: [event: FocusEvent]
}>()

defineSlots<{ prefix?: () => unknown, suffix?: () => unknown }>()

const locale = useLocale()

const model = defineModel<string>({ default: '' })
const attrs = useAttrs()
const { controlId, invalid, describedBy, required, ariaRequired, rootAttrs, controlAttrs, validate } = useFormField(attrs)
watch(model, () => {
  void validate('change')
})
const inputRef = useTemplateRef<HTMLInputElement>('input')
const passwordVisible = shallowRef(false)
const composing = shallowRef(false)
const inputType = computed(() => props.showPassword ? passwordVisible.value ? 'text' : 'password' : props.type)
function focus() {
  inputRef.value?.focus()
}
function clear() {
  model.value = ''
  emit('input', '')
  emit('change', '')
  emit('clear')
  focus()
}
defineExpose({ focus, blur: () => inputRef.value?.blur(), select: () => inputRef.value?.select(), clear })

const showClear = computed(() => Boolean(props.clearable && model.value && !props.disabled && (attrs.readonly === undefined || attrs.readonly === false)))
const containerClasses = computed(() => getInputClasses(props.size, props.disabled, invalid.value, { trailing: Boolean(showClear.value || props.showPassword) }))

const iconClasses = computed(() => [
  'inline-flex shrink-0',
  props.disabled ? 'text-cp-text-disabled' : invalid.value ? 'text-cp-error' : 'text-cp-text-quaternary',
])

function updateModel(event: Event) {
  if (composing.value)
    return
  const value = (event.target as HTMLInputElement).value
  if (value !== model.value) {
    model.value = value
    emit('input', value)
  }
}

function handleChange(event: Event) {
  updateModel(event)
  emit('change', (event.target as HTMLInputElement).value)
}

function finishComposition(event: CompositionEvent) {
  composing.value = false
  // 浏览器可能紧接着发送 input，等模型同步后再补齐没有 input 的情况
  queueMicrotask(() => {
    void nextTick(() => updateModel(event))
  })
}

function syncAutofill(event: AnimationEvent) {
  if (event.animationName === 'cp-input-autofill-sync')
    updateModel(event)
}
</script>

<template>
  <div v-bind="rootAttrs" class="cp-input min-w-0">
    <span class="group/input relative inline-flex w-full items-center gap-2" :class="containerClasses">
      <span v-if="$slots.prefix" :class="iconClasses">
        <slot name="prefix" />
      </span>
      <input
        v-bind="controlAttrs"
        :id="controlId"
        ref="input"
        :value="model"
        class="cp-input__field"
        :class="inputFieldClass"
        :placeholder="placeholder"
        :type="inputType"
        :disabled="disabled"
        :required="required"
        :aria-describedby="describedBy"
        :aria-invalid="invalid || undefined"
        :aria-required="ariaRequired"
        @animationstart="syncAutofill"
        @change="handleChange"
        @input="updateModel"
        @compositionstart="composing = true"
        @compositionend="finishComposition"
        @focus="emit('focus', $event)"
        @blur="emit('blur', $event); validate('blur')"
      >
      <span v-if="$slots.suffix" :class="iconClasses">
        <slot name="suffix" />
      </span>
      <button
        v-if="showClear"
        type="button"
        :aria-label="locale.input.clear"
        :class="clearButtonClass"
        class="pointer-events-none opacity-0 group-focus-within/input:pointer-events-auto group-focus-within/input:opacity-100 group-hover/input:pointer-events-auto group-hover/input:opacity-100"
        @mousedown.prevent
        @click="clear"
      >
        <X :size="14" />
      </button>
      <button
        v-if="showPassword"
        type="button"
        :class="clearButtonClass"
        class="cp-input__password-toggle"
        :aria-label="(passwordVisible ? locale.input.hide : locale.input.show)(typeof attrs['aria-label'] === 'string' ? attrs['aria-label'] : locale.input.content)"
        :aria-pressed="passwordVisible"
        :disabled="disabled"
        @mousedown.prevent
        @click="passwordVisible = !passwordVisible"
      >
        <EyeOff v-if="passwordVisible" :size="14" /><Eye v-else :size="14" />
      </button>
    </span>
  </div>
</template>

<!-- Keep this namespaced animation unscoped: animationstart exposes its compiled name. -->
<style>
.cp-input__field:-webkit-autofill,
.cp-input__field:-webkit-autofill:hover,
.cp-input__field:-webkit-autofill:focus,
.cp-input__field:autofill {
  animation-name: cp-input-autofill-sync;
  animation-duration: 1ms;
  background-clip: text !important;
  -webkit-background-clip: text !important;
  -webkit-text-fill-color: var(--cp-color-text) !important;
  -webkit-box-shadow: none !important;
  box-shadow: none !important;
}

@keyframes cp-input-autofill-sync {
  from {
    opacity: 1;
  }

  to {
    opacity: 1;
  }
}
</style>
