mod error;
mod onchain;
mod types;
mod utils;

use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use chrono::Utc;
use prometheus::{Encoder, TextEncoder};
use sqlx::types::Json as DbJson;
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;
use tracing::info;
use uuid::Uuid;

use crate::error::{ApiError, ApiResult};
use crate::types::*;
use crate::utils::*;

fn build_app(state: SharedState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/profiles", post(create_profile).get(list_profiles))
        .route("/register", post(register_user))
        .route("/campaigns", post(create_campaign).get(list_campaigns))
        .route("/campaigns/discovery", get(list_campaign_discovery))
        .route("/campaigns/:campaign_id", get(get_campaign))
        .route("/tasks/complete", post(complete_task))
        .route("/tool/:service/run", post(run_tool))
        .route("/proxy/:service/run", post(run_proxy))
        .route(
            "/sponsored-apis",
            post(create_sponsored_api).get(list_sponsored_apis),
        )
        .route("/sponsored-apis/:api_id", get(get_sponsored_api))
        .route("/sponsored-apis/:api_id/run", post(run_sponsored_api))
        .route(
            "/webhooks/x402scan/settlement",
            post(ingest_x402scan_settlement),
        )
        .route("/dashboard/sponsor/:campaign_id", get(sponsor_dashboard))
        .route("/creator/metrics/event", post(record_creator_metric_event))
        .route("/creator/metrics", get(creator_metrics))
        .route("/metrics", get(prometheus_metrics))
        .with_state(state)
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

    if let Some(db) = {
        let state = state.inner.read().await;
        state.db.clone()
    } {
        sqlx::migrate!("./migrations")
            .run(&db)
            .await
            .expect("database migrations should run");

        if let Err(err) = load_campaigns_from_db(&state).await {
            eprintln!("failed to load campaigns from database: {err}");
        }
    }

    let app = build_app(state);

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

async fn health(State(state): State<SharedState>) -> Response {
    let state = state.inner.read().await;
    respond(
        &state.metrics,
        "/health",
        Ok((
            StatusCode::OK,
            Json(MessageResponse {
                message: "ok".to_string(),
            }),
        )),
    )
}

async fn create_profile(
    State(state): State<SharedState>,
    Json(payload): Json<CreateUserRequest>,
) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<UserProfile>)> = async {
        let db = {
            let state = state.inner.read().await;
            state.db.clone()
        }
        .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        if payload.email.trim().is_empty() {
            return Err(ApiError::validation("email is required"));
        }

        if payload.region.trim().is_empty() {
            return Err(ApiError::validation("region is required"));
        }

        let profile = UserProfile {
            id: Uuid::new_v4(),
            email: payload.email,
            region: payload.region,
            roles: payload.roles,
            tools_used: payload.tools_used,
            attributes: payload.attributes,
            created_at: Utc::now(),
        };

        let inserted = sqlx::query_as::<_, UserProfile>(
            r#"
            insert into users (id, email, region, roles, tools_used, attributes, created_at)
            values ($1, $2, $3, $4, $5, $6, $7)
            returning id, email, region, roles, tools_used, attributes, created_at
            "#,
        )
        .bind(profile.id)
        .bind(profile.email)
        .bind(profile.region)
        .bind(profile.roles)
        .bind(profile.tools_used)
        .bind(DbJson(profile.attributes))
        .bind(profile.created_at)
        .fetch_one(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        Ok((StatusCode::CREATED, Json(inserted)))
    }
    .await;

    respond(&metrics, "/profiles", result)
}

async fn list_profiles(State(state): State<SharedState>) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<Vec<UserProfile>>)> = async {
        let db = {
            let state = state.inner.read().await;
            state.db.clone()
        }
        .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        let profiles = sqlx::query_as::<_, UserProfile>(
            r#"
            select id, email, region, roles, tools_used, attributes, created_at
            from users
            order by created_at desc
            "#,
        )
        .fetch_all(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        Ok((StatusCode::OK, Json(profiles)))
    }
    .await;

    respond(&metrics, "/profiles", result)
}

async fn register_user(
    State(state): State<SharedState>,
    Json(payload): Json<CreateUserRequest>,
) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<UserProfile>)> = async {
        let db = {
            let state = state.inner.read().await;
            state.db.clone()
        }
        .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        if payload.email.trim().is_empty() {
            return Err(ApiError::validation("email is required"));
        }

        if payload.region.trim().is_empty() {
            return Err(ApiError::validation("region is required"));
        }

        let profile = UserProfile {
            id: Uuid::new_v4(),
            email: payload.email,
            region: payload.region,
            roles: payload.roles,
            tools_used: payload.tools_used,
            attributes: payload.attributes,
            created_at: Utc::now(),
        };

        let inserted = sqlx::query_as::<_, UserProfile>(
            r#"
            insert into users (id, email, region, roles, tools_used, attributes, created_at)
            values ($1, $2, $3, $4, $5, $6, $7)
            returning id, email, region, roles, tools_used, attributes, created_at
            "#,
        )
        .bind(profile.id)
        .bind(profile.email)
        .bind(profile.region)
        .bind(profile.roles)
        .bind(profile.tools_used)
        .bind(DbJson(profile.attributes))
        .bind(profile.created_at)
        .fetch_one(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        Ok((StatusCode::CREATED, Json(inserted)))
    }
    .await;

    respond(&metrics, "/register", result)
}

async fn create_campaign(
    State(state): State<SharedState>,
    Json(payload): Json<CreateCampaignRequest>,
) -> Response {
    let (metrics, db, public_base_url) = {
        let state = state.inner.read().await;
        (
            state.metrics.clone(),
            state.db.clone(),
            state.config.public_base_url.clone(),
        )
    };

    let result: ApiResult<(StatusCode, Json<CreateCampaignResponse>)> = async {
        let db = db.ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        if payload.name.trim().is_empty() {
            return Err(ApiError::validation("name is required"));
        }
        if payload.sponsor.trim().is_empty() {
            return Err(ApiError::validation("sponsor is required"));
        }
        if payload.required_task.trim().is_empty() {
            return Err(ApiError::validation("required_task is required"));
        }
        if payload.subsidy_per_call_cents == 0 {
            return Err(ApiError::validation(
                "subsidy_per_call_cents must be greater than 0",
            ));
        }
        if payload.budget_cents == 0 {
            return Err(ApiError::validation("budget_cents must be greater than 0"));
        }

        for url in &payload.query_urls {
            reqwest::Url::parse(url)
                .map_err(|_| ApiError::validation(format!("invalid query URL: {url}")))?;
        }

        let candidate = Campaign {
            id: Uuid::new_v4(),
            name: payload.name,
            sponsor: payload.sponsor,
            target_roles: payload.target_roles,
            target_tools: payload.target_tools,
            required_task: payload.required_task,
            subsidy_per_call_cents: payload.subsidy_per_call_cents,
            budget_total_cents: payload.budget_cents,
            budget_remaining_cents: payload.budget_cents,
            query_urls: payload.query_urls,
            active: true,
            created_at: Utc::now(),
        };

        let row = sqlx::query_as::<_, CampaignRow>(
            r#"
            insert into campaigns (
                id, name, sponsor, target_roles, target_tools, required_task,
                subsidy_per_call_cents, budget_total_cents, budget_remaining_cents,
                query_urls, active, created_at
            ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            returning id, name, sponsor, target_roles, target_tools, required_task,
                subsidy_per_call_cents, budget_total_cents, budget_remaining_cents,
                query_urls, active, created_at
            "#,
        )
        .bind(candidate.id)
        .bind(candidate.name)
        .bind(candidate.sponsor)
        .bind(candidate.target_roles)
        .bind(candidate.target_tools)
        .bind(candidate.required_task)
        .bind(candidate.subsidy_per_call_cents as i64)
        .bind(candidate.budget_total_cents as i64)
        .bind(candidate.budget_remaining_cents as i64)
        .bind(candidate.query_urls)
        .bind(candidate.active)
        .bind(candidate.created_at)
        .fetch_one(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        let campaign = Campaign::try_from(row)
            .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err))?;

        let base = public_base_url.trim_end_matches('/');
        let response = CreateCampaignResponse {
            campaign: campaign.clone(),
            campaign_url: format!("{base}/campaigns/{}", campaign.id),
            dashboard_url: format!("{base}/dashboard/sponsor/{}", campaign.id),
        };

        Ok((StatusCode::CREATED, Json(response)))
    }
    .await;

    respond(&metrics, "/campaigns", result)
}

async fn list_campaigns(State(state): State<SharedState>) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<Vec<Campaign>>)> = async {
        let mut campaigns = load_campaigns_from_db(&state).await?;
        campaigns.sort_by_key(|campaign| campaign.created_at);
        Ok((StatusCode::OK, Json(campaigns)))
    }
    .await;

    respond(&metrics, "/campaigns", result)
}

async fn get_campaign(State(state): State<SharedState>, Path(campaign_id): Path<Uuid>) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<Campaign>)> = async {
        let campaigns = load_campaigns_from_db(&state).await?;
        let campaign = campaigns
            .into_iter()
            .find(|campaign| campaign.id == campaign_id)
            .ok_or_else(|| ApiError::not_found("campaign not found"))?;
        Ok((StatusCode::OK, Json(campaign)))
    }
    .await;

    respond(&metrics, "/campaigns/:campaign_id", result)
}

async fn list_campaign_discovery(State(state): State<SharedState>) -> Response {
    let (metrics, base) = {
        let state = state.inner.read().await;
        (
            state.metrics.clone(),
            state
                .config
                .public_base_url
                .trim_end_matches('/')
                .to_string(),
        )
    };

    let result: ApiResult<(StatusCode, Json<Vec<CampaignDiscoveryItem>>)> = async {
        let campaigns = load_campaigns_from_db(&state).await?;
        let mut rows: Vec<CampaignDiscoveryItem> = campaigns
            .into_iter()
            .filter(|campaign| campaign.active)
            .filter(|campaign| !campaign.query_urls.is_empty())
            .map(|campaign| CampaignDiscoveryItem {
                campaign_id: campaign.id,
                name: campaign.name,
                sponsor: campaign.sponsor,
                active: campaign.active,
                query_urls: campaign.query_urls,
                service_run_url: format!("{base}/proxy/:service/run"),
                sponsored_api_discovery_url: format!("{base}/sponsored-apis"),
            })
            .collect();
        rows.sort_by_key(|item| item.name.clone());
        Ok((StatusCode::OK, Json(rows)))
    }
    .await;

    respond(&metrics, "/campaigns/discovery", result)
}

async fn load_campaigns_from_db(state: &SharedState) -> ApiResult<Vec<Campaign>> {
    let db = {
        let state = state.inner.read().await;
        state.db.clone()
    }
    .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

    let rows = sqlx::query_as::<_, CampaignRow>(
        r#"
        select id, name, sponsor, target_roles, target_tools, required_task,
            subsidy_per_call_cents, budget_total_cents, budget_remaining_cents,
            query_urls, active, created_at
        from campaigns
        order by created_at desc
        "#,
    )
    .fetch_all(&db)
    .await
    .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

    let campaigns: Vec<Campaign> = rows
        .into_iter()
        .map(Campaign::try_from)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err))?;

    Ok(campaigns)
}

async fn complete_task(
    State(state): State<SharedState>,
    Json(payload): Json<TaskCompletionRequest>,
) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<TaskCompletion>)> = async {
        let db = {
            let state = state.inner.read().await;
            state.db.clone()
        }
        .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        // Verify campaign exists
        let campaign_exists =
            sqlx::query_scalar::<_, bool>("select exists(select 1 from campaigns where id = $1)")
                .bind(payload.campaign_id)
                .fetch_one(&db)
                .await
                .map_err(|err| {
                    ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
                })?;

        if !campaign_exists {
            return Err(ApiError::not_found("campaign not found"));
        }

        // Verify user exists
        let user_exists =
            sqlx::query_scalar::<_, bool>("select exists(select 1 from users where id = $1)")
                .bind(payload.user_id)
                .fetch_one(&db)
                .await
                .map_err(|err| {
                    ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
                })?;

        if !user_exists {
            return Err(ApiError::not_found("user not found"));
        }

        let completion = TaskCompletion {
            id: Uuid::new_v4(),
            campaign_id: payload.campaign_id,
            user_id: payload.user_id,
            task_name: payload.task_name,
            details: payload.details,
            created_at: Utc::now(),
        };

        sqlx::query(
            r#"
            insert into task_completions (id, campaign_id, user_id, task_name, details, created_at)
            values ($1, $2, $3, $4, $5, $6)
            "#,
        )
        .bind(completion.id)
        .bind(completion.campaign_id)
        .bind(completion.user_id)
        .bind(completion.task_name.clone())
        .bind(completion.details.clone())
        .bind(completion.created_at)
        .execute(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        Ok((StatusCode::CREATED, Json(completion)))
    }
    .await;

    respond(&metrics, "/tasks/complete", result)
}

async fn run_tool(
    State(state): State<SharedState>,
    Path(service): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<ServiceRunRequest>,
) -> Response {
    let (price, metrics, http, config) = {
        let state = state.inner.read().await;
        (
            state.service_price(&service),
            state.metrics.clone(),
            state.http.clone(),
            state.config.clone(),
        )
    };

    let resource_path = format!("/tool/{service}/run");
    let result: ApiResult<Response> = match verify_x402_payment(
        &http,
        &config,
        &service,
        price,
        &resource_path,
        &headers,
    )
    .await
    {
        Ok(payment) => {
            metrics
                .payment_events_total
                .with_label_values(&["user_direct", "settled"])
                .inc();

            Ok(build_paid_tool_response(
                service,
                payload,
                "user_direct".to_string(),
                None,
                payment.tx_hash,
                Some(payment.payment_response_header.as_str()),
            ))
        }
        Err(err) => Err(err),
    };

    respond(&metrics, "/tool/:service/run", result)
}

async fn run_proxy(
    State(state): State<SharedState>,
    Path(service): Path<String>,
    headers: HeaderMap,
    Json(payload): Json<ServiceRunRequest>,
) -> Response {
    let has_header = headers.contains_key(PAYMENT_SIGNATURE_HEADER);

    let (db, price, metrics, http, config) = {
        let state = state.inner.read().await;
        (
            state.db.clone(),
            state.service_price(&service),
            state.metrics.clone(),
            state.http.clone(),
            state.config.clone(),
        )
    };

    let db = match db {
        Some(db) => db,
        None => {
            return respond(
                &metrics,
                "/proxy/:service/run",
                Err::<Response, ApiError>(ApiError::config(
                    "Postgres not configured; set DATABASE_URL",
                )),
            );
        }
    };

    if has_header {
        // Verify user exists in database
        let user_exists =
            sqlx::query_scalar::<_, bool>("select exists(select 1 from users where id = $1)")
                .bind(payload.user_id)
                .fetch_one(&db)
                .await
                .map_err(|err| {
                    ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
                })
                .unwrap_or(false);

        if !user_exists {
            return respond(
                &metrics,
                "/proxy/:service/run",
                Err::<Response, ApiError>(ApiError::not_found(
                    "user profile is required before proxy usage",
                )),
            );
        }

        let resource_path = format!("/proxy/{service}/run");
        let result =
            match verify_x402_payment(&http, &config, &service, price, &resource_path, &headers)
                .await
            {
                Ok(payment) => {
                    metrics
                        .payment_events_total
                        .with_label_values(&["user_direct", "settled"])
                        .inc();

                    Ok(build_paid_tool_response(
                        service,
                        payload,
                        "user_direct".to_string(),
                        None,
                        payment.tx_hash,
                        Some(payment.payment_response_header.as_str()),
                    ))
                }
                Err(err) => Err(err),
            };

        return respond(&metrics, "/proxy/:service/run", result);
    }

    // Load user from database
    let user = sqlx::query_as::<_, UserProfile>(
        "select id, email, region, roles, tools_used, attributes, created_at from users where id = $1"
    )
    .bind(payload.user_id)
    .fetch_optional(&db)
    .await
    .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
    .and_then(|opt| {
        opt.ok_or_else(|| ApiError::not_found("user profile is required before proxy usage"))
    });

    let user = match user {
        Ok(user) => user,
        Err(err) => {
            return respond(
                &metrics,
                "/proxy/:service/run",
                Err::<Response, ApiError>(err),
            );
        }
    };

    // Load campaigns from database
    let campaigns = sqlx::query_as::<_, CampaignRow>(
        r#"
        select id, name, sponsor, target_roles, target_tools, required_task,
            subsidy_per_call_cents, budget_total_cents, budget_remaining_cents,
            query_urls, active, created_at
        from campaigns
        where active = true and budget_remaining_cents >= $1
        order by created_at desc
        "#,
    )
    .bind(price as i64)
    .fetch_all(&db)
    .await
    .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))
    .and_then(|rows| {
        rows.into_iter()
            .map(Campaign::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err))
    });

    let campaigns = match campaigns {
        Ok(campaigns) => campaigns,
        Err(err) => {
            return respond(
                &metrics,
                "/proxy/:service/run",
                Err::<Response, ApiError>(err),
            );
        }
    };

    let mut match_without_task: Option<Campaign> = None;
    let mut match_with_task: Option<Campaign> = None;

    for campaign in campaigns {
        if !user_matches_campaign(&user, &campaign) {
            continue;
        }

        match has_completed_task(&db, campaign.id, payload.user_id, &campaign.required_task).await {
            Ok(true) => {
                match_with_task = Some(campaign);
                break;
            }
            Ok(false) => {
                if match_without_task.is_none() {
                    match_without_task = Some(campaign);
                }
            }
            Err(err) => {
                return respond(
                    &metrics,
                    "/proxy/:service/run",
                    Err::<Response, ApiError>(err),
                );
            }
        }
    }

    if let Some(campaign) = match_with_task {
        let new_remaining = campaign.budget_remaining_cents.saturating_sub(price);
        let still_active = new_remaining >= price && new_remaining > 0;

        // Update campaign budget in database
        let budget_update = sqlx::query(
            r#"
            update campaigns
            set budget_remaining_cents = $1, active = $2
            where id = $3
            "#,
        )
        .bind(new_remaining as i64)
        .bind(still_active)
        .bind(campaign.id)
        .execute(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
        if let Err(err) = budget_update {
            return respond(
                &metrics,
                "/proxy/:service/run",
                Err::<Response, ApiError>(err),
            );
        }

        let tx_hash = format!("sponsor-{}", Uuid::new_v4());

        // Save payment to database
        let payment_insert = sqlx::query(
            r#"
            insert into payments (tx_hash, campaign_id, service, amount_cents, payer, source, status, created_at)
            values ($1, $2, $3, $4, $5, $6, $7, $8)
            "#,
        )
        .bind(&tx_hash)
        .bind(campaign.id)
        .bind(&service)
        .bind(price as i64)
        .bind(&campaign.sponsor)
        .bind("sponsor")
        .bind("settled")
        .bind(Utc::now())
        .execute(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()));
        if let Err(err) = payment_insert {
            return respond(
                &metrics,
                "/proxy/:service/run",
                Err::<Response, ApiError>(err),
            );
        }

        metrics
            .payment_events_total
            .with_label_values(&["sponsored", "settled"])
            .inc();
        metrics.sponsor_spend_cents_total.inc_by(price);

        return respond(
            &metrics,
            "/proxy/:service/run",
            Ok(build_paid_tool_response(
                service,
                payload,
                "sponsored".to_string(),
                Some(campaign.sponsor),
                Some(tx_hash),
                None,
            )),
        );
    }

    if let Some(campaign) = match_without_task {
        return respond(
            &metrics,
            "/proxy/:service/run",
            Err::<Response, ApiError>(ApiError::precondition(format!(
                "complete sponsor task '{}' for campaign '{}' before sponsored usage",
                campaign.required_task, campaign.name
            ))),
        );
    }

    respond(
        &metrics,
        "/proxy/:service/run",
        Err::<Response, ApiError>(payment_required_error(
            &config,
            &service,
            price,
            &format!("/proxy/{service}/run"),
            "no eligible sponsor campaign found",
            "either complete a sponsor task or pay with PAYMENT-SIGNATURE",
        )),
    )
}

async fn create_sponsored_api(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Json(payload): Json<CreateSponsoredApiRequest>,
) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<SponsoredApi>)> = async {
        let (db, http, config) = {
            let state = state.inner.read().await;
            (state.db.clone(), state.http.clone(), state.config.clone())
        };

        let db = db.ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        if payload.name.trim().is_empty() {
            return Err(ApiError::validation("name is required"));
        }
        if payload.sponsor.trim().is_empty() {
            return Err(ApiError::validation("sponsor is required"));
        }
        if payload.budget_cents == 0 {
            return Err(ApiError::validation("budget_cents must be greater than 0"));
        }

        let price_cents = payload.price_cents.unwrap_or(DEFAULT_PRICE_CENTS);
        if price_cents == 0 {
            return Err(ApiError::validation("price_cents must be greater than 0"));
        }

        let upstream_method = normalize_upstream_method(payload.upstream_method)?;
        reqwest::Url::parse(payload.upstream_url.trim())
            .map_err(|_| ApiError::validation("upstream_url must be a valid URL"))?;

        for (header, value) in &payload.upstream_headers {
            HeaderName::from_bytes(header.as_bytes())
                .map_err(|_| ApiError::validation(format!("invalid upstream header: {header}")))?;
            HeaderValue::from_str(value).map_err(|_| {
                ApiError::validation(format!("invalid upstream header value for: {header}"))
            })?;
        }

        if config.sponsored_api_create_price_cents > 0 {
            let resource_path = "/sponsored-apis".to_string();
            verify_x402_payment(
                &http,
                &config,
                SPONSORED_API_CREATE_SERVICE,
                config.sponsored_api_create_price_cents,
                &resource_path,
                &headers,
            )
            .await?;
            metrics
                .payment_events_total
                .with_label_values(&["user_direct", "settled"])
                .inc();
        }

        let api_id = Uuid::new_v4();
        let api = SponsoredApi {
            id: api_id,
            name: payload.name,
            sponsor: payload.sponsor,
            description: payload.description,
            upstream_url: payload.upstream_url,
            upstream_method,
            upstream_headers: payload.upstream_headers,
            price_cents,
            budget_total_cents: payload.budget_cents,
            budget_remaining_cents: payload.budget_cents,
            active: true,
            service_key: sponsored_api_service_key(api_id),
            created_at: Utc::now(),
        };

        let inserted_row = sqlx::query_as::<_, SponsoredApiRow>(
            r#"
            insert into sponsored_apis (
                id, name, sponsor, description, upstream_url, upstream_method,
                upstream_headers, price_cents, budget_total_cents, budget_remaining_cents,
                active, service_key, created_at
            ) values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            returning id, name, sponsor, description, upstream_url, upstream_method,
                upstream_headers, price_cents, budget_total_cents, budget_remaining_cents,
                active, service_key, created_at
            "#,
        )
        .bind(api.id)
        .bind(api.name)
        .bind(api.sponsor)
        .bind(api.description)
        .bind(api.upstream_url)
        .bind(api.upstream_method)
        .bind(DbJson(api.upstream_headers))
        .bind(api.price_cents as i64)
        .bind(api.budget_total_cents as i64)
        .bind(api.budget_remaining_cents as i64)
        .bind(api.active)
        .bind(api.service_key)
        .bind(api.created_at)
        .fetch_one(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        let inserted = SponsoredApi::try_from(inserted_row)
            .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err))?;
        Ok((StatusCode::CREATED, Json(inserted)))
    }
    .await;

    respond(&metrics, "/sponsored-apis", result)
}

async fn list_sponsored_apis(State(state): State<SharedState>) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<Vec<SponsoredApi>>)> = async {
        let db = {
            let state = state.inner.read().await;
            state.db.clone()
        }
        .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        let api_rows = sqlx::query_as::<_, SponsoredApiRow>(
            r#"
            select id, name, sponsor, description, upstream_url, upstream_method,
                upstream_headers, price_cents, budget_total_cents, budget_remaining_cents,
                active, service_key, created_at
            from sponsored_apis
            order by created_at desc
            "#,
        )
        .fetch_all(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        let apis: Vec<SponsoredApi> = api_rows
            .into_iter()
            .map(SponsoredApi::try_from)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err))?;

        Ok((StatusCode::OK, Json(apis)))
    }
    .await;

    respond(&metrics, "/sponsored-apis", result)
}

async fn get_sponsored_api(State(state): State<SharedState>, Path(api_id): Path<Uuid>) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<SponsoredApi>)> = async {
        let db = {
            let state = state.inner.read().await;
            state.db.clone()
        }
        .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        let api = sqlx::query_as::<_, SponsoredApiRow>(
            r#"
            select id, name, sponsor, description, upstream_url, upstream_method,
                upstream_headers, price_cents, budget_total_cents, budget_remaining_cents,
                active, service_key, created_at
            from sponsored_apis
            where id = $1
            "#,
        )
        .bind(api_id)
        .fetch_optional(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
        .ok_or_else(|| ApiError::not_found("sponsored api not found"))
        .and_then(|row| {
            SponsoredApi::try_from(row)
                .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err))
        })?;

        Ok((StatusCode::OK, Json(api)))
    }
    .await;

    respond(&metrics, "/sponsored-apis/:api_id", result)
}

async fn run_sponsored_api(
    State(state): State<SharedState>,
    Path(api_id): Path<Uuid>,
    headers: HeaderMap,
    Json(payload): Json<SponsoredApiRunRequest>,
) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<Response> = async {
        let (db, http, config) = {
            let state = state.inner.read().await;
            (state.db.clone(), state.http.clone(), state.config.clone())
        };

        let db = db.ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        let api = sqlx::query_as::<_, SponsoredApiRow>(
            r#"
            select id, name, sponsor, description, upstream_url, upstream_method,
                upstream_headers, price_cents, budget_total_cents, budget_remaining_cents,
                active, service_key, created_at
            from sponsored_apis
            where id = $1
            "#,
        )
        .bind(api_id)
        .fetch_optional(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
        .ok_or_else(|| ApiError::not_found("sponsored api not found"))
        .and_then(|row| {
            SponsoredApi::try_from(row)
                .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err))
        })?;

        let price = api.price_cents;
        let service_key = api.service_key.clone();
        let mut payment_mode = "sponsored".to_string();
        let mut sponsored_by = None;
        let mut tx_hash: Option<String> = None;
        let mut payment_response_header: Option<String> = None;

        if headers.contains_key(PAYMENT_SIGNATURE_HEADER) {
            let resource_path = format!("/sponsored-apis/{api_id}/run");
            let payment = verify_x402_payment(
                &http,
                &config,
                &service_key,
                price,
                &resource_path,
                &headers,
            )
            .await?;
            metrics
                .payment_events_total
                .with_label_values(&["user_direct", "settled"])
                .inc();
            payment_mode = "user_direct".to_string();
            tx_hash = payment.tx_hash;
            payment_response_header = Some(payment.payment_response_header);
        } else if api.active && api.budget_remaining_cents >= price {
            let new_remaining = api.budget_remaining_cents.saturating_sub(price);
            let still_active = new_remaining >= price && new_remaining > 0;

            sqlx::query(
                r#"
                update sponsored_apis
                set budget_remaining_cents = $1, active = $2
                where id = $3
                "#,
            )
            .bind(new_remaining as i64)
            .bind(still_active)
            .bind(api.id)
            .execute(&db)
            .await
            .map_err(|err| {
                ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
            })?;

            metrics
                .payment_events_total
                .with_label_values(&["sponsored", "settled"])
                .inc();
            metrics.sponsor_spend_cents_total.inc_by(price);
            sponsored_by = Some(api.sponsor.clone());
        } else {
            return Err(payment_required_error(
                &config,
                &service_key,
                price,
                &format!("/sponsored-apis/{api_id}/run"),
                "sponsored budget exhausted",
                "pay with PAYMENT-SIGNATURE and retry",
            ));
        }

        let SponsoredApiRunRequest { caller, input } = payload;
        let (upstream_status, upstream_body) =
            call_upstream(&http, &api, input, config.sponsored_api_timeout_secs).await?;

        let call_log = SponsoredApiCall {
            id: Uuid::new_v4(),
            sponsored_api_id: api.id,
            payment_mode: payment_mode.clone(),
            amount_cents: price,
            tx_hash: tx_hash.clone(),
            caller,
            created_at: Utc::now(),
        };

        sqlx::query(
            r#"
            insert into sponsored_api_calls (
                id, sponsored_api_id, payment_mode, amount_cents, tx_hash, caller, created_at
            ) values ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(call_log.id)
        .bind(call_log.sponsored_api_id)
        .bind(call_log.payment_mode)
        .bind(call_log.amount_cents as i64)
        .bind(call_log.tx_hash)
        .bind(call_log.caller)
        .bind(call_log.created_at)
        .execute(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        let response_payload = SponsoredApiRunResponse {
            api_id: api.id,
            payment_mode,
            sponsored_by,
            tx_hash,
            upstream_status,
            upstream_body,
        };

        let mut response = (StatusCode::OK, Json(response_payload)).into_response();
        response.headers_mut().insert(
            HeaderName::from_static(X402_VERSION_HEADER),
            HeaderValue::from_static("2"),
        );
        if let Some(settlement_header) = payment_response_header {
            if let Ok(header_value) = HeaderValue::from_str(&settlement_header) {
                response.headers_mut().insert(
                    HeaderName::from_static(PAYMENT_RESPONSE_HEADER),
                    header_value,
                );
            }
        }

        Ok(response)
    }
    .await;

    respond(&metrics, "/sponsored-apis/:api_id/run", result)
}

async fn ingest_x402scan_settlement(
    State(state): State<SharedState>,
    Json(payload): Json<X402ScanSettlementRequest>,
) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<MessageResponse>)> = async {
        let db = {
            let state = state.inner.read().await;
            state.db.clone()
        }
        .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        let source_str = match payload.source {
            PaymentSource::User => "user",
            PaymentSource::Sponsor => "sponsor",
        };
        let status_str = match payload.status {
            PaymentStatus::Settled => "settled",
            PaymentStatus::Failed => "failed",
        };

        sqlx::query(
            r#"
            insert into payments (tx_hash, campaign_id, service, amount_cents, payer, source, status, created_at)
            values ($1, $2, $3, $4, $5, $6, $7, $8)
            on conflict (tx_hash) do nothing
            "#,
        )
        .bind(&payload.tx_hash)
        .bind(payload.campaign_id)
        .bind(&payload.service)
        .bind(payload.amount_cents as i64)
        .bind(&payload.payer)
        .bind(source_str)
        .bind(status_str)
        .bind(Utc::now())
        .execute(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        let mode = match payload.source {
            PaymentSource::User => "user_direct",
            PaymentSource::Sponsor => "sponsored",
        };
        let status = match payload.status {
            PaymentStatus::Settled => "settled",
            PaymentStatus::Failed => "failed",
        };

        metrics
            .payment_events_total
            .with_label_values(&[mode, status])
            .inc();

        Ok((
            StatusCode::ACCEPTED,
            Json(MessageResponse {
                message: "settlement ingested".to_string(),
            }),
        ))
    }
    .await;

    respond(&metrics, "/webhooks/x402scan/settlement", result)
}

async fn sponsor_dashboard(
    State(state): State<SharedState>,
    Path(campaign_id): Path<Uuid>,
) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<SponsorDashboard>)> = async {
        let db = {
            let state = state.inner.read().await;
            state.db.clone()
        }
        .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        // Load campaign from database
        let campaign_row = sqlx::query_as::<_, CampaignRow>(
            r#"
            select id, name, sponsor, target_roles, target_tools, required_task,
                subsidy_per_call_cents, budget_total_cents, budget_remaining_cents,
                query_urls, active, created_at
            from campaigns
            where id = $1
            "#,
        )
        .bind(campaign_id)
        .fetch_optional(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
        .ok_or_else(|| ApiError::not_found("campaign not found"))?;

        let campaign = Campaign::try_from(campaign_row)
            .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err))?;

        // Count task completions
        let tasks_completed = sqlx::query_scalar::<_, i64>(
            "select count(*) from task_completions where campaign_id = $1",
        )
        .bind(campaign_id)
        .fetch_one(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
            as usize;

        // Get sponsored payments
        let payment_amounts: Vec<i64> = sqlx::query_scalar::<_, i64>(
            r#"
            select amount_cents
            from payments
            where campaign_id = $1
              and source = 'sponsor'
              and status = 'settled'
            "#,
        )
        .bind(campaign_id)
        .fetch_all(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        let sponsored_calls = payment_amounts.len();
        let spend_cents: u64 = payment_amounts
            .into_iter()
            .map(|amount| amount as u64)
            .sum();

        let response = SponsorDashboard {
            remaining_budget_cents: campaign.budget_remaining_cents,
            campaign,
            tasks_completed,
            sponsored_calls,
            spend_cents,
        };

        Ok((StatusCode::OK, Json(response)))
    }
    .await;

    respond(&metrics, "/dashboard/sponsor/:campaign_id", result)
}

async fn record_creator_metric_event(
    State(state): State<SharedState>,
    Json(payload): Json<CreatorMetricEventRequest>,
) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<CreatorEvent>)> = async {
        let db = {
            let state = state.inner.read().await;
            state.db.clone()
        }
        .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        let event = CreatorEvent {
            id: Uuid::new_v4(),
            skill_name: payload.skill_name,
            platform: payload.platform,
            event_type: payload.event_type,
            duration_ms: payload.duration_ms,
            success: payload.success,
            created_at: Utc::now(),
        };

        sqlx::query(
            r#"
            insert into creator_events (id, skill_name, platform, event_type, duration_ms, success, created_at)
            values ($1, $2, $3, $4, $5, $6, $7)
            "#,
        )
        .bind(event.id)
        .bind(&event.skill_name)
        .bind(&event.platform)
        .bind(&event.event_type)
        .bind(event.duration_ms.map(|d| d as i64))
        .bind(event.success)
        .bind(event.created_at)
        .execute(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        metrics
            .creator_events_total
            .with_label_values(&[&event.skill_name, &event.platform, &event.event_type])
            .inc();

        Ok((StatusCode::CREATED, Json(event)))
    }
    .await;

    respond(&metrics, "/creator/metrics/event", result)
}

async fn creator_metrics(State(state): State<SharedState>) -> Response {
    let metrics = {
        let state = state.inner.read().await;
        state.metrics.clone()
    };

    let result: ApiResult<(StatusCode, Json<CreatorMetricSummary>)> = async {
        let db = {
            let state = state.inner.read().await;
            state.db.clone()
        }
        .ok_or_else(|| ApiError::config("Postgres not configured; set DATABASE_URL"))?;

        // Get total events and success events
        let total_events = sqlx::query_scalar::<_, i64>("select count(*) from creator_events")
            .fetch_one(&db)
            .await
            .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
            as usize;

        let success_events = sqlx::query_scalar::<_, i64>(
            "select count(*) from creator_events where success = true",
        )
        .fetch_one(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?
            as usize;

        let success_rate = if total_events == 0 {
            0.0
        } else {
            success_events as f64 / total_events as f64
        };

        // Get per-skill metrics
        #[derive(sqlx::FromRow)]
        struct SkillMetricsRow {
            skill_name: String,
            total_events: i64,
            success_events: i64,
            avg_duration_ms: Option<f64>,
            last_seen_at: chrono::DateTime<chrono::Utc>,
        }

        let skill_rows = sqlx::query_as::<_, SkillMetricsRow>(
            r#"
            select
                skill_name,
                count(*) as total_events,
                count(*) filter (where success = true) as success_events,
                avg(duration_ms) as avg_duration_ms,
                max(created_at) as last_seen_at
            from creator_events
            group by skill_name
            order by total_events desc, last_seen_at desc
            "#,
        )
        .fetch_all(&db)
        .await
        .map_err(|err| ApiError::database(StatusCode::INTERNAL_SERVER_ERROR, err.to_string()))?;

        let per_skill: Vec<SkillMetrics> = skill_rows
            .into_iter()
            .map(|row| SkillMetrics {
                skill_name: row.skill_name,
                total_events: row.total_events as usize,
                success_events: row.success_events as usize,
                avg_duration_ms: row.avg_duration_ms,
                last_seen_at: row.last_seen_at,
            })
            .collect();

        Ok((
            StatusCode::OK,
            Json(CreatorMetricSummary {
                total_events,
                success_events,
                success_rate,
                per_skill,
            }),
        ))
    }
    .await;

    respond(&metrics, "/creator/metrics", result)
}

async fn prometheus_metrics(State(state): State<SharedState>) -> Response {
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
        .into_response()
}

#[cfg(test)]
mod test;
