//! Candidate projection integration; cache contents never authorize an execution lease.
use super::super::scheduling_projection::SchedulingProjectionKey;
use super::*;
use gateway_core::account::AccountAffinity;
use std::time::Instant;
use tokio::sync::OwnedMutexGuard;

pub(super) struct SelectionProjection {
    pub key: Option<SchedulingProjectionKey>,
    pub candidates: Option<Arc<[AccountCandidate]>>,
    pub binding: Option<ProviderSessionAffinityBinding>,
    pub fill: Option<OwnedMutexGuard<()>>,
    pub started: Instant,
}
impl SelectionProjection {
    pub fn owner(&self) -> Option<&ProviderAccountId> {
        self.candidates.as_ref().and_then(|_| {
            self.binding
                .as_ref()
                .map(ProviderSessionAffinityBinding::account_id)
        })
    }
}
impl CodexCredentialSelector {
    pub(super) async fn read_projection(
        &self,
        request: &CredentialSelectionInput<'_>,
    ) -> Result<SelectionProjection, CredentialSelectionError> {
        let key = match (request.upstream_model, request.attempt.account_scope()) {
            (Some(model), Some(scope))
                if request.responses_model
                    && !request.attempt.is_quality_check()
                    && !request.attempt.is_diagnostic_required_account()
                    && request.attempt.required_account().is_none()
                    && matches!(
                        request.attempt.continuation_attempt(),
                        ContinuationAttempt::None | ContinuationAttempt::ReplayAny
                    )
                    && request.affinity_key().is_some()
                    && request
                        .attempt
                        .account_selection_policy()
                        .openai_account_affinity()
                        != AccountAffinity::Relaxed =>
            {
                Some(SchedulingProjectionKey {
                    client_key: request.attempt.client_api_key_ref().clone(),
                    scope: Arc::clone(scope),
                    model: model.to_owned(),
                    policy: request.attempt.account_selection_policy(),
                    requires_websocket: false,
                    concurrency_pool: request.concurrency_pool(),
                    provider_route: request.attempt.provider_route().map(str::to_owned),
                    request_path: request.request_url.path().to_owned(),
                    allow_codex_quota_overdraft: request.attempt.codex_quota_overdraft_eligible(),
                    image_request: request.image_request,
                    search_request: request.search_request,
                    requires_oauth: request.requires_oauth,
                })
            }
            _ => None,
        };
        let binding = if key.is_some()
            && let Some(session) = request.affinity_key()
        {
            match self.lookup_request_affinity(request, session).await {
                SessionAffinityLookup::Bound(binding) => Some(binding),
                SessionAffinityLookup::Missing => None,
                SessionAffinityLookup::Unavailable => return Err(binding_unavailable()),
            }
        } else {
            None
        };
        let (candidates, fill) = match (key.as_ref(), binding.as_ref()) {
            (Some(key), Some(_)) => self.scheduling_projection.read(key).await,
            _ => (None, None),
        };
        let candidates = candidates.filter(|candidates| {
            binding.as_ref().is_some_and(|binding| {
                candidates
                    .iter()
                    .any(|candidate| candidate.account.id() == binding.account_id())
            })
        });
        Ok(SelectionProjection {
            key,
            candidates,
            binding,
            fill,
            started: Instant::now(),
        })
    }
    pub(super) fn publish_projection(
        &self,
        projection: &SelectionProjection,
        candidates: &[AccountCandidate],
    ) {
        if let Some(key) = projection.key.as_ref() {
            self.scheduling_projection
                .publish(key.clone(), projection.started, candidates);
        }
    }
    pub(super) fn invalidate_projection(&self, projection: &SelectionProjection) {
        if let Some(key) = projection.key.as_ref() {
            self.scheduling_projection.invalidate(key);
        }
    }
}
