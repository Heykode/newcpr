//! 会话绑定的通用选号与认领规则，不解释协议身份或 Provider 账号资格

use super::{ProviderSessionAffinityBinding, ProviderSessionAffinityKey, ProviderSessionClaim};
use crate::account::{
    AccountAffinity, AccountCandidate, AccountSelectionContext, AccountSelectionPolicy,
    AccountSelector, ProviderAccountId,
};

/// 一次选号观察到的绑定；CAS 冲突后须释放租约并重新读取
pub struct ProviderSessionAffinityState<'a> {
    key: &'a ProviderSessionAffinityKey,
    binding: Option<ProviderSessionAffinityBinding>,
    policy: AccountSelectionPolicy,
}

impl<'a> ProviderSessionAffinityState<'a> {
    #[must_use]
    pub const fn new(
        key: &'a ProviderSessionAffinityKey,
        binding: Option<ProviderSessionAffinityBinding>,
        policy: AccountSelectionPolicy,
    ) -> Self {
        Self {
            key,
            binding,
            policy,
        }
    }

    #[must_use]
    pub fn bound_account(&self) -> Option<&ProviderAccountId> {
        self.binding
            .as_ref()
            .map(ProviderSessionAffinityBinding::account_id)
    }

    /// 原生续接的固定账号由调用方优先设置，不以软亲和覆盖
    pub fn apply_preference(&self, context: &mut AccountSelectionContext) {
        if self.policy.account_affinity() != AccountAffinity::Relaxed
            && context.preferred_account.is_none()
        {
            context.preferred_account = self.bound_account().cloned();
            context.preferred_account_overrides_weight =
                self.policy.account_affinity() == AccountAffinity::Strict;
        }
    }

    /// Provider 先投影凭据、额度和模型资格，通用层只区分容量不足与资格失效
    #[must_use]
    pub fn binding_eligible(
        &self,
        candidates: &[AccountCandidate],
        context: &AccountSelectionContext,
    ) -> bool {
        self.bound_account().is_some_and(|bound| {
            candidates.iter().any(|candidate| {
                candidate.account.id() == bound
                    && matches!(
                        AccountSelector.scheduling_blocker(candidate, context),
                        None | Some(crate::account::AccountSchedulingBlocker::ConcurrencyLimit)
                    )
            })
        })
    }

    /// 严格模式允许有界排队；队列关闭或绑定已失去资格时按普通策略重选
    #[must_use]
    pub fn wait_account(
        &self,
        candidates: &[AccountCandidate],
        context: &AccountSelectionContext,
        queue_enabled: bool,
    ) -> Option<&ProviderAccountId> {
        (queue_enabled
            && self.policy.account_affinity() == AccountAffinity::Strict
            && self.binding_eligible(candidates, context))
        .then(|| self.bound_account())
        .flatten()
    }

    /// 保留有效主绑定及其版本；换号时创建新的本地 CAS 版本。
    #[must_use]
    pub fn claim_binding(
        &self,
        selected: &ProviderAccountId,
        binding_eligible: bool,
        pinned: bool,
    ) -> ProviderSessionAffinityBinding {
        if let Some(binding) = &self.binding
            && (binding.account_id() == selected
                || pinned
                || (self.policy.account_affinity() == AccountAffinity::Preferred
                    && binding_eligible))
        {
            return binding.clone();
        }
        ProviderSessionAffinityBinding::new(selected.clone(), super::BindingToken::generate())
    }

    pub fn claim<'s>(
        &'s self,
        binding: &'s ProviderSessionAffinityBinding,
    ) -> ProviderSessionClaim<'s> {
        ProviderSessionClaim {
            key: self.key,
            expected: self.binding.as_ref(),
            binding,
            ttl: self.policy.session_affinity_ttl(),
        }
    }
}
