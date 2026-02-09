use crate::error::{ApiError, ApiResult};
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use prometheus::{IntCounter, IntCounterVec, Opts, Registry};
use reqwest::Client;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use uuid::Uuid;

pub const PAYMENT_SIGNATURE_HEADER: &str = "payment-signature";
pub const PAYMENT_RESPONSE_HEADER: &str = "payment-response";
pub const X402_VERSION_HEADER: &str = "x402-version";
pub const DEFAULT_PRICE_CENTS: u64 = 5;
pub const SPONSORED_API_CREATE_SERVICE: &str = "sponsored-api-create";
pub const SPONSORED_API_SERVICE_PREFIX: &str = "sponsored-api";
pub const DEFAULT_SPONSORED_API_CREATE_PRICE_CENTS: u64 = 25;
pub const DEFAULT_SPONSORED_API_TIMEOUT_SECS: u64 = 12;
#[derive(Clone)]
pub struct SupabaseClient {
    pub base_url: String,
    pub api_key: String,
    pub http: Client,
}

impl SupabaseClient {
    pub fn from_env(http: Client) -> Option<Self> {
        let base_url = std::env::var("SUPABASE_URL").ok()?;
        let api_key = std::env::var("SUPABASE_SERVICE_ROLE_KEY").ok()?;
        Some(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            api_key,
            http,
        })
    }

    pub fn table_url(&self, table: &str) -> String {
        format!("{}/rest/v1/{}", self.base_url, table)
    }

    pub fn authed(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        builder
            .header("apikey", &self.api_key)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
    }

    pub async fn insert_one<T: Serialize + DeserializeOwned>(
        &self,
        table: &str,
        value: &T,
    ) -> ApiResult<T> {
        let url = format!("{}?select=*", self.table_url(table));
        let response = self
            .authed(self.http.post(url))
            .header("Prefer", "return=representation")
            .json(value)
            .send()
            .await
            .map_err(|err| ApiError::supabase(StatusCode::BAD_GATEWAY, err.to_string()))?;
        let rows: Vec<T> = self.parse_json(response).await?;
        rows.into_iter()
            .next()
            .ok_or_else(|| ApiError::supabase(StatusCode::BAD_GATEWAY, "insert returned no rows"))
    }

    pub async fn insert_void<T: Serialize>(&self, table: &str, value: &T) -> ApiResult<()> {
        let url = self.table_url(table);
        let response = self
            .authed(self.http.post(url))
            .header("Prefer", "return=minimal")
            .json(value)
            .send()
            .await
            .map_err(|err| ApiError::supabase(StatusCode::BAD_GATEWAY, err.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ApiError::supabase(status, body));
        }

        Ok(())
    }

    pub async fn select_one<T: DeserializeOwned>(
        &self,
        table: &str,
        filter: &str,
    ) -> ApiResult<Option<T>> {
        let url = format!("{}?{}&select=*", self.table_url(table), filter);
        let response = self
            .authed(self.http.get(url))
            .send()
            .await
            .map_err(|err| ApiError::supabase(StatusCode::BAD_GATEWAY, err.to_string()))?;
        let rows: Vec<T> = self.parse_json(response).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn select_many<T: DeserializeOwned>(
        &self,
        table: &str,
        filter: Option<&str>,
    ) -> ApiResult<Vec<T>> {
        let url = match filter {
            Some(filter) => format!("{}?{}&select=*", self.table_url(table), filter),
            None => format!("{}?select=*", self.table_url(table)),
        };
        let response = self
            .authed(self.http.get(url))
            .send()
            .await
            .map_err(|err| ApiError::supabase(StatusCode::BAD_GATEWAY, err.to_string()))?;
        self.parse_json(response).await
    }

    pub async fn update_one<T: DeserializeOwned>(
        &self,
        table: &str,
        filter: &str,
        payload: &Value,
    ) -> ApiResult<Option<T>> {
        let url = format!("{}?{}&select=*", self.table_url(table), filter);
        let response = self
            .authed(self.http.patch(url))
            .header("Prefer", "return=representation")
            .json(payload)
            .send()
            .await
            .map_err(|err| ApiError::supabase(StatusCode::BAD_GATEWAY, err.to_string()))?;
        let rows: Vec<T> = self.parse_json(response).await?;
        Ok(rows.into_iter().next())
    }

    pub async fn parse_json<T: DeserializeOwned>(
        &self,
        response: reqwest::Response,
    ) -> ApiResult<T> {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(ApiError::supabase(status, body));
        }
        serde_json::from_str(&body)
            .map_err(|err| ApiError::supabase(status, format!("{err}: {body}")))
    }
}

#[derive(Clone)]
pub struct AppConfig {
    pub sponsored_api_create_price_cents: u64,
    pub sponsored_api_timeout_secs: u64,
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
        }
    }
}

#[derive(Clone)]
pub struct SharedState {
    pub inner: Arc<RwLock<AppState>>,
}

pub struct AppState {
    pub users: HashMap<Uuid, UserProfile>,
    pub campaigns: HashMap<Uuid, Campaign>,
    pub task_completions: Vec<TaskCompletion>,
    pub payments: HashMap<String, PaymentRecord>,
    pub creator_events: Vec<CreatorEvent>,
    pub service_prices: HashMap<String, u64>,
    pub metrics: Metrics,
    pub supabase: Option<SupabaseClient>,
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
        let mut service_prices = HashMap::new();
        service_prices.insert("scraping".to_string(), 5);
        service_prices.insert("design".to_string(), 8);
        service_prices.insert("storage".to_string(), 3);
        service_prices.insert("data-tooling".to_string(), 4);

        let http = Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .expect("http client should build");

        let config = AppConfig::from_env();
        let supabase = SupabaseClient::from_env(http.clone());

        Self {
            users: HashMap::new(),
            campaigns: HashMap::new(),
            task_completions: Vec::new(),
            payments: HashMap::new(),
            creator_events: Vec::new(),
            service_prices,
            metrics: Metrics::new(),
            supabase,
            http,
            config,
        }
    }

    pub fn service_price(&self, service: &str) -> u64 {
        self.service_prices
            .get(service)
            .copied()
            .unwrap_or(DEFAULT_PRICE_CENTS)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub id: Uuid,
    pub email: String,
    pub region: String,
    pub roles: Vec<String>,
    pub tools_used: Vec<String>,
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
    pub budget_remaining_cents: u64,
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
    pub message: String,
    pub next_step: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaymentProof {
    pub tx_hash: String,
    pub service: String,
    pub amount_cents: u64,
    pub payer: String,
    pub sponsored_campaign_id: Option<Uuid>,
    pub created_at: DateTime<Utc>,
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
pub struct PaymentRecord {
    pub tx_hash: String,
    pub campaign_id: Option<Uuid>,
    pub service: String,
    pub amount_cents: u64,
    pub payer: String,
    pub source: PaymentSource,
    pub status: PaymentStatus,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
pub struct PaymentSettlement {
    pub tx_hash: String,
    pub status: PaymentStatus,
    pub settled_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
pub struct CreateDirectPaymentRequest {
    pub payer: String,
    pub service: String,
    pub amount_cents: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct DirectPaymentResponse {
    pub tx_hash: String,
    pub payment_signature: String,
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
