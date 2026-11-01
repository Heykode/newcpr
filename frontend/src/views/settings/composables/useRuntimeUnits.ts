import type { RequestTuning } from '@/api/modules/settings'
import { computed, ref } from 'vue'

interface RuntimeForm {
  responsesMaxDecompressedBodyBytes: number
  requestIntervalMs: number | null
  requestTuning: RequestTuning
}

// Decimal arithmetic avoids rejecting e.g. 1.001 seconds due to float rounding.
function scaledInteger(raw: string, scale: number): number | null {
  const value = raw.trim()
  const match = /^(\d*)(?:\.(\d*))?(?:e([+-]?\d+))?$/i.exec(value)
  if (!match || !(match[1] || match[2]) || value.length > 128)
    return null
  const exponent = Number(match[3] ?? 0) - (match[2]?.length ?? 0)
  if (!Number.isInteger(exponent) || Math.abs(exponent) > 128)
    return null
  let numerator = BigInt(`${match[1]}${match[2] ?? ''}`) * BigInt(scale)
  const denominator = exponent < 0 ? 10n ** BigInt(-exponent) : 1n
  if (exponent > 0)
    numerator *= 10n ** BigInt(exponent)
  if (numerator % denominator !== 0n)
    return null
  const result = Number(numerator / denominator)
  return Number.isSafeInteger(result) ? result : null
}

function unitField(read: () => number | null, write: (value: number) => void, scale: number) {
  const draft = ref<string>()
  const display = () => read() === null ? '' : String(read()! / scale)
  const input = computed({
    get: () => draft.value ?? display(),
    set: (raw: string) => {
      draft.value = raw
      const parsed = scaledInteger(raw, scale)
      if (parsed !== null)
        write(parsed)
    },
  })
  const invalid = computed(() => draft.value !== undefined && scaledInteger(draft.value, scale) === null)
  return {
    input,
    invalid,
    setInput: (raw: string) => { input.value = raw },
    reset: () => { draft.value = undefined },
  }
}

export function useRuntimeUnits(form: RuntimeForm) {
  const tuning = (key: 'websocketLargeRequestThresholdBytes' | 'streamPrefetchBytes' | 'websocketMaxAgeMs' | 'websocketStreamIdleTimeoutMs' | 'websocketFailureWindowMs' | 'websocketFailureOpenDurationMs', scale: number) =>
    unitField(() => form.requestTuning[key], (value) => { form.requestTuning[key] = value }, scale)
  const fields = {
    bodyMiB: unitField(() => form.responsesMaxDecompressedBodyBytes, (value) => { form.responsesMaxDecompressedBodyBytes = value }, 1024 * 1024),
    intervalSeconds: unitField(() => form.requestIntervalMs, (value) => { form.requestIntervalMs = value }, 1000),
    largeRequestMiB: tuning('websocketLargeRequestThresholdBytes', 1024 * 1024),
    prefetchKiB: tuning('streamPrefetchBytes', 1024),
    maxAgeSeconds: tuning('websocketMaxAgeMs', 1000),
    idleSeconds: tuning('websocketStreamIdleTimeoutMs', 1000),
    failureWindowSeconds: tuning('websocketFailureWindowMs', 1000),
    failureOpenSeconds: tuning('websocketFailureOpenDurationMs', 1000),
  }
  return {
    fields,
    invalid: computed(() => Object.values(fields).some(field => field.invalid.value)),
    reset: () => Object.values(fields).forEach(field => field.reset()),
    snapshot: () => Object.values(fields).map(field => field.input.value),
  }
}

export type RuntimeUnitFields = ReturnType<typeof useRuntimeUnits>['fields']
