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
  templates.push({ id: 'fixture-split', revision: 1, config: { ...templates[0].config, name: '账号 A · 维护 B', credentialProxyId: proxies[1].id } })
  const quality = [{ id: 'fixture-check', revision: 1, name: '入池质量检测', config: {} }]
  const base = { ...library[0], automatic: false, status: 'pending', planType: null, workspaceId: null, credentialStatus: 'none', verifiedAt: null, expiresAt: null }
  const sample = (id, name) => ({ ...structuredClone(base), id, email: `${id}@example.invalid`, displayName: name || null })
  const existing = library.at(-1)
  Object.assign(accounts[0], { outboundProxyId: proxies[1].id, outboundProxyEndpoint: proxies[1].endpoint })
  Object.assign(existing, { displayName: '已入池示例', poolStatus: 'synced', egress: { kind: 'proxy', source: 'account', label: proxies[1].name } })
  library.splice(0, library.length, sample('new-one', '新账号示例'), sample('verify', '需人工验证示例'), existing)
  let nextId = 1
  function proxyFor(id) {
    return proxies.find(proxy => proxy.id === id)
  }
  function editAccount(account, config) {
    if (config.outboundProxyId !== undefined && !config.preserveOutboundProxy) {
      account.outboundProxyId = config.outboundProxyId || null
      account.outboundProxyEndpoint = proxyFor(account.outboundProxyId)?.endpoint ?? null
    }
    if (config.credentialProxyId !== undefined)
      account.credentialProxyId = config.credentialProxyId
    const row = library.find(row => row.poolAccountIds.includes(account.id))
    if (row) {
      const proxy = proxyFor(account.outboundProxyId)
      if (config.outboundProxyId !== undefined && !config.preserveOutboundProxy)
        row.egress = { kind: proxy ? 'proxy' : 'default', source: 'account', label: proxy?.name ?? '部署默认出口', proxyId: proxy?.id ?? null }
      row.credentialEgress = account.credentialProxyId ? { kind: 'proxy', source: 'credential', label: proxyFor(account.credentialProxyId)?.name ?? '原代理不可用', proxyId: account.credentialProxyId } : row.egress
    }
  }
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
      const proxy = proxyFor(plan.exit.outboundProxyId)
      const credentialId = plan.settings.credentialProxyId || plan.exit.outboundProxyId
      const unavailable = credentialId && (!proxyFor(credentialId)?.enabled || credentialId === 'fixture-C')
      if (unavailable || row.id === 'verify') {
        plan.state = unavailable ? 'failed' : 'verification'
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
        credentialProxyId: plan.settings.credentialProxyId || null,
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
        credentialEgress: { kind: 'proxy', source: 'credential', label: plan.credentialExitLabel || plan.exitLabel },
      })
    }
  }
  function command(body) {
    advance()
    if (body.newAccount) {
      const input = body.newAccount
      const proxyId = input.outboundProxyId || ''
      body = { ...body, settings: { ...input.settings, customName: input.settings.customName ?? '', proxyMode: proxyId ? 'proxy' : 'direct', proxyId }, exit: { outboundProxyId: proxyId }, config: { ...input.settings, name: '入池设置', outboundProxyId: proxyId || null }, qualityTemplate: input.qualityTemplate }
    }
    if (body.mode === 'sample') {
      return importSamples(2)
    }
    const importing = body.mode === 'import-sample'
    const row = importing ? sample(`sample-${nextId++}`) : library.find(row => row.id === body.id)
    if (importing)
      body = { ...body, revision: row.revision, mode: 'now' }
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
      const credentialProxy = proxyFor(body.settings.credentialProxyId)
      if (body.settings.credentialProxyId && !credentialProxy)
        throw new Error('凭据维护出口不存在，请重新选择')
      row.enrollmentPlan = { state: body.mode === 'now' ? 'running' : body.mode, settings: body.settings, exit: body.exit, exitLabel, credentialExitLabel: credentialProxy?.name, when: body.when, startedAt: Date.now() }
      Object.assign(row.enrollmentPlan, {
        config: body.config || { ...body.settings, name: '入池设置', outboundProxyId: body.exit.outboundProxyId || null },
        customName: body.settings.customName || null,
        purchaseCost: body.settings.purchaseCost || null,
        qualityTemplate: body.qualityTemplate || null,
        template: body.template || null,
      })
      row.egress = { kind: proxy ? 'proxy' : 'default', source: 'import', label: exitLabel }
      row.credentialEgress = { kind: credentialProxy ? 'proxy' : row.egress.kind, source: credentialProxy ? 'credential' : 'import', label: credentialProxy?.name || exitLabel }
      row.status = body.mode === 'now' ? 'running' : 'pending'
    }
    row.revision++
    if (importing)
      library.unshift(row)
  }
  function importSamples(count) {
    const revisions = {}
    for (let i = 0; i < Math.min(count, 500); i++) {
      const row = sample(`sample-${nextId++}`)
      library.unshift(row)
      revisions[row.id] = row.revision
    }
    return { imported: Object.keys(revisions).length, revisions }
  }
  function draftSamples(body) {
    return Object.entries(body.revisions).map(([id, revision]) => {
      const row = library.find(item => item.id === id)
      if (!row || row.enrollmentPlan || row.poolAccountIds.length || row.revision !== revision)
        return { id, success: false, message: '资料已变化、已有安排或已入池' }
      command({ id, revision, mode: 'draft', newAccount: body.newAccount, template: body.template })
      return { id, success: true, message: '完成' }
    })
  }
  return { advance, command, editAccount, proxies, quality, importSamples, draftSamples }
}
