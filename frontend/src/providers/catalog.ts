import type { Component } from 'vue'
import { GoogleAntigravity, Openai, Xai } from '@boxicons/vue'
import { PROVIDER_DISPLAY_NAMES } from '@/utils/providers'

export { formatProviderLabel } from '@/utils/providers'
export { isSupportedProvider as isBuiltinProvider } from '@/utils/providers'
export type BuiltinProvider = keyof typeof PROVIDER_DISPLAY_NAMES
export const PROVIDER_IDS = Object.keys(PROVIDER_DISPLAY_NAMES) as BuiltinProvider[]
const icons: Record<BuiltinProvider, Component> = { openai: Openai, xai: Xai, antigravity: GoogleAntigravity }
export function providerIcon(provider: BuiltinProvider): Component {
  return icons[provider]
}
