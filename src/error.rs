use axum::{
    Json,
    http::{HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::Value;
use thiserror::Error;

use crate::types::{PaymentRequired, X402_VERSION_HEADER};

pub type ApiResult<T> = Result<T, ApiError>;

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Value>,
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("payment required")]
    PaymentRequired(PaymentRequired),
    #[error("{message}")]
    Http {
        status: StatusCode,
        code: String,
        message: String,
    },
    #[error("supabase error: {message}")]
    Supabase { status: StatusCode, message: String },
    #[error("upstream error: {message}")]
    Upstream { status: StatusCode, message: String },
    #[error("config error: {message}")]
    Config { message: String },
    #[error("internal error: {message}")]
    Internal { message: String },
}

impl ApiError {
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::Http {
            status: StatusCode::NOT_FOUND,
            code: "not_found".to_string(),
            message: message.into(),
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::Http {
            status: StatusCode::BAD_REQUEST,
            code: "validation_error".to_string(),
            message: message.into(),
        }
    }

    pub fn precondition(message: impl Into<String>) -> Self {
        Self::Http {
            status: StatusCode::PRECONDITION_REQUIRED,
            code: "precondition_required".to_string(),
            message: message.into(),
        }
    }

    pub fn supabase(status: StatusCode, message: impl Into<String>) -> Self {
        Self::Supabase {
            status,
            message: message.into(),
        }
    }

    pub fn upstream(status: StatusCode, message: impl Into<String>) -> Self {
        Self::Upstream {
            status,
            message: message.into(),
        }
    }

    pub fn config(message: impl Into<String>) -> Self {
        Self::Config {
            message: message.into(),
        }
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::Internal {
            message: message.into(),
        }
    }

    fn status_code(&self) -> StatusCode {
        match self {
            Self::PaymentRequired(_) => StatusCode::PAYMENT_REQUIRED,
            Self::Http { status, .. } => *status,
            Self::Supabase { status, .. } => *status,
            Self::Upstream { status, .. } => *status,
            Self::Config { .. } | Self::Internal { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    fn body(&self) -> ErrorBody {
        match self {
            Self::PaymentRequired(_) => ErrorBody {
                code: "payment_required".to_string(),
                message: "payment required".to_string(),
                details: None,
            },
            Self::Http { code, message, .. } => ErrorBody {
                code: code.clone(),
                message: message.clone(),
                details: None,
            },
            Self::Supabase { message, .. } => ErrorBody {
                code: "supabase_error".to_string(),
                message: message.clone(),
                details: None,
            },
            Self::Upstream { message, .. } => ErrorBody {
                code: "upstream_error".to_string(),
                message: message.clone(),
                details: None,
            },
            Self::Config { message } => ErrorBody {
                code: "config_error".to_string(),
                message: message.clone(),
                details: None,
            },
            Self::Internal { message } => ErrorBody {
                code: "internal_error".to_string(),
                message: message.clone(),
                details: None,
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            ApiError::PaymentRequired(payload) => payment_required_response(payload),
            other => {
                let status = other.status_code();
                let body = ErrorResponse {
                    error: other.body(),
                };
                (status, Json(body)).into_response()
            }
        }
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
