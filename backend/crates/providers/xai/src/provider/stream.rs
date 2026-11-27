//! xAI inference/compaction 流执行与终态校验。

use super::*;

pub(super) struct GrokStreamAttempt {
    pub(super) client_identity: GrokClientIdentity,
    pub(super) wire_profile: XaiWireProfileState,
    pub(super) credential_recovery: Arc<dyn GrokCredentialRecovery>,
    pub(super) responses_url: Url,
    pub(super) request: GrokResponsesRequest,
    pub(super) upstream_model: UpstreamModelId,
    pub(super) context: AttemptContext,
    pub(super) session: Arc<SelectedGrokSession>,
    pub(super) output_started_at: Instant,
    pub(super) session_capture: Option<GrokSessionCapture>,
}

pub(super) fn cold_http_sse_stream(
    selector: Arc<dyn GrokSessionSelector>,
    transport: Arc<dyn GrokInferenceTransport>,
    attempt: GrokStreamAttempt,
) -> EventStream {
    let GrokStreamAttempt {
        client_identity,
        wire_profile,
        credential_recovery,
        responses_url,
        request,
        upstream_model,
        context,
        session,
        output_started_at,
        mut session_capture,
    } = attempt;
    Box::pin(async_stream::try_stream! {
        if context.cancellation().is_cancelled() {
            Err(provider_error(
                ProviderErrorKind::Cancelled,
                UpstreamSendState::NotSent,
            ))?;
        }
        let headers = build_grok_headers(
            &wire_profile,
            &session,
            &client_identity,
            context.request_id(),
            request.session_id(),
            None,
            &upstream_model,
        );
        let cancellation = context.cancellation().clone();
        let response = {
            let body = request.to_json_bytes().map_err(map_request_error)?;
            let inference_request = GrokInferenceRequest::new(
                responses_url.clone(),
                headers.clone(),
                body,
                session.binding().clone(),
            ).with_trace(context.trace());
            if context.deadline().is_elapsed() {
                Err(provider_error(
                    ProviderErrorKind::Timeout,
                    UpstreamSendState::NotSent,
                ))?;
                return;
            };
            let boundary = tokio::select! {
                biased;
                _ = cancellation.cancelled() => InferenceBoundary::Cancelled,
                _ = context.deadline().wait() => InferenceBoundary::Deadline,
                response = transport.execute(inference_request) => InferenceBoundary::Response(response),
            };
            match boundary {
                InferenceBoundary::Cancelled => {
                    Err(provider_error(ProviderErrorKind::Cancelled, UpstreamSendState::Ambiguous))?;
                    return;
                }
                InferenceBoundary::Deadline => {
                    Err(provider_error(ProviderErrorKind::Timeout, UpstreamSendState::Ambiguous))?;
                    return;
                }
                InferenceBoundary::Response(Ok(response)) => response,
                InferenceBoundary::Response(Err(error)) => {
                    let observation = xai_error_observation(&error)?;
                    let credential_failure = transport_credential_failure(&error, &upstream_model);
                    let error = map_continuation_failure(
                        &context,
                        map_transport_error_for_context(error, &context),
                    );
                    let error = recover_or_record_failure(
                        selector.as_ref(),
                        credential_recovery.as_ref(),
                        &session,
                        error,
                        credential_failure,
                        context.credential_recovery_attempted(),
                    )
                    .await;
                    yield ProviderEvent::observation(observation);
                    Err(error)?;
                    return;
                }
            }
        };

        let mut observation = xai_response_observation(&response)?;
        let base_timings = observation.timings();
        let mut first_token_ms: Option<u64> = None;
        yield ProviderEvent::observation(observation.clone());

        let mut body = response.into_body();
        let mut decoder = GrokCanonicalDecoder::for_request(upstream_model.as_str(), &request)
            .with_pricing(context.pricing().get("xai").and_then(|models| models.get(upstream_model.as_str())).cloned());
        loop {
            if context.deadline().is_elapsed() {
                Err(provider_error(
                    ProviderErrorKind::Timeout,
                    UpstreamSendState::Sent,
                ))?;
                return;
            };
            let next = tokio::select! {
                biased;
                _ = cancellation.cancelled() => Err(provider_error(
                    ProviderErrorKind::Cancelled,
                    UpstreamSendState::Sent,
                )),
                _ = context.deadline().wait() => Err(provider_error(
                    ProviderErrorKind::Timeout,
                    UpstreamSendState::Sent,
                )),
                chunk = body.next() => match chunk {
                    Some(Ok(chunk)) => Ok(Some(chunk)),
                    Some(Err(error)) => Err(map_and_record_stream_transport_failure(
                        selector.as_ref(),
                        &session,
                        error,
                        &upstream_model,
                    ).await),
                    None => Ok(None),
                },
            }?;
            let Some(chunk) = next else {
                break;
            };
            let mut events = match decoder.push(&chunk) {
                Ok(events) => events,
                Err(error) => {
                    let error = map_continuation_failure(&context, error);
                    let error = record_stream_failure(
                        selector.as_ref(),
                        &session,
                        error,
                        &upstream_model,
                    )
                    .await;
                    Err(error)?;
                    return;
                }
            };
            // 首个非前导输出事件（结构帧也算）即上报携带 first_token_ms 的观测，
            // 供 Core 覆盖会话级兜底值。
            if decoder.take_output_start() && first_token_ms.is_none() {
                first_token_ms = Some(
                    u64::try_from(output_started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
                );
                yield ProviderEvent::observation(
                    observation
                        .clone()
                        .with_timings(ProviderResponseTimings {
                            first_token_ms,
                            ..base_timings
                        }),
                );
            }
            if let Some(tier) = decoder.response_service_tier()
                && observation.service_tier() != Some(tier)
            {
                observation = observation.with_service_tier_if_valid(tier.to_owned());
                yield ProviderEvent::observation(observation.clone().with_timings(
                    ProviderResponseTimings { first_token_ms, ..base_timings },
                ));
            }
            let completed = events
                .iter()
                .flat_map(ProviderEvent::canonical_facts)
                .any(|event| matches!(event, GatewayEvent::Completed(_)));
            attach_xai_session_update(&mut events, &mut session_capture)?;
            if completed && session.allows_account_state_mutation() {
                selector.record_success(&session).await;
            }
            for event in events {
                ensure_sent_context(&context)?;
                yield event;
            }
            if completed {
                return;
            }
        }
        let mut final_events = match decoder.finish() {
            Ok(events) => events,
            Err(error) => {
                let error = map_continuation_failure(&context, error);
                let error = record_stream_failure(
                    selector.as_ref(),
                    &session,
                    error,
                    &upstream_model,
                )
                .await;
                Err(error)?;
                return;
            }
        };
        let completed = final_events
            .iter()
            .flat_map(ProviderEvent::canonical_facts)
            .any(|event| matches!(event, GatewayEvent::Completed(_)));
        // 尾部 finish 补全缓冲中的首个输出帧时，同样上报首字。
        if decoder.take_output_start() && first_token_ms.is_none() {
            first_token_ms = Some(
                u64::try_from(output_started_at.elapsed().as_millis()).unwrap_or(u64::MAX),
            );
            yield ProviderEvent::observation(
                observation
                    .clone()
                    .with_timings(ProviderResponseTimings {
                        first_token_ms,
                        ..base_timings
                    }),
            );
        }
        if let Some(tier) = decoder.response_service_tier()
            && observation.service_tier() != Some(tier)
        {
            observation = observation.with_service_tier_if_valid(tier.to_owned());
            yield ProviderEvent::observation(observation.clone().with_timings(
                ProviderResponseTimings { first_token_ms, ..base_timings },
            ));
        }
        attach_xai_session_update(&mut final_events, &mut session_capture)?;
        if completed && session.allows_account_state_mutation() {
            selector.record_success(&session).await;
        }
        for event in final_events {
            ensure_sent_context(&context)?;
            yield event;
        }
    })
}
