//! 可复用的账号出口配置，以及脱敏后的连通性测试结果。

use chrono::{DateTime, Utc};
use gateway_core::account::{OutboundProxy, RequestLocation};
use serde::{Deserialize, Serialize};

use super::{PageSize, Revision, account_groups::AccountGroupRef};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountProxySelection {
    Direct,
    Url(OutboundProxy),
    Saved(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportProxyBinding {
    pub id: String,
    pub proxy: OutboundProxy,
}

#[derive(Debug, Clone)]
pub struct ProxyListQuery {
    pub page: u32,
    pub page_size: PageSize,
    pub search: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyAccountRef {
    pub id: String,
    pub name: String,
    pub email: Option<String>,
    pub provider_kind: String,
    pub authentication_kind: String,
    pub plan_type: Option<String>,
    pub plan_type_display: Option<String>,
    pub groups: Vec<AccountGroupRef>,
    pub enabled: bool,
}

/// 按代理查询关联账号，分页与搜索均在存储层执行。
#[derive(Debug, Clone)]
pub struct ProxyAccountListQuery {
    pub proxy_id: String,
    pub page: u32,
    pub page_size: PageSize,
    pub search: String,
}

#[derive(Debug, Clone)]
pub struct ProxyAccountPage {
    pub items: Vec<ProxyAccountRef>,
    pub total: u64,
    pub page: u32,
    pub page_size: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyTestResult {
    pub location: ProxyLocationDetection,
    pub success: bool,
    pub latency_ms: u64,
    pub exit_ip: Option<std::net::IpAddr>,
    pub exit_ipv4: Option<std::net::Ipv4Addr>,
    pub exit_ipv6: Option<std::net::Ipv6Addr>,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct ProxyRecord {
    pub auto_location: bool,
    pub detected_location: Option<DetectedProxyLocation>,
    pub request_location: Option<gateway_core::account::RequestLocation>,
    pub id: String,
    pub name: String,
    pub proxy: OutboundProxy,
    pub revision: Revision,
    pub account_count: u64,
    pub last_test_at: Option<DateTime<Utc>>,
    pub last_test: Option<ProxyTestResult>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct ProxyPage {
    pub items: Vec<ProxyRecord>,
    pub total: u64,
    pub page: u32,
    pub page_size: u16,
}

#[derive(Debug, Clone)]
pub struct NewProxy {
    pub auto_location: bool,
    /// Server-owned probe result; never accepted from the HTTP caller.
    pub test: Option<ProxyTestResult>,
    pub request_location: Option<gateway_core::account::RequestLocation>,
    pub name: String,
    pub proxy: OutboundProxy,
}

#[derive(Debug, Clone)]
pub struct UpdateProxy {
    pub auto_location: Option<bool>,
    pub test: Option<ProxyTestResult>,
    /// Missing preserves the value; explicit null clears the override.
    pub request_location: Option<Option<gateway_core::account::RequestLocation>>,
    pub id: String,
    pub revision: Revision,
    pub name: String,
    pub proxy: Option<OutboundProxy>,
}

#[derive(Debug, Clone)]
pub struct ProxyMutation {
    pub config_revision: Revision,
    pub record: ProxyRecord,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum ProxyLocationDetection {
    #[default]
    NotRequested,
    Detected {
        location: RequestLocation,
    },
    Failed {
        message: String,
    },
    Conflict,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetectedProxyLocation {
    pub location: RequestLocation,
    pub exit_ipv4: Option<std::net::Ipv4Addr>,
    pub exit_ipv6: Option<std::net::Ipv6Addr>,
    pub detected_at: DateTime<Utc>,
}

impl ProxyRecord {
    #[must_use]
    pub fn effective_location(&self) -> Option<&RequestLocation> {
        if self.auto_location {
            self.detected_location.as_ref().map(|value| &value.location)
        } else {
            self.request_location.as_ref()
        }
    }

    #[must_use]
    pub fn detected_location_after_test(
        &self,
        result: &ProxyTestResult,
    ) -> Option<DetectedProxyLocation> {
        match &result.location {
            ProxyLocationDetection::Detected { location } => Some(DetectedProxyLocation {
                location: location.clone(),
                exit_ipv4: result.exit_ipv4,
                exit_ipv6: result.exit_ipv6,
                detected_at: Utc::now(),
            }),
            ProxyLocationDetection::Failed { .. } => {
                self.detected_location.clone().filter(|previous| {
                    !result.success
                        || (result
                            .exit_ipv4
                            .is_none_or(|ip| previous.exit_ipv4 == Some(ip))
                            && result
                                .exit_ipv6
                                .is_none_or(|ip| previous.exit_ipv6 == Some(ip)))
                })
            }
            ProxyLocationDetection::Conflict => None,
            ProxyLocationDetection::NotRequested => self.detected_location.clone(),
        }
    }
}
