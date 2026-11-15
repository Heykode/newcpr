export interface AnalyticsMetadata {
  coverageStart: string | null
  coverageEnd: string | null
  generatedAt: string
  lagSeconds: number
  state: 'ready' | 'building' | 'lagging' | 'stale'
  percentilesApproximate: boolean
  schemaVersion: number
  histogramVersion: number
  generation: number
}
