<script setup lang="ts">
import { GoogleAntigravity, Key, LinkAlt, Openai, Xai } from '@boxicons/vue'
import { computed } from 'vue'
import { formatProviderLabel } from '@/utils/providers'

const props = withDefaults(
  defineProps<{
    provider?: string | null
    authenticationKind?: string | null
    size?: 'xs' | 'sm' | 'md'
  }>(),
  {
    size: 'md',
  },
)

const normalizedProvider = computed(() => (props.provider ?? '').trim().toLowerCase())
const normalizedAuthenticationKind = computed(() => (props.authenticationKind ?? '').trim().toLowerCase())
const showAuthenticationKind = computed(() => props.authenticationKind !== undefined)
const sizeClasses = {
  xs: { gap: 'gap-0.75', container: 'size-4.5 rounded-md', icon: 'size-2.5' },
  sm: { gap: 'gap-1', container: 'size-5 rounded-md', icon: 'size-3' },
  md: { gap: 'gap-2', container: 'size-7 rounded-lg', icon: 'size-4' },
}
const classes = computed(() => sizeClasses[props.size])

const providerLabel = computed(() => formatProviderLabel(props.provider, '未知平台'))

const authenticationLabel = computed(() => {
  if (normalizedAuthenticationKind.value === 'oauth')
    return 'OAuth'
  if (normalizedAuthenticationKind.value === 'api_key')
    return 'API Key'
  return props.authenticationKind?.trim() || '未知认证类型'
})
</script>

<template>
  <span class="inline-flex items-center whitespace-nowrap" :class="classes.gap">
    <span
      class="inline-flex shrink-0 items-center justify-center bg-cp-fill-secondary/80 text-cp-text"
      :class="classes.container"
      :title="providerLabel"
    >
      <Openai v-if="normalizedProvider === 'openai'" :class="classes.icon" />
      <Xai v-else-if="normalizedProvider === 'xai'" :class="classes.icon" />
      <GoogleAntigravity v-else-if="normalizedProvider === 'antigravity'" :class="classes.icon" />
      <span v-else class="text-[10px] font-heavy text-cp-text-quaternary">?</span>
    </span>

    <span
      v-if="showAuthenticationKind"
      class="inline-flex shrink-0 items-center justify-center bg-cp-fill-secondary/80 text-cp-text"
      :class="classes.container"
      :title="authenticationLabel"
    >
      <LinkAlt v-if="normalizedAuthenticationKind === 'oauth'" :class="classes.icon" />
      <Key v-else-if="normalizedAuthenticationKind === 'api_key'" :class="classes.icon" />
      <span v-else class="text-[10px] font-heavy text-cp-text-quaternary">?</span>
    </span>

    <span class="sr-only">
      {{ providerLabel }}<template v-if="showAuthenticationKind">，{{ authenticationLabel }}</template>
    </span>
  </span>
</template>
