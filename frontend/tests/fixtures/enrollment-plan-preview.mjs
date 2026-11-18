// State belongs only to this loopback demo. No credentials, external requests or background login jobs.
export function enrollmentExample(library, accounts, groups, templates, settings) {
  const proxies = ['A', 'B', 'C'].map((letter, index) => ({
    id: `fixture-${letter}`,
    name: `代理 ${letter}${index === 2 ? '（不可用）' : ''}`,
    enabled: true,
    endpoint: `http://192.0.2.${index + 1}:8080`,
    revision: 1,
    hasAuthentication: false,
    accountCount: 0,
    maxWebsocketConnections: 0,
    lastTest: null,
  }))
  groups.push({ ...groups[0], id: 'grp_00000000000000000000000000000092', name: '备用组' })
  templates.push({ id: 'fixture-standard', revision: 1, config: { name: '常用入池 · 代理 A', groupIds: [groups[0].id], weight: 1, concurrencyLimit: null, enabled: true, outboundProxyId: proxies[0].id, egressMode: 'unchanged', requestProxySource: 'account' } })
  const quality = [{ id: 'fixture-check', revision: 1, name: '入池质量检测', config: {} }]
  const base = { ...library[0], automatic: false, status: 'pending', planType: null, workspaceId: null, credentialStatus: 'none', verifiedAt: null, expiresAt: null }
  const sample = (id, name) => ({ ...structuredClone(base), id, email: `${id}@example.invalid`, displayName: name || null })
  const existing = library.at(-1)
  Object.assign(existing, { displayName: '已入池示例', poolStatus: 'synced', egress: { kind: 'proxy', source: 'account', label: proxies[1].name } })
  library.splice(0, library.length, sample('new-one', '新账号示例'), sample('verify', '需人工验证示例'), existing)
  let nextId = 1
  function advance() {
    const now = Date.now()
    for (const row of library) {
      const plan = row.enrollmentPlan
      if (!plan)
        continue
      if (plan.state === 'scheduled' && !settings.paused && Date.parse(plan.when) <= now) {
        plan.state = 'running'
        plan.startedAt = now
        row.status = 'running'
        row.revision++
      }
      if (plan.state !== 'running' || now - plan.startedAt < 1200)
        continue
      row.revision++
      row.lastAttemptAt = new Date(now).toISOString()
      const proxy = proxies.find(proxy => proxy.id === plan.exit.outboundProxyId)
      if (proxy?.id === 'fixture-C' || row.id === 'verify') {
        plan.state = proxy?.id === 'fixture-C' ? 'failed' : 'verification'
        row.status = 'failed'
        row.message = plan.state === 'failed' ? '所选出口不可用' : '等待人工验证'
        continue
      }
      plan.state = 'done'
      const account = {
        ...structuredClone(accounts[0]),
        id: `pool-${row.id}`,
        email: row.email,
        customName: plan.settings.customName || null,
        enabled: plan.settings.enabled,
        groups: groups.filter(group => plan.settings.groupIds.includes(group.id)),
        modelAccess: plan.settings.modelAccess,
        outboundProxyId: proxy?.id ?? null,
        outboundProxyEndpoint: proxy?.endpoint ?? null,
        reloginCount: 1,
        lastReloginAt: row.lastAttemptAt,
      }
      accounts.push(account)
      Object.assign(row, {
        displayName: account.customName || row.email,
        poolAccountIds: [account.id],
        credentialStatus: 'verified',
        poolStatus: 'synced',
        reloginCount: 1,
        status: 'ready',
        lastReloginAt: row.lastAttemptAt,
        egress: { kind: proxy ? 'proxy' : plan.settings.proxyMode === 'ipv6' ? 'ipv6' : 'default', label: plan.exitLabel, source: 'account', proxyId: proxy?.id ?? null },
      })
    }
  }
  function command(body) {
    advance()
    if (body.mode === 'sample') {
      library.unshift(sample(`sample-${nextId++}`))
      return
    }
    const row = library.find(row => row.id === body.id)
    if (!row || row.poolAccountIds.length || row.revision !== body.revision || row.enrollmentPlan?.state === 'running')
      throw new Error('账号状态已变化，请关闭后重新查看')
    if (body.mode === 'cancel') {
      if (row.enrollmentPlan?.state !== 'scheduled')
        throw new Error('没有可取消的定时安排')
      Object.assign(row.enrollmentPlan, { state: 'draft', when: null })
    }
    else {
      if (!['draft', 'now', 'scheduled'].includes(body.mode) || !body.settings || !body.exit)
        throw new Error('安排无效')
      if (body.mode === 'now' && settings.paused)
        throw new Error('队列已暂停')
      if (body.mode === 'scheduled' && !(Date.parse(body.when) > Date.now()))
        throw new Error('请选择晚于现在的执行时间')
      const proxy = proxies.find(proxy => proxy.id === body.exit.outboundProxyId)
      if (body.exit.outboundProxyId && !proxy)
        throw new Error('所选代理不存在')
      const exitLabel = proxy?.name ?? ({ ipv6: 'IPv6 出口', mihomo: 'Mihomo', proxy_pool: '代理池' }[body.settings.proxyMode] || '部署默认出口')
      row.enrollmentPlan = { state: body.mode === 'now' ? 'running' : body.mode, settings: body.settings, exit: body.exit, exitLabel, when: body.when, startedAt: Date.now() }
      row.status = body.mode === 'now' ? 'running' : 'pending'
    }
    row.revision++
  }
  return { advance, command, proxies, quality }
}
