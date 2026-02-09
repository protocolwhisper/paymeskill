use axum::{
    Json,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::Utc;
use reqwest::{Client, Method};
use serde_json::Value;
use std::time::Duration;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::types::{
    AppState, Campaign, Metrics, PAYMENT_RESPONSE_HEADER, PAYMENT_SIGNATURE_HEADER, PaymentProof,
    PaymentRequired, PaymentSettlement, PaymentStatus, SPONSORED_API_SERVICE_PREFIX,
    ServiceRunRequest, ServiceRunResponse, SponsoredApi, UserProfile, X402_VERSION_HEADER,
};

pub fn respond<T: IntoResponse>(
    metrics: &Metrics,
    endpoint: &str,
    result: ApiResult<T>,
) -> Response {
    let response = match result {
        Ok(value) => value.into_response(),
        Err(err) => err.into_response(),
    };

    mark_request(metrics, endpoint, response.status());
    response
}

pub fn user_matches_campaign(user: &UserProfile, campaign: &Campaign) -> bool {
    let role_match = if campaign.target_roles.is_empty() {
        true
    } else {
        user.roles
            .iter()
            .any(|role| campaign.target_roles.iter().any(|target| target == role))
    };

    let tool_match = if campaign.target_tools.is_empty() {
        true
    } else {
        user.tools_used
            .iter()
            .any(|tool| campaign.target_tools.iter().any(|target| target == tool))
    };

    role_match && tool_match
}

pub fn has_completed_task(
    state: &AppState,
    campaign_id: Uuid,
    user_id: Uuid,
    required_task: &str,
) -> bool {
    state.task_completions.iter().any(|completion| {
        completion.campaign_id == campaign_id
            && completion.user_id == user_id
            && completion.task_name == required_task
    })
}

pub fn verify_payment_proof(
    state: &AppState,
    service: &str,
    price: u64,
    headers: &HeaderMap,
) -> ApiResult<PaymentProof> {
    let Some(signature) = headers
        .get(PAYMENT_SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(ApiError::PaymentRequired(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: "missing payment proof".to_string(),
            next_step: "call /payments/mock/direct first, then retry with payment-signature header"
                .to_string(),
        }));
    };

    let proof = decode_payment_proof(signature).map_err(|err| {
        ApiError::PaymentRequired(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: format!("invalid payment proof: {err}"),
            next_step: "regenerate payment signature via /payments/mock/direct".to_string(),
        })
    })?;

    if proof.service != service {
        return Err(ApiError::PaymentRequired(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: "payment proof service mismatch".to_string(),
            next_step: "create a payment proof for this specific service".to_string(),
        }));
    }

    if proof.amount_cents < price {
        return Err(ApiError::PaymentRequired(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: format!(
                "insufficient amount in proof: {} < {}",
                proof.amount_cents, price
            ),
            next_step: "create a payment proof with an amount >= service price".to_string(),
        }));
    }

    let payment = state.payments.get(&proof.tx_hash);
    match payment {
        Some(payment) if payment.status == PaymentStatus::Settled => Ok(proof),
        Some(_) => Err(ApiError::PaymentRequired(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: "payment exists but is not settled".to_string(),
            next_step: "wait for settlement or ingest a settled webhook from x402scan".to_string(),
        })),
        None => Err(ApiError::PaymentRequired(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: "payment tx hash not found in ledger".to_string(),
            next_step:
                "register payment via /payments/mock/direct or /webhooks/x402scan/settlement"
                    .to_string(),
        })),
    }
}

pub fn build_paid_tool_response(
    service: String,
    request: ServiceRunRequest,
    proof: &PaymentProof,
    sponsored_by: Option<String>,
) -> Response {
    let payment_mode = payment_mode_from_proof(proof).to_string();

    let payload = ServiceRunResponse {
        service: service.clone(),
        output: format!(
            "Executed '{}' task for user {} with input: {}",
            service, request.user_id, request.input
        ),
        payment_mode,
        sponsored_by,
        tx_hash: Some(proof.tx_hash.clone()),
    };

    let mut response = (StatusCode::OK, Json(payload)).into_response();
    attach_payment_headers(&mut response, proof);
    response
}

pub fn attach_payment_headers(response: &mut Response, proof: &PaymentProof) {
    response.headers_mut().insert(
        HeaderName::from_static(X402_VERSION_HEADER),
        HeaderValue::from_static("2"),
    );

    let settlement = PaymentSettlement {
        tx_hash: proof.tx_hash.clone(),
        status: PaymentStatus::Settled,
        settled_at: Utc::now(),
    };

    let settlement_encoded = STANDARD.encode(
        serde_json::to_vec(&settlement).expect("payment settlement response should serialize"),
    );

    if let Ok(header_value) = HeaderValue::from_str(&settlement_encoded) {
        response.headers_mut().insert(
            HeaderName::from_static(PAYMENT_RESPONSE_HEADER),
            header_value,
        );
    }
}

pub fn payment_mode_from_proof(proof: &PaymentProof) -> &'static str {
    if proof.sponsored_campaign_id.is_some() {
        "sponsored"
    } else {
        "user_direct"
    }
}

pub fn encode_payment_proof(proof: &PaymentProof) -> String {
    let serialized = serde_json::to_vec(proof).expect("payment proof should serialize");
    STANDARD.encode(serialized)
}

fn decode_payment_proof(encoded: &str) -> Result<PaymentProof, String> {
    let raw = STANDARD.decode(encoded).map_err(|err| err.to_string())?;
    serde_json::from_slice::<PaymentProof>(&raw).map_err(|err| err.to_string())
}

pub fn mark_request(metrics: &Metrics, endpoint: &str, status: StatusCode) {
    let status_label = status.as_u16().to_string();
    metrics
        .http_requests_total
        .with_label_values(&[endpoint, status_label.as_str()])
        .inc();
}

pub fn sponsored_api_service_key(api_id: Uuid) -> String {
    format!("{}-{}", SPONSORED_API_SERVICE_PREFIX, api_id)
}

pub fn normalize_upstream_method(method: Option<String>) -> ApiResult<String> {
    let value = method.unwrap_or_else(|| "POST".to_string());
    let normalized = value.trim().to_uppercase();
    match normalized.as_str() {
        "GET" | "POST" => Ok(normalized),
        _ => Err(ApiError::validation("upstream_method must be GET or POST")),
    }
}

pub async fn call_upstream(
    http: &Client,
    api: &SponsoredApi,
    payload: Value,
    timeout_secs: u64,
) -> ApiResult<(u16, String)> {
    let method = match api.upstream_method.as_str() {
        "GET" => Method::GET,
        "POST" => Method::POST,
        other => {
            return Err(ApiError::internal(format!(
                "unsupported upstream method: {other}"
            )));
        }
    };

    let mut request = http
        .request(method.clone(), &api.upstream_url)
        .timeout(Duration::from_secs(timeout_secs));

    for (header, value) in &api.upstream_headers {
        request = request.header(header, value);
    }

    if matches!(method, Method::GET) {
        if let Some(params) = payload.as_object() {
            request = request.query(params);
        }
    } else {
        request = request.json(&payload);
    }

    let response = request
        .send()
        .await
        .map_err(|err| ApiError::upstream(StatusCode::BAD_GATEWAY, err.to_string()))?;

    let status = response.status().as_u16();
    let body = response.text().await.unwrap_or_default();
    Ok((status, body))
}
