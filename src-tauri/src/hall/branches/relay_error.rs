use crate::app_error::{AppCommandError, AppErrorCode};
use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
pub(super) struct RelayError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: &'static str,
}
impl RelayError {
    pub fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }
    pub fn upstream(error: reqwest::Error) -> Self {
        if error.is_timeout() {
            Self::new(
                StatusCode::GATEWAY_TIMEOUT,
                "branch_timeout",
                "Branch request timed out",
            )
        } else {
            Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "branch_unreachable",
                "Branch could not be reached",
            )
        }
    }
    pub fn invalid_response() -> Self {
        Self::new(
            StatusCode::BAD_GATEWAY,
            "branch_invalid_response",
            "Branch returned an invalid response",
        )
    }
}
impl From<AppCommandError> for RelayError {
    fn from(error: AppCommandError) -> Self {
        match error.code {
            AppErrorCode::InvalidInput => Self::new(
                StatusCode::BAD_REQUEST,
                "branch_invalid_request",
                "Invalid branch request",
            ),
            AppErrorCode::NotFound => Self::new(
                StatusCode::NOT_FOUND,
                "branch_not_found",
                "Branch is not registered",
            ),
            AppErrorCode::PermissionDenied => Self::new(
                StatusCode::FORBIDDEN,
                "branch_disabled",
                "Branch is disabled",
            ),
            _ => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "branch_unavailable",
                "Branch is unavailable",
            ),
        }
    }
}
impl IntoResponse for RelayError {
    fn into_response(self) -> Response {
        (
            self.status,
            [
                (header::CACHE_CONTROL, "no-store"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            Json(json!({"code":self.code,"message":self.message})),
        )
            .into_response()
    }
}
pub(super) fn check_upstream(response: &reqwest::Response) -> Result<(), RelayError> {
    if response.status() == StatusCode::UNAUTHORIZED {
        Err(RelayError::new(
            StatusCode::BAD_GATEWAY,
            "branch_auth_failed",
            "Branch credentials were rejected",
        ))
    } else if response.status().is_redirection() {
        Err(RelayError::new(
            StatusCode::BAD_GATEWAY,
            "branch_redirect_refused",
            "Branch redirects are not permitted",
        ))
    } else {
        Ok(())
    }
}
