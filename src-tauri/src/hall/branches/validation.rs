use crate::{
    app_error::AppCommandError, db::service::remote_workspace_connection_service as remote,
    models::RemoteWorkspaceHeader,
};
pub fn validate_uuid(id: &str) -> Result<(), AppCommandError> {
    if uuid::Uuid::parse_str(id).is_ok_and(|u| u.hyphenated().to_string() == id) {
        Ok(())
    } else {
        Err(AppCommandError::invalid_input("Invalid branch identifier"))
    }
}
pub fn validate_endpoint(raw: &str) -> Result<String, AppCommandError> {
    let raw = raw.trim();
    let invalid = || {
        AppCommandError::invalid_input(
            "Branch URL must be an http(s) host with an unambiguous base path",
        )
    };
    if raw.len() > 2048 || raw.contains('\\') || raw.split('/').any(|s| matches!(s, "." | "..")) {
        return Err(invalid());
    }
    let normalized = remote::normalize_base_url(raw).map_err(|_| invalid())?;
    let url = reqwest::Url::parse(&normalized).map_err(|_| invalid())?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path().contains('%')
        || url.path().contains("//")
        || url
            .path()
            .split('/')
            .any(|s| s.eq_ignore_ascii_case("branches"))
    {
        return Err(invalid());
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

pub(super) fn credentials(
    token: &str,
    headers: &[RemoteWorkspaceHeader],
) -> Result<Vec<RemoteWorkspaceHeader>, AppCommandError> {
    if token.trim().is_empty()
        || token.len() > 8192
        || http::HeaderValue::from_str(&format!("Bearer {}", token.trim())).is_err()
    {
        return Err(AppCommandError::invalid_input("Invalid branch token"));
    }
    if headers.len() > 32
        || headers
            .iter()
            .any(|h| h.name.len() > 128 || h.value.len() > 8192)
    {
        return Err(AppCommandError::invalid_input(
            "Too many or oversized branch headers",
        ));
    }
    // In addition to transport framing, prohibit headers which impersonate an
    // incoming operator or establish browser sessions at the upstream.
    for h in headers {
        let name = h.name.trim().to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "cookie"
                | "set-cookie"
                | "forwarded"
                | "origin"
                | "referer"
                | "proxy-authorization"
                | "proxy-authenticate"
                | "trailer"
                | "te"
        ) || name.starts_with("x-forwarded-")
            || name.starts_with("sec-")
        {
            return Err(AppCommandError::invalid_input(
                "Reserved branch credential header",
            ));
        }
    }
    remote::validate_headers(headers)
        .map_err(|_| AppCommandError::invalid_input("Invalid branch credential headers"))
}
