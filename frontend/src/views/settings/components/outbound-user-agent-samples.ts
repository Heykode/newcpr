import catalog from './outbound-user-agent-samples.json'

// Exact rows from pinned third-party release assets, checked on 2026-09-24.
// Only existing parser-compatible platforms are offered; never rewrite a UA.
// These are not official artifact verification or an automatic update source.
export const userAgentSamples = catalog.samples

export function sampleSource(sample: typeof userAgentSamples[number]) {
  return sample.client === 'Desktop' ? catalog.sources.desktop : catalog.sources.cli
}

export const userAgentSampleOptions = userAgentSamples.map(sample => ({
  value: sample.id,
  label: `${sample.client} · ${sample.platform}`,
  client: sample.client,
}))

export const userAgentSampleClients = [...new Set(userAgentSamples.map(sample => sample.client))]
  .map(client => ({ label: client, value: client }))
