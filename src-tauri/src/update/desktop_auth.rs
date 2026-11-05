//! Credentials and fail-closed policy for the local OneHall desktop updater.
use base64::Engine;
use serde_json::Value;
use url::Url;

pub(crate) fn validate_token(raw: &str) -> Result<&str, &'static str> {
    let token = raw
        .strip_suffix("\r\n")
        .or_else(|| raw.strip_suffix('\n'))
        .unwrap_or(raw);
    if token.len() != 64
        || !token
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err("Update credential must contain 64 lowercase hexadecimal characters");
    }
    Ok(token)
}

pub(crate) fn authorization_header(token: Option<&str>) -> Result<http::HeaderValue, &'static str> {
    let token = validate_token(
        token.ok_or("Update credential missing; import this device's credential file")?,
    )?;
    let mut value = http::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|_| "Invalid update credential")?;
    value.set_sensitive(true);
    Ok(value)
}

pub(crate) fn validate_url(url: &Url, origin: &Url) -> Result<(), &'static str> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.origin() != origin.origin()
    {
        return Err("Update URL must use the configured HTTPS origin without credentials, query or fragment");
    }
    Ok(())
}

pub(crate) fn validate_window(label: &str, url: &Url) -> Result<(), &'static str> {
    let local_origin = (url.scheme() == "tauri" && url.host_str() == Some("localhost"))
        || (matches!(url.scheme(), "http" | "https") && url.host_str() == Some("tauri.localhost"))
        || (cfg!(debug_assertions)
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1")));
    if !matches!(label, "main" | "settings")
        || !local_origin
        || url
            .query_pairs()
            .any(|(name, _)| matches!(name.as_ref(), "remoteConnectionId" | "remoteWindowId"))
    {
        return Err("Device updates require a local desktop window");
    }
    Ok(())
}

fn key_packet(value: &Value) -> Option<Vec<u8>> {
    let outer = base64::engine::general_purpose::STANDARD
        .decode(value.as_str()?)
        .ok()?;
    let text = std::str::from_utf8(&outer).ok()?;
    let lines: Vec<_> = text.trim_end().lines().collect();
    if lines.len() != 2 || !lines[0].starts_with("untrusted comment: ") {
        return None;
    }
    let packet = base64::engine::general_purpose::STANDARD
        .decode(lines[1])
        .ok()?;
    (packet.len() == 42 && packet.starts_with(b"Ed")).then_some(packet)
}

pub(crate) fn validate_policy(
    policy: &Value,
    own: &Value,
    base: &Value,
    runtime: &Value,
) -> Result<(), &'static str> {
    let denied = "OneHall signed desktop updates are not configured";
    if policy["product"] != "onehall" || policy["updateMode"] != "desktop-signed" {
        return Err(denied);
    }
    let packet = key_packet(&own["pubkey"]).ok_or(denied)?;
    // The key id is editable metadata; ownership depends on the Ed25519 bytes.
    if packet[10..] == key_packet(&base["pubkey"]).ok_or(denied)?[10..]
        || Some(packet) != key_packet(&runtime["pubkey"])
        || runtime["requireSignedVersion"] != true
        || runtime["allowDowngrades"] == true
        || [
            "dangerousInsecureTransportProtocol",
            "dangerousAcceptInvalidCerts",
            "dangerousAcceptInvalidHostnames",
        ]
        .iter()
        .any(|field| runtime[field] == true)
        || runtime["endpoints"] != own["endpoints"]
    {
        return Err(denied);
    }
    let endpoints = own["endpoints"]
        .as_array()
        .filter(|v| !v.is_empty())
        .ok_or(denied)?;
    let origin = Url::parse(endpoints[0].as_str().ok_or(denied)?).map_err(|_| denied)?;
    for endpoint in endpoints {
        let url = Url::parse(endpoint.as_str().ok_or(denied)?).map_err(|_| denied)?;
        validate_url(&url, &origin)?;
        // Tauri expands placeholders; an origin must never be templated.
        if url.host_str().is_some_and(|host| host.contains(['{', '}'])) {
            return Err(denied);
        }
    }
    Ok(())
}

pub(crate) fn read_credential_file(path: &std::path::Path) -> Result<String, &'static str> {
    use std::io::Read;
    let denied = "Unable to read update credential file";
    if !path.is_absolute() {
        return Err("Update credential file requires an absolute path");
    }
    let metadata = std::fs::symlink_metadata(path).map_err(|_| denied)?;
    if !metadata.is_file() || metadata.len() > 66 {
        return Err(denied);
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| denied)?;
    if !file.metadata().map_err(|_| denied)?.is_file() {
        return Err(denied);
    }
    let mut raw = String::new();
    file.take(67).read_to_string(&mut raw).map_err(|_| denied)?;
    Ok(validate_token(&raw)?.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn key(byte: u8) -> String {
        let mut packet = vec![b'E', b'd'];
        packet.extend([byte; 40]);
        base64::engine::general_purpose::STANDARD.encode(format!(
            "untrusted comment: test\n{}\n",
            base64::engine::general_purpose::STANDARD.encode(packet)
        ))
    }
    fn configs() -> (Value, Value, Value, Value) {
        let policy = serde_json::json!({"product":"onehall","updateMode":"desktop-signed"});
        let own = serde_json::json!({"pubkey":key(1),"endpoints":["https://updates.example/latest.json"]});
        let base = serde_json::json!({"pubkey":key(2)});
        let mut runtime = own.clone();
        runtime["requireSignedVersion"] = true.into();
        (policy, own, base, runtime)
    }
    #[test]
    fn token_is_exact_lowercase_hex_with_optional_file_line_ending() {
        let valid = "a1".repeat(32);
        assert_eq!(validate_token(&valid).unwrap(), valid);
        assert_eq!(validate_token(&format!("{valid}\n")).unwrap(), valid);
        for raw in [
            "A1".repeat(32),
            "a".repeat(63),
            "a".repeat(65),
            format!(" {valid}"),
            format!("{valid}\nextra"),
        ] {
            assert!(validate_token(&raw).is_err());
        }
    }
    #[test]
    fn artifact_and_manifest_require_same_clean_https_origin() {
        let origin = Url::parse("https://updates.example/latest.json").unwrap();
        assert!(validate_url(
            &Url::parse("https://updates.example/v1/pkg.zip").unwrap(),
            &origin
        )
        .is_ok());
        for raw in [
            "http://updates.example/a",
            "https://other.example/a",
            "https://updates.example:444/a",
            "https://user@updates.example/a",
            "https://updates.example/a?secret=x",
            "https://updates.example/a#fragment",
        ] {
            assert!(
                validate_url(&Url::parse(raw).unwrap(), &origin).is_err(),
                "{raw}"
            );
        }
    }
    #[test]
    fn remote_or_external_windows_cannot_manage_device_updates() {
        let local = Url::parse("tauri://localhost/settings").unwrap();
        assert!(validate_window("settings", &local).is_ok());
        assert!(validate_window("main", &local).is_ok());
        assert!(validate_window("remote-settings-1", &local).is_err());
        assert!(validate_window(
            "settings",
            &Url::parse("tauri://localhost/settings?remoteConnectionId=1").unwrap()
        )
        .is_err());
        assert!(validate_window(
            "settings",
            &Url::parse("https://attacker.example/settings").unwrap()
        )
        .is_err());
    }
    #[test]
    fn desktop_policy_requires_explicit_own_key_runtime_match_and_secure_transport() {
        let (policy, own, base, runtime) = configs();
        assert!(validate_policy(&policy, &own, &base, &runtime).is_ok());
        let mut manual = policy.clone();
        manual["updateMode"] = "manual".into();
        assert!(validate_policy(&manual, &own, &base, &runtime).is_err());
        assert!(validate_policy(&policy, &base, &base, &runtime).is_err());
        for field in ["pubkey", "endpoints", "requireSignedVersion"] {
            let mut changed = runtime.clone();
            changed[field] = Value::Null;
            assert!(validate_policy(&policy, &own, &base, &changed).is_err());
        }
        for field in [
            "dangerousInsecureTransportProtocol",
            "dangerousAcceptInvalidCerts",
            "dangerousAcceptInvalidHostnames",
        ] {
            let mut changed = runtime.clone();
            changed[field] = true.into();
            assert!(validate_policy(&policy, &own, &base, &changed).is_err());
        }
    }
    #[test]
    fn refuses_inherited_key_even_with_a_different_comment_and_matching_runtime() {
        let (policy, mut own, base, mut runtime) = configs();
        let text = base64::engine::general_purpose::STANDARD
            .decode(base["pubkey"].as_str().unwrap())
            .unwrap();
        let renamed = String::from_utf8(text)
            .unwrap()
            .replace("comment: test", "comment: OneHall");
        own["pubkey"] = base64::engine::general_purpose::STANDARD
            .encode(renamed)
            .into();
        runtime["pubkey"] = own["pubkey"].clone();
        assert!(validate_policy(&policy, &own, &base, &runtime).is_err());
    }

    #[test]
    fn refuses_cross_origin_manifest_fallback_and_unsigned_version_policy() {
        let (policy, mut own, base, mut runtime) = configs();
        own["endpoints"] = serde_json::json!([
            "https://updates.example/latest.json",
            "https://other.example/latest.json"
        ]);
        runtime["endpoints"] = own["endpoints"].clone();
        assert!(validate_policy(&policy, &own, &base, &runtime).is_err());
        let (policy, own, base, mut runtime) = configs();
        runtime["requireSignedVersion"] = false.into();
        assert!(validate_policy(&policy, &own, &base, &runtime).is_err());
        runtime["requireSignedVersion"] = true.into();
        runtime["allowDowngrades"] = true.into();
        assert!(validate_policy(&policy, &own, &base, &runtime).is_err());
    }

    #[test]
    fn imports_only_a_small_regular_explicit_file_and_sanitizes_failures() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("device-secret-name");
        let token = "a1".repeat(32);
        std::fs::write(&path, format!("{token}\r\n")).unwrap();
        assert_eq!(read_credential_file(&path).unwrap(), token);
        assert!(read_credential_file(std::path::Path::new("relative.token")).is_err());
        assert!(read_credential_file(dir.path()).is_err());
        std::fs::write(&path, "x".repeat(1024)).unwrap();
        let error = read_credential_file(&path).unwrap_err();
        assert!(!error.contains("device-secret-name"));
        assert!(!error.contains(&token));
        #[cfg(unix)]
        {
            let link = dir.path().join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(read_credential_file(&link).is_err());
        }
    }
    #[test]
    fn refuses_upstream_ed25519_key_with_a_changed_key_id() {
        let (policy, mut own, base, mut runtime) = configs();
        let mut packet = vec![b'E', b'd'];
        packet.extend([9; 8]);
        packet.extend([2; 32]);
        own["pubkey"] = base64::engine::general_purpose::STANDARD
            .encode(format!(
                "untrusted comment: own\n{}\n",
                base64::engine::general_purpose::STANDARD.encode(packet)
            ))
            .into();
        runtime["pubkey"] = own["pubkey"].clone();
        assert!(validate_policy(&policy, &own, &base, &runtime).is_err());
    }
    #[test]
    fn missing_or_malformed_credential_cannot_authorize_a_request() {
        assert!(authorization_header(None).is_err());
        assert!(authorization_header(Some("invalid")).is_err());
        let token = "a1".repeat(32);
        let header = authorization_header(Some(&token)).unwrap();
        assert_eq!(header.to_str().unwrap(), format!("Bearer {token}"));
        assert!(header.is_sensitive());
    }
}
