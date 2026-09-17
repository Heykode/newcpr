import { reloginEntries } from './relogin-count-data.mjs'

const workspace = '12345678-1234-5678-9012-123456789012'
export const layoutEntries = Array.from({ length: 5 }, (_, index) => {
  const row = {
    ...reloginEntries[0],
    id: `relogin_layout_${index}`,
    email: `workspace-recovery-sample-${index}@example.invalid`,
    importedAt: new Date(Date.UTC(2026, 8, 17, 1, 5 - index)).toISOString(),
    workspaceId: workspace,
    planType: 'self_serve_business_prolite',
    poolAccountIds: [`acct_layout_${index}`],
    reloginAccountId: `acct_layout_${index}`,
    poolAccounts: [{
      id: `acct_layout_${index}`,
      workspaceId: workspace,
      planType: 'self_serve_business_prolite',
      enabled: index !== 3,
      status: index === 1 ? 'error' : 'normal',
      errorReason: index === 1 ? 'credential_expired' : null,
      errorMessage: index === 1 ? 'HTTP 401: credential expired' : null,
    }],
  }
  if (index === 0) {
    row.status = 'pending'
    row.credentialStatus = 'none'
    row.poolStatus = 'absent'
    row.poolAccountIds = []
    row.poolAccounts = []
    row.reloginAccountId = null
    row.workspaceId = null
    row.planType = null
    row.message = '资料已导入'
  }
  else if (index === 1) {
    row.status = 'uncertain'
    row.poolStatus = 'pending_push'
    row.message = '推送结果未确认，请先核对号池'
  }
  else if (index === 4) {
    row.poolStatus = 'pending_push'
    row.message = '已获取新 JSON'
  }
  return row
})
