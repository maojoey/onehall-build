//! Host adapters between authenticated callers and the exchange core.
use super::{Administrator, ExchangeError};
use crate::app_error::AppCommandError;

/// Single operator namespace for submissions made through the admin surface.
pub(crate) const ADMIN_OWNER: &str = "hall-administrator";

impl From<ExchangeError> for AppCommandError {
    fn from(error: ExchangeError) -> Self {
        match error {
            ExchangeError::Invalid => Self::invalid_input("Exchange request is invalid"),
            ExchangeError::Forbidden => Self::permission_denied("Exchange object is not available"),
            ExchangeError::Conflict => Self::already_exists("Exchange object changed or conflicts"),
            ExchangeError::Quota => Self::invalid_input("Exchange size or count limit exceeded"),
            ExchangeError::Unstable => Self::invalid_input("Exchange file was not stable"),
            ExchangeError::Storage => Self::io_error("Exchange storage is unavailable"),
        }
    }
}

/// Tauri-managed handle; `None` means v2 exchange stays closed on this host.
#[derive(Clone)]
#[cfg_attr(not(feature = "tauri-runtime"), allow(dead_code))]
pub struct HostExchange(pub Option<super::Exchange>);

/// Open the private store under the data directory. A failure closes exchange
/// features (v2 is rejected explicitly) instead of blocking application start.
pub fn open_default(
    db: sea_orm::DatabaseConnection,
    data_dir: &std::path::Path,
) -> Option<super::Exchange> {
    match super::Exchange::open(db, &data_dir.join("exchange")) {
        Ok(exchange) => Some(exchange),
        Err(error) => {
            tracing::warn!(%error, "file exchange store is unavailable; v2 exchange is closed");
            None
        }
    }
}

/// Only call after the surrounding router/window has authenticated the operator.
pub(crate) fn administrator() -> Administrator {
    Administrator::authenticated(ADMIN_OWNER).expect("constant owner is valid")
}
