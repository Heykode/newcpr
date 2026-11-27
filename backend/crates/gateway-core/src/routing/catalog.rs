//! 快照编译与客户端读取需要的 Provider 目录合同，不暴露执行或账号选择能力。

use std::collections::{BTreeMap, BTreeSet};

use futures::future::BoxFuture;

use crate::account::ProviderAccountId;
use crate::identity::ProviderKind;
use crate::operation::RawJsonPayload;

use super::{
    ModelCapabilities, ModelPresentation, PublicModelId, PublicModelProfile, UpstreamModelId,
};

/// Provider 声明的推理档位目标及确认该分组成员关系的账号
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReasoningModelVariant {
    pub model: UpstreamModelId,
    pub accounts: BTreeSet<ProviderAccountId>,
}

/// 上游账号的聊天菜单声明，未声明时仍可按模型 ID 发现
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum AgentModelListing {
    #[default]
    Unspecified,
    Hidden,
    Listed {
        order: usize,
        display_name: String,
    },
}

/// 具体型号在单个账号目录中声明的能力与说明
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AccountModelMetadata {
    pub agent_listing: AgentModelListing,
    pub description: Option<String>,
    pub context_window_tokens: Option<u64>,
    pub max_context_window_tokens: Option<u64>,
    pub image_input: bool,
    pub max_output_tokens: Option<u64>,
}

/// 可执行公开模型及当前账号范围内的目录来源，不应用客户端展示规则
#[derive(Debug)]
pub struct ExecutableModelMetadata<'a> {
    pub model: PublicModelId,
    pub sources: Vec<ModelMetadataSource<'a>>,
}

/// 来源只暴露目标型号与元数据，不暴露账号身份；未声明账号级元数据时为 None
#[derive(Debug)]
pub struct ModelMetadataSource<'a> {
    pub target: &'a UpstreamModelId,
    pub metadata: Option<&'a AccountModelMetadata>,
}

impl ModelPresentation {
    /// 分组向客户端承诺的能力必须覆盖每个可选档位和账号
    pub(super) fn apply_member_metadata<'a>(
        &mut self,
        mut members: impl Iterator<Item = Option<&'a AccountModelMetadata>>,
    ) {
        let Some(first) = members.next() else {
            return;
        };
        let mut has_metadata = first.is_some();
        let mut description = first.and_then(|m| m.description.as_deref());
        let mut window = first.and_then(|m| m.context_window_tokens);
        let mut max_window = first.and_then(|m| m.max_context_window_tokens);
        let mut images = first.is_some_and(|m| m.image_input);
        for member in members {
            has_metadata |= member.is_some();
            if description != member.and_then(|m| m.description.as_deref()) {
                description = None;
            }
            window = window.min(member.and_then(|m| m.context_window_tokens));
            max_window = max_window.min(member.and_then(|m| m.max_context_window_tokens));
            images &= member.is_some_and(|m| m.image_input);
        }
        if has_metadata {
            self.description = description.map(str::to_owned);
            self.context_window_tokens = window;
            self.max_context_window_tokens = max_window;
            self.image_input = images;
        }
    }
}

/// Provider 原生客户端目录条目；Core 只解释模型标识，不解释协议正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelDescriptor {
    pub model: UpstreamModelId,
    pub payload: RawJsonPayload,
}

/// 对外目录保留原生正文；只有没有原生目录的 Provider 才使用通用画像适配。
#[derive(Debug, Clone)]
pub enum PublicModelDescriptor {
    Native {
        model: PublicModelId,
        payload: RawJsonPayload,
    },
    Adapted(PublicModelProfile),
}

/// Provider 实时目录编译后的单模型能力。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderModelCapabilities {
    upstream_model: UpstreamModelId,
    capabilities: ModelCapabilities,
    presentation: Option<ModelPresentation>,
    catalog_accounts: Option<BTreeSet<ProviderAccountId>>,
}

impl ProviderModelCapabilities {
    #[must_use]
    pub const fn new(upstream_model: UpstreamModelId, capabilities: ModelCapabilities) -> Self {
        Self {
            upstream_model,
            capabilities,
            presentation: None,
            catalog_accounts: None,
        }
    }

    #[must_use]
    pub fn with_presentation(mut self, presentation: ModelPresentation) -> Self {
        self.presentation = Some(presentation);
        self
    }

    /// Discovery sources restrict catalog visibility, never inference authorization.
    #[must_use]
    pub fn with_catalog_accounts(mut self, accounts: BTreeSet<ProviderAccountId>) -> Self {
        self.catalog_accounts = Some(accounts);
        self
    }

    #[must_use]
    pub const fn catalog_accounts(&self) -> Option<&BTreeSet<ProviderAccountId>> {
        self.catalog_accounts.as_ref()
    }

    #[must_use]
    pub const fn upstream_model(&self) -> &UpstreamModelId {
        &self.upstream_model
    }

    #[must_use]
    pub const fn capabilities(&self) -> &ModelCapabilities {
        &self.capabilities
    }

    #[must_use]
    pub const fn presentation(&self) -> Option<&ModelPresentation> {
        self.presentation.as_ref()
    }
}

/// Provider 实时目录成功发布后的进程内单调代次。
///
/// 代次只表达“目录内容已经变化”，不承载模型、ETag 或 Provider 私有数据。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProviderCatalogGeneration(u64);

impl ProviderCatalogGeneration {
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

/// 目录未知，不能据此判定某个模型不存在；不携带执行错误或上游协议细节。
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("provider model catalog is unavailable")]
pub struct ProviderCatalogUnavailable;

/// 快照编译与对账使用的对象安全目录端口。
pub trait ProviderCatalogPort: Send + Sync {
    /// Only exhaustive catalogs can prove that an unlisted model is unavailable.
    fn model_catalog_is_exhaustive(&self, _provider: &ProviderKind) -> bool {
        true
    }

    /// 返回全部已注册 Provider 的目录代次；注册集合在初始化后保持不变。
    ///
    /// 即使某个目录暂时不可读，也必须保留它的 Provider 与最近成功发布的代次。
    fn catalog_generations(&self) -> BTreeMap<ProviderKind, ProviderCatalogGeneration>;

    /// 查询模型发现事实；只有完整目录的空列表能证明无模型，失败表示未知。
    fn query_model_capabilities(
        &self,
        provider: &ProviderKind,
    ) -> BoxFuture<'_, Result<Vec<ProviderModelCapabilities>, ProviderCatalogUnavailable>>;
}
