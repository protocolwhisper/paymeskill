use axum::{
    Json,
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use reqwest::{Client, Method};
use serde_json::Value;
use std::{collections::HashMap, time::Duration};
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::onchain::{VerifiedX402Payment, verify_and_settle_x402_payment};
use crate::types::{
    AppConfig, Campaign, Metrics, PAYMENT_RESPONSE_HEADER, PAYMENT_SIGNATURE_HEADER,
    PaymentRequired, SPONSORED_API_SERVICE_PREFIX, ServiceRunRequest, ServiceRunResponse,
    SponsoredApi, UserProfile, X402_VERSION_HEADER, X402PaymentRequirement,
};
use sqlx::PgPool;

const USDC_BASE_UNITS_PER_CENT: u128 = 10_000;

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

pub async fn has_completed_task(
    db: &PgPool,
    campaign_id: Uuid,
    user_id: Uuid,
    required_task: &str,
) -> ApiResult<bool> {
    let exists = sqlx::query_scalar::<_, bool>(
        r#"
        select exists(
            select 1 from task_completions
            where campaign_id = $1
              and user_id = $2
              and task_name = $3
        )
        "#,
    )
    .bind(campaign_id)
    .bind(user_id)
    .bind(required_task)
    .fetch_one(db)
    .await
    .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    Ok(exists)
}

pub async fn verify_x402_payment(
    http: &Client,
    config: &AppConfig,
    service: &str,
    amount_cents: u64,
    resource_path: &str,
    headers: &HeaderMap,
) -> ApiResult<VerifiedX402Payment> {
    let Some(signature) = headers
        .get(PAYMENT_SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(payment_required_error(
            config,
            service,
            amount_cents,
            resource_path,
            "missing PAYMENT-SIGNATURE header",
            "create a payment from the PAYMENT-REQUIRED challenge and retry",
        ));
    };

    let requirement = build_payment_requirement(config, service, amount_cents, resource_path)?;
    match verify_and_settle_x402_payment(http, config, signature, &requirement).await {
        Ok(payment) => Ok(payment),
        Err(err) => match err {
            ApiError::Config { .. } => Err(err),
            _ => Err(payment_required_error(
                config,
                service,
                amount_cents,
                resource_path,
                format!("payment rejected: {err}"),
                "regenerate PAYMENT-SIGNATURE from the latest challenge and retry",
            )),
        },
    }
}

pub fn payment_required_error(
    config: &AppConfig,
    service: &str,
    amount_cents: u64,
    resource_path: &str,
    message: impl Into<String>,
    next_step: impl Into<String>,
) -> ApiError {
    let requirement = match build_payment_requirement(config, service, amount_cents, resource_path)
    {
        Ok(value) => value,
        Err(err) => return err,
    };

    let payment_required = match encode_payment_required_header(&requirement) {
        Ok(value) => value,
        Err(err) => return ApiError::internal(err),
    };

    ApiError::PaymentRequired(PaymentRequired {
        service: service.to_string(),
        amount_cents,
        accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
        payment_required,
        message: message.into(),
        next_step: next_step.into(),
    })
}

fn build_payment_requirement(
    config: &AppConfig,
    service: &str,
    amount_cents: u64,
    resource_path: &str,
) -> ApiResult<X402PaymentRequirement> {
    let pay_to = required_non_empty_env_like(config.x402_pay_to.as_deref(), "X402_PAY_TO")?;
    let asset = required_non_empty_env_like(config.x402_asset.as_deref(), "X402_ASSET")?;

    let resource = format!(
        "{}{}",
        config.public_base_url.trim_end_matches('/'),
        resource_path
    );

    Ok(X402PaymentRequirement {
        scheme: "exact".to_string(),
        network: config.x402_network.clone(),
        max_amount_required: amount_to_base_units(amount_cents),
        resource,
        description: format!("Access paid service '{service}'"),
        mime_type: "application/json".to_string(),
        pay_to,
        max_timeout_seconds: 300,
        asset,
        output_schema: None,
        extra: HashMap::new(),
    })
}

fn required_non_empty_env_like(value: Option<&str>, key: &str) -> ApiResult<String> {
    let Some(raw) = value else {
        return Err(ApiError::config(format!("{key} is required for x402")));
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(ApiError::config(format!("{key} is required for x402")));
    }
    Ok(trimmed.to_string())
}

fn amount_to_base_units(amount_cents: u64) -> String {
    (u128::from(amount_cents) * USDC_BASE_UNITS_PER_CENT).to_string()
}

fn encode_payment_required_header(requirement: &X402PaymentRequirement) -> Result<String, String> {
    let bytes = serde_json::to_vec(&vec![requirement]).map_err(|err| err.to_string())?;
    Ok(STANDARD.encode(bytes))
}

pub fn build_paid_tool_response(
    service: String,
    request: ServiceRunRequest,
    payment_mode: String,
    sponsored_by: Option<String>,
    tx_hash: Option<String>,
    payment_response_header: Option<&str>,
) -> Response {
    let payload = ServiceRunResponse {
        service: service.clone(),
        output: format!(
            "Executed '{}' task for user {} with input: {}",
            service, request.user_id, request.input
        ),
        payment_mode,
        sponsored_by,
        tx_hash,
    };

    let mut response = (StatusCode::OK, Json(payload)).into_response();
    response.headers_mut().insert(
        HeaderName::from_static(X402_VERSION_HEADER),
        HeaderValue::from_static("2"),
    );

    if let Some(payment_response) = payment_response_header {
        if let Ok(header_value) = HeaderValue::from_str(payment_response) {
            response.headers_mut().insert(
                HeaderName::from_static(PAYMENT_RESPONSE_HEADER),
                header_value,
            );
        }
    }

    response
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
