//! Relogin and maintenance use the current pool binding, never a second account setting.
use super::*;
use crate::model::relogin_maintenance::ReloginEgressView;

impl DefaultReloginService {
    pub(super) async fn login_request(
        &self,
        entry: &ReloginEntry,
        pool: &[AccountRecord],
        workspace_id: Option<String>,
    ) -> Result<ReloginRequest, AdminError> {
        let matches = matching_accounts(entry, pool);
        let selected = statistics_account(entry, &matches);
        if entry.target.is_some() && selected.is_none() {
            return Err(AdminError::conflict("原账号目标已变化，请重新选择工作区"));
        }
        let account = selected.or_else(|| {
            // Highest-plan selection may inspect several workspaces of one principal.
            // Only an identical effective route permits sharing that Web login.
            matches.first().copied()
        });
        let mut proxies = BTreeMap::new();
        let mut route = self.egress_view(entry, account, &mut proxies).await?;
        if statistics_account(entry, &matches).is_none() && matches.len() > 1 {
            for other in matches.iter().skip(1) {
                let other_route = self.egress_view(entry, Some(other), &mut proxies).await?;
                if other.effective_outbound_proxy()
                    != account.and_then(AccountRecord::effective_outbound_proxy)
                    || route.source_ip != other_route.source_ip
                    || route.mode != other_route.mode
                {
                    return Err(AdminError::conflict(
                        "同邮箱工作区使用不同出口，请先选择要维护的原工作区",
                    ));
                }
            }
        }
        let proxy = if let Some(account) = account {
            account.effective_outbound_proxy().cloned()
        } else {
            super::super::import_proxy_binding(self.proxies.as_ref(), import_proxy_id(entry))
                .await?
                .map(|reservation| reservation.binding.proxy)
        };
        if route.mode.is_some_and(|mode| mode.is_random()) && route.source_ip.is_none() {
            route.source_ip = self
                .provider
                .select_relogin_source(account.expect("pooled egress"))
                .map_err(|error| map_provider_error(error, "relogin egress"))?;
            if route.source_ip.is_none() {
                return Err(AdminError::unavailable("账号 IPv6 出口暂不可用"));
            }
        }
        Ok(ReloginRequest {
            web_session: entry.maintenance.session.clone(),
            email: entry.email.clone(),
            password: entry.password.clone(),
            mfa_secret: entry.mfa_secret.clone(),
            workspace_id,
            outbound_proxy: proxy,
            source_ip: route.source_ip,
            egress_mode: route.mode,
        })
    }

    pub(super) async fn egress_view(
        &self,
        entry: &ReloginEntry,
        account: Option<&AccountRecord>,
        proxies: &mut BTreeMap<String, crate::model::proxies::ProxyRecord>,
    ) -> Result<ReloginEgressView, AdminError> {
        let mut view = ReloginEgressView {
            kind: "default".into(),
            label: "部署默认出口".into(),
            source: if account.is_some() {
                "account"
            } else if entry.enrollment.is_some() || entry.maintenance.import_egress.is_some() {
                "import"
            } else {
                "default"
            }
            .into(),
            account_id: account.map(|row| row.id.clone()),
            ..Default::default()
        };
        let proxy_id = account
            .map(|row| row.outbound_proxy_id.as_deref())
            .unwrap_or_else(|| import_proxy_id(entry));
        view.proxy_id = proxy_id.map(str::to_owned);
        let proxy = account.and_then(AccountRecord::effective_outbound_proxy);
        if let Some(id) = proxy_id {
            if !proxies.contains_key(id) {
                match self.proxies.get(id).await {
                    Ok(record) => {
                        proxies.insert(id.to_owned(), record);
                    }
                    Err(_) => {
                        view.kind = "unavailable".into();
                        view.label = "绑定代理信息暂不可用".into();
                    }
                }
            }
            if let Some(record) = proxies.get(id) {
                view.label = record.name.clone();
                let effective = if account.is_some() {
                    proxy
                } else {
                    record.enabled.then_some(&record.proxy)
                };
                if let Some(effective) = effective {
                    view.kind = "proxy".into();
                    view.address = Some(effective.endpoint());
                    view.exit_ip = record
                        .last_test
                        .as_ref()
                        .filter(|test| test.success && effective == &record.proxy)
                        .and_then(|test| test.exit_ip)
                        .map(|ip| ip.to_string());
                    view.latency_ms = record
                        .last_test
                        .as_ref()
                        .filter(|test| test.success && effective == &record.proxy)
                        .map(|test| test.latency_ms);
                    view.checked_at = record.last_test_at;
                } else {
                    view.label = format!("{}（已停用，部署默认出口）", view.label);
                }
            } else if let Some(proxy) = proxy {
                view.kind = "proxy".into();
                view.label = "账号代理".into();
                view.address = Some(proxy.endpoint());
            }
        } else if let Some(proxy) = proxy {
            view.kind = "proxy".into();
            view.label = "账号代理".into();
            view.address = Some(proxy.endpoint());
        }
        if let Some(account) = account {
            let state = self
                .provider
                .relogin_egress_state()
                .map_err(|error| map_provider_error(error, "relogin egress"))?;
            if let Some(state) = state {
                let id = ProviderAccountId::new(account.id.clone())
                    .map_err(|_| AdminError::invalid("账号标识无效"))?;
                let mode = state
                    .account_overrides
                    .get(&id)
                    .copied()
                    .ok_or_else(|| AdminError::conflict("账号出口配置已变化，请刷新后重试"))?
                    .unwrap_or(state.default_mode);
                if mode.is_active() {
                    if proxy.is_some() {
                        return Err(AdminError::conflict("账号代理与 IPv6 出口策略冲突"));
                    }
                    // A Web session keeps its chosen rotating source while the configured
                    // policy and enabled pool still allow it. Fixed policies use the live binding.
                    let source = if mode.is_random() {
                        entry
                            .maintenance
                            .session
                            .as_ref()
                            .filter(|session| session.egress_mode == Some(mode))
                            .and_then(|session| session.source_ip)
                            .filter(|source| {
                                state
                                    .addresses
                                    .iter()
                                    .any(|item| item.enabled && item.address == *source)
                            })
                    } else {
                        state.fixed_bindings.get(&id).copied()
                    };
                    let source = source.filter(|source| {
                        state
                            .addresses
                            .iter()
                            .any(|item| item.enabled && item.address == *source)
                    });
                    if source.is_none() && !mode.is_random() {
                        return Err(AdminError::unavailable("账号绑定的 IPv6 出口不可用"));
                    }
                    view.kind = "ipv6".into();
                    view.label = if mode.is_random() {
                        "IPv6 会话出口"
                    } else {
                        "账号固定 IPv6"
                    }
                    .into();
                    view.address = source.map(|source| source.to_string());
                    view.source_ip = source;
                    view.mode = Some(mode);
                }
            }
        }
        Ok(view)
    }
}

pub(super) fn import_proxy_id(entry: &ReloginEntry) -> Option<&str> {
    entry
        .enrollment
        .as_ref()
        .map(|value| value.config.outbound_proxy_id.as_deref())
        .unwrap_or_else(|| {
            entry
                .maintenance
                .import_egress
                .as_ref()
                .and_then(|value| value.proxy_id.as_deref())
        })
}
pub(super) fn import_egress_mode(
    entry: &ReloginEntry,
) -> Option<Option<gateway_core::provider_ports::egress::EgressMode>> {
    entry
        .enrollment
        .as_ref()
        .map(|value| value.config.egress_mode)
        .unwrap_or_else(|| {
            entry
                .maintenance
                .import_egress
                .as_ref()
                .and_then(|value| value.mode)
        })
}
