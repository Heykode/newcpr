//! Authenticated IPv6 control plane. Listing never allocates an address.

use std::{collections::BTreeMap, net::Ipv6Addr};

use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use gateway_admin::model::{
    Revision,
    egress::{ProviderEgressMutation, ReplaceProviderEgress, SetProviderAccountEgress},
};
use gateway_core::{
    account::ProviderAccountId,
    provider_ports::egress::{EgressMode, ProviderEgressAddress, ProviderEgressConfig},
};
use serde::{Deserialize, Serialize};

use super::{AdminAuth, AdminEnvelope, AdminError, AdminJson, AdminResponse, AdminSessionState};

const MAX_ADDRESSES: usize = 4096;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EgressAddressRequest {
    pub id: String,
    pub address: String,
    /// New addresses are disabled unless the administrator explicitly enables them.
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReplaceEgressRequest {
    pub revision: u64,
    pub default_mode: String,
    pub addresses: Vec<EgressAddressRequest>,
}

impl ReplaceEgressRequest {
    pub fn into_command(self) -> Result<ReplaceProviderEgress, AdminError> {
        if self.addresses.len() > MAX_ADDRESSES {
            return Err(AdminError::bad_request("IPv6 地址池最多支持 4096 个地址"));
        }
        Ok(ReplaceProviderEgress {
            expected_revision: revision(self.revision)?,
            default_mode: mode(&self.default_mode)?,
            addresses: self
                .addresses
                .into_iter()
                .map(|address| {
                    let source = address
                        .address
                        .parse()
                        .map_err(|_| AdminError::bad_request("IPv6 地址格式不合法"))?;
                    validate_address(source)?;
                    Ok(ProviderEgressAddress {
                        id: address.id,
                        address: source,
                        enabled: address.enabled,
                    })
                })
                .collect::<Result<_, AdminError>>()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccountEgressRequest {
    pub account_id: String,
    pub revision: u64,
    /// Omitted, null or blank inherits; "unchanged" explicitly disables IPv6 policy.
    pub mode: Option<String>,
}

impl AccountEgressRequest {
    pub fn into_command(self) -> Result<SetProviderAccountEgress, AdminError> {
        Ok(SetProviderAccountEgress {
            account_id: ProviderAccountId::new(self.account_id)
                .map_err(|_| AdminError::bad_request("账号 ID 不合法"))?,
            expected_revision: revision(self.revision)?,
            mode: self
                .mode
                .as_deref()
                .filter(|value| !value.trim().is_empty())
                .map(mode)
                .transpose()?,
        })
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ExpandEgressRequest {
    pub start: String,
    pub end: String,
}

impl ExpandEgressRequest {
    pub fn expand(self) -> Result<Vec<EgressAddressView>, AdminError> {
        let start: Ipv6Addr = self
            .start
            .parse()
            .map_err(|_| AdminError::bad_request("起始 IPv6 地址格式不合法"))?;
        let end: Ipv6Addr = self
            .end
            .parse()
            .map_err(|_| AdminError::bad_request("结束 IPv6 地址格式不合法"))?;
        let length = u128::from(end)
            .checked_sub(u128::from(start))
            .and_then(|length| length.checked_add(1))
            .filter(|length| *length <= MAX_ADDRESSES as u128)
            .ok_or_else(|| AdminError::bad_request("IPv6 范围必须正向且不超过 4096 个地址"))?;
        (0..length)
            .map(|offset| {
                let address = Ipv6Addr::from(u128::from(start) + offset);
                validate_address(address)?;
                Ok(EgressAddressView {
                    id: format!("ipv6_{:032x}", u128::from(address)),
                    address: address.to_string(),
                    enabled: false,
                })
            })
            .collect()
    }
}

fn validate_address(address: Ipv6Addr) -> Result<(), AdminError> {
    if !gateway_core::provider_ports::egress::is_valid_source_address(address) {
        return Err(AdminError::bad_request(
            "IPv6 地址必须是单播地址，不支持回环、映射或链路本地地址",
        ));
    }
    Ok(())
}

fn mode(value: &str) -> Result<EgressMode, AdminError> {
    EgressMode::parse(value).ok_or_else(|| AdminError::bad_request("IPv6 出口模式不合法"))
}

fn revision(value: u64) -> Result<Revision, AdminError> {
    if value == 0 || value > i64::MAX as u64 {
        return Err(AdminError::bad_request("IPv6 配置版本不合法"));
    }
    Revision::new(value).map_err(|_| AdminError::bad_request("IPv6 配置版本不合法"))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressAddressView {
    pub id: String,
    pub address: String,
    pub enabled: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct EgressConfigView {
    revision: u64,
    default_mode: &'static str,
    addresses: Vec<EgressAddressView>,
    account_overrides: BTreeMap<String, Option<&'static str>>,
    fixed_bindings: BTreeMap<String, String>,
}

impl From<ProviderEgressConfig> for EgressConfigView {
    fn from(config: ProviderEgressConfig) -> Self {
        Self {
            revision: config.revision,
            default_mode: config.default_mode.as_str(),
            addresses: config
                .addresses
                .into_iter()
                .map(|address| EgressAddressView {
                    id: address.id,
                    address: address.address.to_string(),
                    enabled: address.enabled,
                })
                .collect(),
            account_overrides: config
                .account_overrides
                .into_iter()
                .map(|(id, mode)| (id.to_string(), mode.map(EgressMode::as_str)))
                .collect(),
            fixed_bindings: config
                .fixed_bindings
                .into_iter()
                .map(|(id, address)| (id.to_string(), address.to_string()))
                .collect(),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EgressMutationView {
    config_revision: u64,
    config: EgressConfigView,
}

impl From<ProviderEgressMutation> for EgressMutationView {
    fn from(mutation: ProviderEgressMutation) -> Self {
        Self {
            config_revision: mutation.config_revision.get(),
            config: mutation.config.into(),
        }
    }
}

pub fn router<S>() -> Router<S>
where
    S: AdminSessionState + Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/api/admin/ipv6-egress", get(load::<S>))
        .route("/api/admin/ipv6-egress/update", post(replace::<S>))
        .route("/api/admin/ipv6-egress/account", post(set_account::<S>))
        .route("/api/admin/ipv6-egress/expand", post(expand))
}

async fn load<S>(_: AdminAuth, State(state): State<S>) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let config = state
        .admin_services()
        .egress()
        .load()
        .await
        .map_err(super::wire::map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(EgressConfigView::from(config)),
    ))
}

async fn replace<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<ReplaceEgressRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let mutation = state
        .admin_services()
        .egress()
        .replace(request.into_command()?, &auth.context().mutation_context())
        .await
        .map_err(super::wire::map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(EgressMutationView::from(mutation)),
    ))
}

async fn set_account<S>(
    auth: AdminAuth,
    State(state): State<S>,
    AdminJson(request): AdminJson<AccountEgressRequest>,
) -> Result<impl IntoResponse, AdminError>
where
    S: AdminSessionState + Send + Sync,
{
    let mutation = state
        .admin_services()
        .egress()
        .set_account(request.into_command()?, &auth.context().mutation_context())
        .await
        .map_err(super::wire::map_admin_service_error)?;
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(EgressMutationView::from(mutation)),
    ))
}

async fn expand(
    _: AdminAuth,
    AdminJson(request): AdminJson<ExpandEgressRequest>,
) -> Result<impl IntoResponse, AdminError> {
    Ok(AdminResponse::new(
        StatusCode::OK,
        AdminEnvelope::ok(request.expand()?),
    ))
}
