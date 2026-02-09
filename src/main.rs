use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{DateTime, Utc};
use prometheus::{Encoder, IntCounter, IntCounterVec, Opts, Registry, TextEncoder};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

const PAYMENT_SIGNATURE_HEADER: &str = "payment-signature";
const PAYMENT_RESPONSE_HEADER: &str = "payment-response";
const X402_VERSION_HEADER: &str = "x402-version";
const DEFAULT_PRICE_CENTS: u64 = 5;

#[derive(Clone)]
struct SharedState {
    inner: Arc<RwLock<AppState>>,
}

struct AppState {
    users: HashMap<Uuid, UserProfile>,
    campaigns: HashMap<Uuid, Campaign>,
    task_completions: Vec<TaskCompletion>,
    payments: HashMap<String, PaymentRecord>,
    creator_events: Vec<CreatorEvent>,
    service_prices: HashMap<String, u64>,
    metrics: Metrics,
}

#[derive(Clone)]
struct Metrics {
    registry: Registry,
    http_requests_total: IntCounterVec,
    payment_events_total: IntCounterVec,
    creator_events_total: IntCounterVec,
    sponsor_spend_cents_total: IntCounter,
}

impl Metrics {
    fn new() -> Self {
        let registry = Registry::new();

        let http_requests_total = IntCounterVec::new(
            Opts::new("http_requests_total", "Total HTTP requests"),
            &["endpoint", "status"],
        )
        .expect("http counter vec should build");

        let payment_events_total = IntCounterVec::new(
            Opts::new("payment_events_total", "Payment events"),
            &["mode", "status"],
        )
        .expect("payment counter vec should build");

        let creator_events_total = IntCounterVec::new(
            Opts::new("creator_events_total", "Creator skill metric events"),
            &["skill", "platform", "event_type"],
        )
        .expect("creator counter vec should build");

        let sponsor_spend_cents_total = IntCounter::new(
            "sponsor_spend_cents_total",
            "Total sponsored spend in cents",
        )
        .expect("sponsor counter should build");

        registry
            .register(Box::new(http_requests_total.clone()))
            .expect("register http counter vec");
        registry
            .register(Box::new(payment_events_total.clone()))
            .expect("register payment counter vec");
        registry
            .register(Box::new(creator_events_total.clone()))
            .expect("register creator counter vec");
        registry
            .register(Box::new(sponsor_spend_cents_total.clone()))
            .expect("register sponsor spend counter");

        Self {
            registry,
            http_requests_total,
            payment_events_total,
            creator_events_total,
            sponsor_spend_cents_total,
        }
    }
}

impl AppState {
    fn new() -> Self {
        let mut service_prices = HashMap::new();
        service_prices.insert("scraping".to_string(), 5);
        service_prices.insert("design".to_string(), 8);
        service_prices.insert("storage".to_string(), 3);
        service_prices.insert("data-tooling".to_string(), 4);

        Self {
            users: HashMap::new(),
            campaigns: HashMap::new(),
            task_completions: Vec::new(),
            payments: HashMap::new(),
            creator_events: Vec::new(),
            service_prices,
            metrics: Metrics::new(),
        }
    }

    fn service_price(&self, service: &str) -> u64 {
        self.service_prices
            .get(service)
            .copied()
            .unwrap_or(DEFAULT_PRICE_CENTS)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct UserProfile {
    id: Uuid,
    email: String,
    region: String,
    roles: Vec<String>,
    tools_used: Vec<String>,
    attributes: HashMap<String, String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct CreateUserRequest {
    email: String,
    region: String,
    roles: Vec<String>,
    tools_used: Vec<String>,
    #[serde(default)]
    attributes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Campaign {
    id: Uuid,
    name: String,
    sponsor: String,
    target_roles: Vec<String>,
    target_tools: Vec<String>,
    required_task: String,
    subsidy_per_call_cents: u64,
    budget_remaining_cents: u64,
    active: bool,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct CreateCampaignRequest {
    name: String,
    sponsor: String,
    #[serde(default)]
    target_roles: Vec<String>,
    #[serde(default)]
    target_tools: Vec<String>,
    required_task: String,
    subsidy_per_call_cents: u64,
    budget_cents: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskCompletion {
    id: Uuid,
    campaign_id: Uuid,
    user_id: Uuid,
    task_name: String,
    details: Option<String>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct TaskCompletionRequest {
    campaign_id: Uuid,
    user_id: Uuid,
    task_name: String,
    details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ServiceRunRequest {
    user_id: Uuid,
    input: String,
}

#[derive(Debug, Clone, Serialize)]
struct ServiceRunResponse {
    service: String,
    output: String,
    payment_mode: String,
    sponsored_by: Option<String>,
    tx_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct PaymentRequired {
    service: String,
    amount_cents: u64,
    accepted_header: String,
    message: String,
    next_step: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaymentProof {
    tx_hash: String,
    service: String,
    amount_cents: u64,
    payer: String,
    sponsored_campaign_id: Option<Uuid>,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PaymentSource {
    User,
    Sponsor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum PaymentStatus {
    Settled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PaymentRecord {
    tx_hash: String,
    campaign_id: Option<Uuid>,
    service: String,
    amount_cents: u64,
    payer: String,
    source: PaymentSource,
    status: PaymentStatus,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct PaymentSettlement {
    tx_hash: String,
    status: PaymentStatus,
    settled_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct CreateDirectPaymentRequest {
    payer: String,
    service: String,
    amount_cents: Option<u64>,
}

#[derive(Debug, Serialize)]
struct DirectPaymentResponse {
    tx_hash: String,
    payment_signature: String,
}

#[derive(Debug, Deserialize)]
struct X402ScanSettlementRequest {
    tx_hash: String,
    service: String,
    amount_cents: u64,
    payer: String,
    source: PaymentSource,
    status: PaymentStatus,
    campaign_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CreatorEvent {
    id: Uuid,
    skill_name: String,
    platform: String,
    event_type: String,
    duration_ms: Option<u64>,
    success: bool,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
struct CreatorMetricEventRequest {
    skill_name: String,
    platform: String,
    event_type: String,
    duration_ms: Option<u64>,
    success: bool,
}

#[derive(Debug, Serialize)]
struct CreatorMetricSummary {
    total_events: usize,
    success_events: usize,
    success_rate: f64,
    per_skill: Vec<SkillMetrics>,
}

#[derive(Debug, Serialize)]
struct SkillMetrics {
    skill_name: String,
    total_events: usize,
    success_events: usize,
    avg_duration_ms: Option<f64>,
    last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct SponsorDashboard {
    campaign: Campaign,
    tasks_completed: usize,
    sponsored_calls: usize,
    spend_cents: u64,
    remaining_budget_cents: u64,
}

#[derive(Debug, Serialize)]
struct MessageResponse {
    message: String,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG")
                .unwrap_or_else(|_| "payloadexchange_mvp=info,tower_http=info".to_string()),
        )
        .with_target(false)
        .compact()
        .init();

    let state = SharedState {
        inner: Arc::new(RwLock::new(AppState::new())),
    };

    let app = Router::new()
        .route("/health", get(health))
        .route("/profiles", post(create_profile).get(list_profiles))
        .route("/campaigns", post(create_campaign).get(list_campaigns))
        .route("/tasks/complete", post(complete_task))
        .route("/tool/:service/run", post(run_tool))
        .route("/proxy/:service/run", post(run_proxy))
        .route("/payments/mock/direct", post(mock_direct_payment))
        .route(
            "/webhooks/x402scan/settlement",
            post(ingest_x402scan_settlement),
        )
        .route("/dashboard/sponsor/:campaign_id", get(sponsor_dashboard))
        .route("/creator/metrics/event", post(record_creator_metric_event))
        .route("/creator/metrics", get(creator_metrics))
        .route("/metrics", get(prometheus_metrics))
        .with_state(state);

    let port = std::env::var("PORT")
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(3000);
    let address = SocketAddr::from(([0, 0, 0, 0], port));

    info!("payloadexchange-mvp listening on http://{}", address);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .expect("bind should succeed");

    if let Err(err) = axum::serve(listener, app).await {
        eprintln!("server error: {err}");
    }
}

async fn health(State(state): State<SharedState>) -> impl IntoResponse {
    let state = state.inner.read().await;
    mark_request(&state.metrics, "/health", StatusCode::OK);
    (
        StatusCode::OK,
        Json(MessageResponse {
            message: "ok".to_string(),
        }),
    )
}

async fn create_profile(
    State(state): State<SharedState>,
    Json(payload): Json<CreateUserRequest>,
) -> impl IntoResponse {
    let mut state = state.inner.write().await;
    let profile = UserProfile {
        id: Uuid::new_v4(),
        email: payload.email,
        region: payload.region,
        roles: payload.roles,
        tools_used: payload.tools_used,
        attributes: payload.attributes,
        created_at: Utc::now(),
    };

    state.users.insert(profile.id, profile.clone());
    mark_request(&state.metrics, "/profiles", StatusCode::CREATED);
    (StatusCode::CREATED, Json(profile))
}

async fn list_profiles(State(state): State<SharedState>) -> impl IntoResponse {
    let state = state.inner.read().await;
    let mut profiles: Vec<UserProfile> = state.users.values().cloned().collect();
    profiles.sort_by_key(|profile| profile.created_at);
    mark_request(&state.metrics, "/profiles", StatusCode::OK);
    (StatusCode::OK, Json(profiles))
}

async fn create_campaign(
    State(state): State<SharedState>,
    Json(payload): Json<CreateCampaignRequest>,
) -> impl IntoResponse {
    let mut state = state.inner.write().await;

    let campaign = Campaign {
        id: Uuid::new_v4(),
        name: payload.name,
        sponsor: payload.sponsor,
        target_roles: payload.target_roles,
        target_tools: payload.target_tools,
        required_task: payload.required_task,
        subsidy_per_call_cents: payload.subsidy_per_call_cents,
        budget_remaining_cents: payload.budget_cents,
        active: true,
        created_at: Utc::now(),
    };

    state.campaigns.insert(campaign.id, campaign.clone());
    mark_request(&state.metrics, "/campaigns", StatusCode::CREATED);
    (StatusCode::CREATED, Json(campaign))
}

async fn list_campaigns(State(state): State<SharedState>) -> impl IntoResponse {
    let state = state.inner.read().await;
    let mut campaigns: Vec<Campaign> = state.campaigns.values().cloned().collect();
    campaigns.sort_by_key(|campaign| campaign.created_at);
    mark_request(&state.metrics, "/campaigns", StatusCode::OK);
    (StatusCode::OK, Json(campaigns))
}

async fn complete_task(
    State(state): State<SharedState>,
    Json(payload): Json<TaskCompletionRequest>,
) -> impl IntoResponse {
    let mut state = state.inner.write().await;

    if !state.campaigns.contains_key(&payload.campaign_id) {
        mark_request(&state.metrics, "/tasks/complete", StatusCode::NOT_FOUND);
        return (
            StatusCode::NOT_FOUND,
            Json(MessageResponse {
                message: "campaign not found".to_string(),
            }),
        )
            .into_response();
    }

    if !state.users.contains_key(&payload.user_id) {
        mark_request(&state.metrics, "/tasks/complete", StatusCode::NOT_FOUND);
        return (
            StatusCode::NOT_FOUND,
            Json(MessageResponse {
                message: "user not found".to_string(),
            }),
        )
            .into_response();
    }

    let completion = TaskCompletion {
        id: Uuid::new_v4(),
        campaign_id: payload.campaign_id,
        user_id: payload.user_id,
        task_name: payload.task_name,
        details: payload.details,
        created_at: Utc::now(),
    };

    state.task_completions.push(completion.clone());
    mark_request(&state.metrics, "/tasks/complete", StatusCode::CREATED);
    (StatusCode::CREATED, Json(completion)).into_response()
}

async fn run_tool(
    State(state): State<SharedState>,
    Path(service): Path<String>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<ServiceRunRequest>,
) -> Response {
    let state = state.inner.write().await;

    let price = state.service_price(&service);
    let response = match verify_payment_proof(&state, &service, price, &headers) {
        Ok(proof) => {
            state
                .metrics
                .payment_events_total
                .with_label_values(&[payment_mode_from_proof(&proof), "settled"])
                .inc();

            build_paid_tool_response(
                service,
                payload,
                &proof,
                proof
                    .sponsored_campaign_id
                    .and_then(|id| state.campaigns.get(&id).map(|c| c.sponsor.clone())),
            )
        }
        Err(requirements) => payment_required_response(requirements),
    };

    mark_request(&state.metrics, "/tool/:service/run", response.status());
    response
}

async fn run_proxy(
    State(state): State<SharedState>,
    Path(service): Path<String>,
    headers: axum::http::HeaderMap,
    Json(payload): Json<ServiceRunRequest>,
) -> Response {
    let mut state = state.inner.write().await;

    if !state.users.contains_key(&payload.user_id) {
        let status = StatusCode::NOT_FOUND;
        mark_request(&state.metrics, "/proxy/:service/run", status);
        return (
            status,
            Json(MessageResponse {
                message: "user profile is required before proxy usage".to_string(),
            }),
        )
            .into_response();
    }

    let price = state.service_price(&service);
    let has_header = headers.contains_key(PAYMENT_SIGNATURE_HEADER);

    if has_header {
        let response = match verify_payment_proof(&state, &service, price, &headers) {
            Ok(proof) => {
                state
                    .metrics
                    .payment_events_total
                    .with_label_values(&[payment_mode_from_proof(&proof), "settled"])
                    .inc();
                build_paid_tool_response(service, payload, &proof, None)
            }
            Err(requirements) => payment_required_response(requirements),
        };

        mark_request(&state.metrics, "/proxy/:service/run", response.status());
        return response;
    }

    let user = match state.users.get(&payload.user_id) {
        Some(user) => user,
        None => {
            let status = StatusCode::NOT_FOUND;
            mark_request(&state.metrics, "/proxy/:service/run", status);
            return (
                status,
                Json(MessageResponse {
                    message: "user profile is required before proxy usage".to_string(),
                }),
            )
                .into_response();
        }
    };

    let mut match_without_task: Option<Campaign> = None;
    let mut match_with_task: Option<Campaign> = None;

    let campaigns: Vec<Campaign> = state.campaigns.values().cloned().collect();
    for campaign in campaigns {
        if !campaign.active || campaign.budget_remaining_cents < price {
            continue;
        }

        if !user_matches_campaign(user, &campaign) {
            continue;
        }

        if has_completed_task(
            &state,
            campaign.id,
            payload.user_id,
            &campaign.required_task,
        ) {
            match_with_task = Some(campaign);
            break;
        }

        if match_without_task.is_none() {
            match_without_task = Some(campaign);
        }
    }

    if let Some(campaign) = match_with_task {
        if let Some(persisted) = state.campaigns.get_mut(&campaign.id) {
            persisted.budget_remaining_cents =
                persisted.budget_remaining_cents.saturating_sub(price);
            if persisted.budget_remaining_cents == 0 {
                persisted.active = false;
            }
        }

        let tx_hash = format!("sponsor-{}", Uuid::new_v4());
        let proof = PaymentProof {
            tx_hash: tx_hash.clone(),
            service: service.clone(),
            amount_cents: price,
            payer: campaign.sponsor.clone(),
            sponsored_campaign_id: Some(campaign.id),
            created_at: Utc::now(),
        };

        state.payments.insert(
            tx_hash.clone(),
            PaymentRecord {
                tx_hash: tx_hash.clone(),
                campaign_id: Some(campaign.id),
                service: service.clone(),
                amount_cents: price,
                payer: campaign.sponsor.clone(),
                source: PaymentSource::Sponsor,
                status: PaymentStatus::Settled,
                created_at: Utc::now(),
            },
        );

        state
            .metrics
            .payment_events_total
            .with_label_values(&["sponsored", "settled"])
            .inc();
        state.metrics.sponsor_spend_cents_total.inc_by(price);

        let response = build_paid_tool_response(service, payload, &proof, Some(campaign.sponsor));
        mark_request(&state.metrics, "/proxy/:service/run", response.status());
        return response;
    }

    if let Some(campaign) = match_without_task {
        let status = StatusCode::PRECONDITION_REQUIRED;
        mark_request(&state.metrics, "/proxy/:service/run", status);
        return (
            status,
            Json(MessageResponse {
                message: format!(
                    "complete sponsor task '{}' for campaign '{}' before sponsored usage",
                    campaign.required_task, campaign.name
                ),
            }),
        )
            .into_response();
    }

    let response = payment_required_response(PaymentRequired {
        service,
        amount_cents: price,
        accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
        message: "no eligible sponsor campaign found".to_string(),
        next_step:
            "either complete a sponsored campaign task or pay directly via /payments/mock/direct"
                .to_string(),
    });

    mark_request(&state.metrics, "/proxy/:service/run", response.status());
    response
}

async fn mock_direct_payment(
    State(state): State<SharedState>,
    Json(payload): Json<CreateDirectPaymentRequest>,
) -> impl IntoResponse {
    let mut state = state.inner.write().await;

    let price = state.service_price(&payload.service);
    let amount = payload.amount_cents.unwrap_or(price);
    let tx_hash = format!("user-{}", Uuid::new_v4());

    let proof = PaymentProof {
        tx_hash: tx_hash.clone(),
        service: payload.service.clone(),
        amount_cents: amount,
        payer: payload.payer.clone(),
        sponsored_campaign_id: None,
        created_at: Utc::now(),
    };

    state.payments.insert(
        tx_hash.clone(),
        PaymentRecord {
            tx_hash: tx_hash.clone(),
            campaign_id: None,
            service: payload.service,
            amount_cents: amount,
            payer: payload.payer,
            source: PaymentSource::User,
            status: PaymentStatus::Settled,
            created_at: Utc::now(),
        },
    );

    state
        .metrics
        .payment_events_total
        .with_label_values(&["user_direct", "settled"])
        .inc();

    let signature = encode_payment_proof(&proof);
    mark_request(&state.metrics, "/payments/mock/direct", StatusCode::CREATED);
    (
        StatusCode::CREATED,
        Json(DirectPaymentResponse {
            tx_hash,
            payment_signature: signature,
        }),
    )
}

async fn ingest_x402scan_settlement(
    State(state): State<SharedState>,
    Json(payload): Json<X402ScanSettlementRequest>,
) -> impl IntoResponse {
    let mut state = state.inner.write().await;

    state.payments.insert(
        payload.tx_hash.clone(),
        PaymentRecord {
            tx_hash: payload.tx_hash,
            campaign_id: payload.campaign_id,
            service: payload.service,
            amount_cents: payload.amount_cents,
            payer: payload.payer,
            source: payload.source.clone(),
            status: payload.status.clone(),
            created_at: Utc::now(),
        },
    );

    let mode = match payload.source {
        PaymentSource::User => "user_direct",
        PaymentSource::Sponsor => "sponsored",
    };
    let status = match payload.status {
        PaymentStatus::Settled => "settled",
        PaymentStatus::Failed => "failed",
    };

    state
        .metrics
        .payment_events_total
        .with_label_values(&[mode, status])
        .inc();

    mark_request(
        &state.metrics,
        "/webhooks/x402scan/settlement",
        StatusCode::ACCEPTED,
    );
    (
        StatusCode::ACCEPTED,
        Json(MessageResponse {
            message: "settlement ingested".to_string(),
        }),
    )
}

async fn sponsor_dashboard(
    State(state): State<SharedState>,
    Path(campaign_id): Path<Uuid>,
) -> impl IntoResponse {
    let state = state.inner.read().await;

    let Some(campaign) = state.campaigns.get(&campaign_id).cloned() else {
        mark_request(
            &state.metrics,
            "/dashboard/sponsor/:campaign_id",
            StatusCode::NOT_FOUND,
        );

        return (
            StatusCode::NOT_FOUND,
            Json(MessageResponse {
                message: "campaign not found".to_string(),
            }),
        )
            .into_response();
    };

    let tasks_completed = state
        .task_completions
        .iter()
        .filter(|task| task.campaign_id == campaign_id)
        .count();

    let sponsored_payments: Vec<&PaymentRecord> = state
        .payments
        .values()
        .filter(|record| {
            record.campaign_id == Some(campaign_id)
                && record.source == PaymentSource::Sponsor
                && record.status == PaymentStatus::Settled
        })
        .collect();

    let sponsored_calls = sponsored_payments.len();
    let spend_cents: u64 = sponsored_payments
        .iter()
        .map(|record| record.amount_cents)
        .sum();

    let response = SponsorDashboard {
        remaining_budget_cents: campaign.budget_remaining_cents,
        campaign,
        tasks_completed,
        sponsored_calls,
        spend_cents,
    };

    mark_request(
        &state.metrics,
        "/dashboard/sponsor/:campaign_id",
        StatusCode::OK,
    );
    (StatusCode::OK, Json(response)).into_response()
}

async fn record_creator_metric_event(
    State(state): State<SharedState>,
    Json(payload): Json<CreatorMetricEventRequest>,
) -> impl IntoResponse {
    let mut state = state.inner.write().await;

    let event = CreatorEvent {
        id: Uuid::new_v4(),
        skill_name: payload.skill_name,
        platform: payload.platform,
        event_type: payload.event_type,
        duration_ms: payload.duration_ms,
        success: payload.success,
        created_at: Utc::now(),
    };

    state
        .metrics
        .creator_events_total
        .with_label_values(&[&event.skill_name, &event.platform, &event.event_type])
        .inc();

    state.creator_events.push(event.clone());

    mark_request(
        &state.metrics,
        "/creator/metrics/event",
        StatusCode::CREATED,
    );
    (StatusCode::CREATED, Json(event))
}

async fn creator_metrics(State(state): State<SharedState>) -> impl IntoResponse {
    let state = state.inner.read().await;

    let total_events = state.creator_events.len();
    let success_events = state
        .creator_events
        .iter()
        .filter(|event| event.success)
        .count();
    let success_rate = if total_events == 0 {
        0.0
    } else {
        success_events as f64 / total_events as f64
    };

    let mut per_skill_map: HashMap<String, Vec<&CreatorEvent>> = HashMap::new();
    for event in &state.creator_events {
        per_skill_map
            .entry(event.skill_name.clone())
            .or_default()
            .push(event);
    }

    let mut per_skill: Vec<SkillMetrics> = per_skill_map
        .into_iter()
        .map(|(skill_name, events)| {
            let total = events.len();
            let success = events.iter().filter(|event| event.success).count();

            let duration_values: Vec<u64> = events
                .iter()
                .filter_map(|event| event.duration_ms)
                .collect();

            let avg_duration_ms = if duration_values.is_empty() {
                None
            } else {
                let sum: u64 = duration_values.iter().sum();
                Some(sum as f64 / duration_values.len() as f64)
            };

            let last_seen_at = events
                .iter()
                .map(|event| event.created_at)
                .max()
                .unwrap_or_else(Utc::now);

            SkillMetrics {
                skill_name,
                total_events: total,
                success_events: success,
                avg_duration_ms,
                last_seen_at,
            }
        })
        .collect();

    per_skill.sort_by(|left, right| {
        right
            .total_events
            .cmp(&left.total_events)
            .then_with(|| right.last_seen_at.cmp(&left.last_seen_at))
    });

    mark_request(&state.metrics, "/creator/metrics", StatusCode::OK);
    (
        StatusCode::OK,
        Json(CreatorMetricSummary {
            total_events,
            success_events,
            success_rate,
            per_skill,
        }),
    )
}

async fn prometheus_metrics(State(state): State<SharedState>) -> impl IntoResponse {
    let state = state.inner.read().await;
    let metric_families = state.metrics.registry.gather();
    let mut buffer = Vec::new();
    let encoder = TextEncoder::new();

    let status = match encoder.encode(&metric_families, &mut buffer) {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    };
    let content_type = encoder.format_type().to_string();

    mark_request(&state.metrics, "/metrics", status);

    (
        status,
        [("content-type", content_type)],
        String::from_utf8_lossy(&buffer).to_string(),
    )
}

fn user_matches_campaign(user: &UserProfile, campaign: &Campaign) -> bool {
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

fn has_completed_task(
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

fn verify_payment_proof(
    state: &AppState,
    service: &str,
    price: u64,
    headers: &axum::http::HeaderMap,
) -> Result<PaymentProof, PaymentRequired> {
    let Some(signature) = headers
        .get(PAYMENT_SIGNATURE_HEADER)
        .and_then(|value| value.to_str().ok())
    else {
        return Err(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: "missing payment proof".to_string(),
            next_step: "call /payments/mock/direct first, then retry with payment-signature header"
                .to_string(),
        });
    };

    let proof = match decode_payment_proof(signature) {
        Ok(proof) => proof,
        Err(err) => {
            return Err(PaymentRequired {
                service: service.to_string(),
                amount_cents: price,
                accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
                message: format!("invalid payment proof: {err}"),
                next_step: "regenerate payment signature via /payments/mock/direct".to_string(),
            });
        }
    };

    if proof.service != service {
        return Err(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: "payment proof service mismatch".to_string(),
            next_step: "create a payment proof for this specific service".to_string(),
        });
    }

    if proof.amount_cents < price {
        return Err(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: format!(
                "insufficient amount in proof: {} < {}",
                proof.amount_cents, price
            ),
            next_step: "create a payment proof with an amount >= service price".to_string(),
        });
    }

    let payment = state.payments.get(&proof.tx_hash);
    match payment {
        Some(payment) if payment.status == PaymentStatus::Settled => Ok(proof),
        Some(_) => Err(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: "payment exists but is not settled".to_string(),
            next_step: "wait for settlement or ingest a settled webhook from x402scan".to_string(),
        }),
        None => Err(PaymentRequired {
            service: service.to_string(),
            amount_cents: price,
            accepted_header: PAYMENT_SIGNATURE_HEADER.to_string(),
            message: "payment tx hash not found in ledger".to_string(),
            next_step:
                "register payment via /payments/mock/direct or /webhooks/x402scan/settlement"
                    .to_string(),
        }),
    }
}

fn payment_required_response(payload: PaymentRequired) -> Response {
    let mut response = (StatusCode::PAYMENT_REQUIRED, Json(payload)).into_response();
    response.headers_mut().insert(
        HeaderName::from_static(X402_VERSION_HEADER),
        HeaderValue::from_static("2"),
    );
    response
}

fn build_paid_tool_response(
    service: String,
    request: ServiceRunRequest,
    proof: &PaymentProof,
    sponsored_by: Option<String>,
) -> Response {
    let payment_mode = payment_mode_from_proof(proof).to_string();
    let settlement = PaymentSettlement {
        tx_hash: proof.tx_hash.clone(),
        status: PaymentStatus::Settled,
        settled_at: Utc::now(),
    };

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
    response.headers_mut().insert(
        HeaderName::from_static(X402_VERSION_HEADER),
        HeaderValue::from_static("2"),
    );

    let settlement_encoded = STANDARD.encode(
        serde_json::to_vec(&settlement).expect("payment settlement response should serialize"),
    );

    if let Ok(header_value) = HeaderValue::from_str(&settlement_encoded) {
        response.headers_mut().insert(
            HeaderName::from_static(PAYMENT_RESPONSE_HEADER),
            header_value,
        );
    }

    response
}

fn payment_mode_from_proof(proof: &PaymentProof) -> &'static str {
    if proof.sponsored_campaign_id.is_some() {
        "sponsored"
    } else {
        "user_direct"
    }
}

fn encode_payment_proof(proof: &PaymentProof) -> String {
    let serialized = serde_json::to_vec(proof).expect("payment proof should serialize");
    STANDARD.encode(serialized)
}

fn decode_payment_proof(encoded: &str) -> Result<PaymentProof, String> {
    let raw = STANDARD.decode(encoded).map_err(|err| err.to_string())?;
    serde_json::from_slice::<PaymentProof>(&raw).map_err(|err| err.to_string())
}

fn mark_request(metrics: &Metrics, endpoint: &str, status: StatusCode) {
    let status_label = status.as_u16().to_string();
    metrics
        .http_requests_total
        .with_label_values(&[endpoint, status_label.as_str()])
        .inc();
}
