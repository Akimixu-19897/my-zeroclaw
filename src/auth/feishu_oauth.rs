use crate::auth::oauth_common::{parse_query_params, url_encode};
use crate::auth::profiles::TokenSet;
use anyhow::{Context, Result};
use chrono::Utc;
use reqwest::Client;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[allow(unused_imports)]
pub use crate::auth::oauth_common::{generate_pkce_state, PkceState};

pub const FEISHU_OAUTH_AUTHORIZE_URL: &str =
    "https://accounts.feishu.cn/open-apis/authen/v1/authorize";
pub const FEISHU_OAUTH_TOKEN_URL: &str = "https://open.feishu.cn/open-apis/authen/v2/oauth/token";
pub const FEISHU_OAUTH_REDIRECT_URI: &str = "http://localhost:1457/auth/callback";
pub const FEISHU_OAUTH_SCOPES: &str =
    "im:message im:message:send_as_bot docs:document drive:drive wiki:wiki";

#[derive(Debug, Deserialize)]
struct TokenEnvelope {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    msg: Option<String>,
    #[serde(default)]
    data: Option<TokenResponse>,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

fn feishu_oauth_client_id() -> Option<String> {
    std::env::var("FEISHU_OAUTH_CLIENT_ID")
        .ok()
        .filter(|s| !s.is_empty())
}

fn feishu_oauth_client_secret() -> Option<String> {
    std::env::var("FEISHU_OAUTH_CLIENT_SECRET")
        .ok()
        .filter(|s| !s.is_empty())
}

fn get_oauth_credentials() -> Result<(String, String)> {
    let client_id = feishu_oauth_client_id().ok_or_else(|| {
        anyhow::anyhow!("FEISHU_OAUTH_CLIENT_ID environment variable is required")
    })?;
    let client_secret = feishu_oauth_client_secret().ok_or_else(|| {
        anyhow::anyhow!("FEISHU_OAUTH_CLIENT_SECRET environment variable is required")
    })?;
    Ok((client_id, client_secret))
}

pub fn build_authorize_url(pkce: &PkceState) -> Result<String> {
    let (client_id, _) = get_oauth_credentials()?;
    let mut params = BTreeMap::new();
    params.insert("response_type", "code");
    params.insert("client_id", client_id.as_str());
    params.insert("redirect_uri", FEISHU_OAUTH_REDIRECT_URI);
    params.insert("scope", FEISHU_OAUTH_SCOPES);
    params.insert("state", pkce.state.as_str());

    let encoded = params
        .into_iter()
        .map(|(k, v)| format!("{}={}", url_encode(k), url_encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    Ok(format!("{FEISHU_OAUTH_AUTHORIZE_URL}?{encoded}"))
}

pub async fn exchange_code_for_tokens(
    client: &Client,
    code: &str,
    _pkce: &PkceState,
) -> Result<TokenSet> {
    let (client_id, client_secret) = get_oauth_credentials()?;
    let response = client
        .post(FEISHU_OAUTH_TOKEN_URL)
        .json(&serde_json::json!({
            "grant_type": "authorization_code",
            "code": code,
            "client_id": client_id,
            "client_secret": client_secret,
            "redirect_uri": FEISHU_OAUTH_REDIRECT_URI,
        }))
        .send()
        .await
        .context("Failed to send Feishu token exchange request")?;
    parse_token_response(response).await
}

pub async fn refresh_access_token(client: &Client, refresh_token: &str) -> Result<TokenSet> {
    let (client_id, client_secret) = get_oauth_credentials()?;
    let response = client
        .post(FEISHU_OAUTH_TOKEN_URL)
        .json(&serde_json::json!({
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
            "client_id": client_id,
            "client_secret": client_secret,
        }))
        .send()
        .await
        .context("Failed to send Feishu refresh token request")?;
    parse_token_response(response).await
}

async fn parse_token_response(response: reqwest::Response) -> Result<TokenSet> {
    let status = response.status();
    let body = response
        .text()
        .await
        .context("Failed to read Feishu OAuth response body")?;
    if !status.is_success() {
        anyhow::bail!("Feishu OAuth request failed ({status}): {body}");
    }

    let envelope: TokenEnvelope =
        serde_json::from_str(&body).context("Failed to parse Feishu OAuth response")?;
    if envelope.code != 0 {
        anyhow::bail!(
            "Feishu OAuth error: {}",
            envelope.msg.unwrap_or_else(|| "unknown error".to_string())
        );
    }
    let token = envelope
        .data
        .ok_or_else(|| anyhow::anyhow!("Feishu OAuth response missing data"))?;
    let expires_at = token
        .expires_in
        .map(|secs| Utc::now() + chrono::Duration::seconds(secs));

    Ok(TokenSet {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        id_token: None,
        expires_at,
        token_type: token.token_type.or_else(|| Some("Bearer".into())),
        scope: token.scope,
    })
}

pub fn parse_code_from_redirect(input: &str, expected_state: Option<&str>) -> Result<String> {
    let trimmed = input.trim();
    if !trimmed.contains("://") && !trimmed.contains('?') && !trimmed.contains('&') {
        if trimmed.is_empty() {
            anyhow::bail!("OAuth code cannot be empty");
        }
        return Ok(trimmed.to_string());
    }

    let query = trimmed
        .split_once('?')
        .map(|(_, query)| query)
        .ok_or_else(|| anyhow::anyhow!("Redirect URL does not contain query parameters"))?;
    let params = parse_query_params(query);
    if let Some(expected_state) = expected_state {
        let state = params
            .get("state")
            .ok_or_else(|| anyhow::anyhow!("Redirect URL is missing state parameter"))?;
        if state != expected_state {
            anyhow::bail!("OAuth state mismatch");
        }
    }
    params
        .get("code")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("Redirect URL is missing code parameter"))
}

pub async fn receive_loopback_code(expected_state: &str, timeout: Duration) -> Result<String> {
    let listener = TcpListener::bind("127.0.0.1:1457")
        .await
        .context("Failed to bind Feishu OAuth callback listener on localhost:1457")?;
    let (mut stream, _) = tokio::time::timeout(timeout, listener.accept())
        .await
        .context("Timed out waiting for Feishu OAuth callback")?
        .context("Failed to accept Feishu OAuth callback")?;

    let mut buffer = vec![0_u8; 4096];
    let read = stream
        .read(&mut buffer)
        .await
        .context("Failed to read Feishu OAuth callback request")?;
    let request = String::from_utf8_lossy(&buffer[..read]);
    let request_line = request
        .lines()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Missing HTTP request line in callback"))?;
    let path = request_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Malformed HTTP request line"))?;

    let code = parse_code_from_redirect(&format!("http://localhost{path}"), Some(expected_state))?;

    let response = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nFeishu login received. You can close this window.";
    stream
        .write_all(response)
        .await
        .context("Failed to write Feishu OAuth callback response")?;
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_code_from_redirect_validates_state() {
        let code = parse_code_from_redirect(
            "http://localhost:1457/auth/callback?code=abc&state=xyz",
            Some("xyz"),
        )
        .unwrap();
        assert_eq!(code, "abc");
    }

    #[test]
    fn parse_code_from_redirect_accepts_raw_code() {
        let code = parse_code_from_redirect("abc123", None).unwrap();
        assert_eq!(code, "abc123");
    }
}
