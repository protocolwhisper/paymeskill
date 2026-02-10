use chrono::{DateTime, Utc};
use prometheus::{IntCounter, IntCounterVec, Opts, Registry};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{PgPool, postgres::PgPoolOptions};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use uuid::Uuid;

pub const PAYMENT_SIGNATURE_HEADER: &str = "payment-signature";
pub const PAYMENT_REQUIRED_HEADER: &str = "payment-required";
pub const PAYMENT_RESPONSE_HEADER: &str = "payment-response";
pub const X402_VERSION_HEADER: &str = "x402-version";
pub const DEFAULT_PRICE_CENTS: u64 = 5;
pub const SPONSORED_API_CREATE_SERVICE: &str = "sponsored-api-create";
pub const SPONSORED_API_SERVICE_PREFIX: &str = "sponsored-api";
pub const DEFAULT_SPONSORED_API_CREATE_PRICE_CENTS: u64 = 25;
pub const DEFAULT_SPONSORED_API_TIMEOUT_SECS: u64 = 12;
pub const DEFAULT_X402_FACILITATOR_URL: &str = "https://x402.org/facilitator";
pub const DEFAULT_X402_VERIFY_PATH: &str = "/verify";
pub const DEFAULT_X402_SETTLE_PATH: &str = "/settle";
pub const DEFAULT_X402_NETWORK: &str = "base-sepolia";
pub const DEFAULT_PUBLIC_BASE_URL: &str = "http://localhost:3000";

#[derive(Clone)]
pub struct AppConfig {
    pub sponsored_api_create_price_cents: u64,
    pub sponsored_api_timeout_secs: u64,
    pub x402_facilitator_url: String,
    pub x402_verify_path: String,
    pub x402_settle_path: String,
    pub x402_facilitator_bearer_token: Option<String>,
    pub x402_network: String,
    pub x402_pay_to: Option<String>,
    pub x402_asset: Option<String>,
    pub public_base_url: String,
}

impl AppConfig {
    pub fn from_env() -> Self {
        Self {
            sponsored_api_create_price_cents: read_env_u64(
                "SPONSORED_API_CREATE_PRICE_CENTS",
                DEFAULT_SPONSORED_API_CREATE_PRICE_CENTS,
            ),
            sponsored_api_timeout_secs: read_env_u64(
                "SPONSORED_API_TIMEOUT_SECS",
                DEFAULT_SPONSORED_API_TIMEOUT_SECS,
            ),
            x402_facilitator_url: std::env::var("X402_FACILITATOR_URL")
                .unwrap_or_else(|_| DEFAULT_X402_FACILITATOR_URL.to_string()),
            x402_verify_path: std::env::var("X402_VERIFY_PATH")
                .unwrap_or_else(|_| DEFAULT_X402_VERIFY_PATH.to_string()),
            x402_settle_path: std::env::var("X402_SETTLE_PATH")
                .unwrap_or_else(|_| DEFAULT_X402_SETTLE_PATH.to_string()),
            x402_facilitator_bearer_token: std::env::var("X402_FACILITATOR_BEARER_TOKEN").ok(),
            x402_network: std::env::var("X402_NETWORK")
                .unwrap_or_else(|_| DEFAULT_X402_NETWORK.to_string()),
            x402_pay_to: std::env::var("X402_PAY_TO").ok(),
            x402_asset: std::env::var("X402_ASSET").ok(),
            public_base_url: std::env::var("PUBLIC_BASE_URL")
                .unwrap_or_else(|_| DEFAULT_PUBLIC_BASE_URL.to_string()),
        }
    }
}

#[derive(Clone)]
pub struct SharedState {
    pub inner: Arc<RwLock<AppState>>,
}

pub struct AppState {
    pub metrics: Metrics,
    pub db: Option<PgPool>,
    pub http: Client,
    pub config: AppConfig,
}

#[derive(Clone)]
pub struct Metrics {
    pub registry: Registry,
    pub http_requests_total: IntCounterVec,
    pub payment_events_total: IntCounterVec,
    pub creator_events_total: IntCounterVec,
    pub sponsor_spend_cents_total: IntCounter,
}

impl Metrics {
    pub fn new() -> Self {
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
    pub fn new() -> Self {
        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("http client should build");

        let config = AppConfig::from_env();
        let db = std::env::var("DATABASE_URL").ok().and_then(|url| {
            PgPoolOptions::new()
                .max_connections(10)
                .connect_lazy(&url)
                .ok()
        });

        Self {
            metrics: Metrics::new(),
            db,
            http,
            config,
        }
    }

    pub fn service_price(&self, service: &str) -> u64 {
        match service {
            "scraping" => 5,
            "design" => 8,
            "storage" => 3,
            "data-tooling" => 4,
            _ => DEFAULT_PRICE_CENTS,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserProfile {
    pub id: Uuid,
    pub email: String,
    pub region: String,
    pub roles: Vec<String>,
    pub tools_used: Vec<String>,
    #[sqlx(json)]
    pub attributes: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub email: String,
    pub region: String,
    pub roles: Vec<String>,
    pub tools_used: Vec<String>,
    #[serde(default)]
    pub attributes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Campaign {
    pub id: Uuid,
    pub name: String,
    pub sponsor: String,
    pub target_roles: Vec<String>,
    pub target_tools: Vec<String>,
    pub required_task: String,
    pub subsidy_per_call_cents: u64,
    pub budget_total_cents: u64,
    pub budget_remaining_cents: u64,
    #[serde(default)]
    pub query_urls: Vec<String>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateCampaignRequest {
    pub name: String,
    pub sponsor: String,
    #[serde(default)]
    pub target_roles: Vec<String>,
    #[serde(default)]
    pub target_tools: Vec<String>,
    pub required_task: String,
    pub subsidy_per_call_cents: u64,
    pub budget_cents: u64,
    #[serde(default)]
    pub query_urls: Vec<String>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CampaignRow {
    pub id: Uuid,
    pub name: String,
    pub sponsor: String,
    pub target_roles: Vec<String>,
    pub target_tools: Vec<String>,
    pub required_task: String,
    pub subsidy_per_call_cents: i64,
    pub budget_total_cents: i64,
    pub budget_remaining_cents: i64,
    pub query_urls: Vec<String>,
    pub active: bool,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<CampaignRow> for Campaign {
    type Error = String;

    fn try_from(value: CampaignRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            name: value.name,
            sponsor: value.sponsor,
            target_roles: value.target_roles,
            target_tools: value.target_tools,
            required_task: value.required_task,
            subsidy_per_call_cents: u64::try_from(value.subsidy_per_call_cents)
                .map_err(|_| "subsidy_per_call_cents must be non-negative".to_string())?,
            budget_total_cents: u64::try_from(value.budget_total_cents)
                .map_err(|_| "budget_total_cents must be non-negative".to_string())?,
            budget_remaining_cents: u64::try_from(value.budget_remaining_cents)
                .map_err(|_| "budget_remaining_cents must be non-negative".to_string())?,
            query_urls: value.query_urls,
            active: value.active,
            created_at: value.created_at,
        })
    }
}

#[derive(Debug, Serialize)]
pub struct CreateCampaignResponse {
    pub campaign: Campaign,
    pub campaign_url: String,
    pub dashboard_url: String,
}

#[derive(Debug, Serialize)]
pub struct CampaignDiscoveryItem {
    pub campaign_id: Uuid,
    pub name: String,
    pub sponsor: String,
    pub active: bool,
    pub query_urls: Vec<String>,
    pub service_run_url: String,
    pub sponsored_api_discovery_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCompletion {
    pub id: Uuid,
    pub campaign_id: Uuid,
    pub user_id: Uuid,
    pub task_name: String,
    pub details: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct TaskCompletionRequest {
    pub campaign_id: Uuid,
    pub user_id: Uuid,
    pub task_name: String,
    pub details: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceRunRequest {
    pub user_id: Uuid,
    pub input: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServiceRunResponse {
    pub service: String,
    pub output: String,
    pub payment_mode: String,
    pub sponsored_by: Option<String>,
    pub tx_hash: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaymentRequired {
    pub service: String,
    pub amount_cents: u64,
    pub accepted_header: String,
    pub payment_required: String,
    pub message: String,
    pub next_step: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaymentSource {
    User,
    Sponsor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    Settled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402PaymentRequirement {
    pub scheme: String,
    pub network: String,
    pub max_amount_required: String,
    pub resource: String,
    pub description: String,
    pub mime_type: String,
    pub pay_to: String,
    pub max_timeout_seconds: u64,
    pub asset: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub extra: HashMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402VerifyResponse {
    pub is_valid: bool,
    #[serde(default)]
    pub invalid_reason: Option<String>,
    #[serde(default)]
    pub payer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct X402SettleResponse {
    pub success: bool,
    #[serde(default)]
    pub transaction: Option<String>,
    #[serde(default)]
    pub payer: Option<String>,
    #[serde(default)]
    pub error_reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct X402ScanSettlementRequest {
    pub tx_hash: String,
    pub service: String,
    pub amount_cents: u64,
    pub payer: String,
    pub source: PaymentSource,
    pub status: PaymentStatus,
    pub campaign_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatorEvent {
    pub id: Uuid,
    pub skill_name: String,
    pub platform: String,
    pub event_type: String,
    pub duration_ms: Option<u64>,
    pub success: bool,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreatorMetricEventRequest {
    pub skill_name: String,
    pub platform: String,
    pub event_type: String,
    pub duration_ms: Option<u64>,
    pub success: bool,
}

#[derive(Debug, Serialize)]
pub struct CreatorMetricSummary {
    pub total_events: usize,
    pub success_events: usize,
    pub success_rate: f64,
    pub per_skill: Vec<SkillMetrics>,
}

#[derive(Debug, Serialize)]
pub struct SkillMetrics {
    pub skill_name: String,
    pub total_events: usize,
    pub success_events: usize,
    pub avg_duration_ms: Option<f64>,
    pub last_seen_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct SponsorDashboard {
    pub campaign: Campaign,
    pub tasks_completed: usize,
    pub sponsored_calls: usize,
    pub spend_cents: u64,
    pub remaining_budget_cents: u64,
}

#[derive(Debug, Serialize)]
pub struct MessageResponse {
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SponsoredApi {
    pub id: Uuid,
    pub name: String,
    pub sponsor: String,
    pub description: Option<String>,
    pub upstream_url: String,
    pub upstream_method: String,
    #[serde(default)]
    pub upstream_headers: HashMap<String, String>,
    pub price_cents: u64,
    pub budget_total_cents: u64,
    pub budget_remaining_cents: u64,
    pub active: bool,
    pub service_key: String,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SponsoredApiRow {
    pub id: Uuid,
    pub name: String,
    pub sponsor: String,
    pub description: Option<String>,
    pub upstream_url: String,
    pub upstream_method: String,
    pub upstream_headers: sqlx::types::Json<HashMap<String, String>>,
    pub price_cents: i64,
    pub budget_total_cents: i64,
    pub budget_remaining_cents: i64,
    pub active: bool,
    pub service_key: String,
    pub created_at: DateTime<Utc>,
}

impl TryFrom<SponsoredApiRow> for SponsoredApi {
    type Error = String;

    fn try_from(value: SponsoredApiRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: value.id,
            name: value.name,
            sponsor: value.sponsor,
            description: value.description,
            upstream_url: value.upstream_url,
            upstream_method: value.upstream_method,
            upstream_headers: value.upstream_headers.0,
            price_cents: u64::try_from(value.price_cents)
                .map_err(|_| "price_cents must be non-negative".to_string())?,
            budget_total_cents: u64::try_from(value.budget_total_cents)
                .map_err(|_| "budget_total_cents must be non-negative".to_string())?,
            budget_remaining_cents: u64::try_from(value.budget_remaining_cents)
                .map_err(|_| "budget_remaining_cents must be non-negative".to_string())?,
            active: value.active,
            service_key: value.service_key,
            created_at: value.created_at,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateSponsoredApiRequest {
    pub name: String,
    pub sponsor: String,
    pub description: Option<String>,
    pub upstream_url: String,
    #[serde(default)]
    pub upstream_method: Option<String>,
    #[serde(default)]
    pub upstream_headers: HashMap<String, String>,
    #[serde(default)]
    pub price_cents: Option<u64>,
    pub budget_cents: u64,
}

#[derive(Debug, Deserialize)]
pub struct SponsoredApiRunRequest {
    #[serde(default)]
    pub caller: Option<String>,
    #[serde(default)]
    pub input: Value,
}

#[derive(Debug, Serialize)]
pub struct SponsoredApiRunResponse {
    pub api_id: Uuid,
    pub payment_mode: String,
    pub sponsored_by: Option<String>,
    pub tx_hash: Option<String>,
    pub upstream_status: u16,
    pub upstream_body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SponsoredApiCall {
    pub id: Uuid,
    pub sponsored_api_id: Uuid,
    pub payment_mode: String,
    pub amount_cents: u64,
    pub tx_hash: Option<String>,
    pub caller: Option<String>,
    pub created_at: DateTime<Utc>,
}

fn read_env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}
