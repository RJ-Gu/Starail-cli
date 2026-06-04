use std::time::Duration;

use anyhow::{bail, Context, Result};
use reqwest::blocking::{Client, RequestBuilder};
use serde_json::{json, Value};

use crate::config::AppConfig;
use crate::paths::StarailPaths;

const USER_AGENT: &str = concat!("starail/", env!("CARGO_PKG_VERSION"));

pub fn get_configs(paths: &StarailPaths) -> Result<Value> {
    request(paths, reqwest::Method::GET, "/configs", None)
}

pub fn get_proxies(paths: &StarailPaths) -> Result<Value> {
    request(paths, reqwest::Method::GET, "/proxies", None)
}

pub fn set_mode(paths: &StarailPaths, mode: &str) -> Result<()> {
    request(
        paths,
        reqwest::Method::PATCH,
        "/configs",
        Some(json!({ "mode": mode })),
    )?;
    println!("Mode set to {mode}.");
    Ok(())
}

pub fn select_proxy(paths: &StarailPaths, group: &str, node: &str) -> Result<()> {
    let group = urlencoding::encode(group);
    request(
        paths,
        reqwest::Method::PUT,
        &format!("/proxies/{group}"),
        Some(json!({ "name": node })),
    )?;
    Ok(())
}

pub fn delay(paths: &StarailPaths, node: &str, test_url: &str, timeout: u64) -> Result<Value> {
    let config = AppConfig::load(paths)?;
    let client = client(Duration::from_secs((timeout / 1000).saturating_add(5)))?;
    let node = urlencoding::encode(node);
    let url = format!("{}/proxies/{node}/delay", config.controller_base());
    let timeout = timeout.to_string();
    let builder = client
        .get(url)
        .query(&[("url", test_url), ("timeout", timeout.as_str())]);

    authed(builder, &config)
        .send()
        .context("failed to call mihomo delay API")?
        .error_for_status()
        .context("mihomo delay API returned an error")?
        .json::<Value>()
        .context("failed to parse mihomo delay API response")
}

fn request(
    paths: &StarailPaths,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    let config = AppConfig::load(paths)?;
    let client = client(Duration::from_secs(5))?;
    let url = format!("{}{}", config.controller_base(), path);
    let mut builder = client.request(method, url);
    if let Some(body) = body {
        builder = builder.json(&body);
    }

    let response = authed(builder, &config)
        .send()
        .context("failed to reach mihomo external controller")?
        .error_for_status()
        .context("mihomo external controller returned an error")?;

    if response.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(Value::Null);
    }

    let text = response
        .text()
        .context("failed to read controller response")?;
    if text.trim().is_empty() {
        Ok(Value::Null)
    } else {
        serde_json::from_str(&text).with_context(|| "failed to parse controller JSON response")
    }
}

fn client(timeout: Duration) -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .timeout(timeout)
        .build()
        .context("failed to create HTTP client")
}

fn authed(builder: RequestBuilder, config: &AppConfig) -> RequestBuilder {
    match config.controller_secret.as_deref() {
        Some(secret) if !secret.trim().is_empty() => builder.bearer_auth(secret),
        _ => builder,
    }
}

pub fn controller_required_error() -> anyhow::Error {
    anyhow::anyhow!("controller is unreachable. Start mihomo first and check 'starail status'.")
}

pub fn ensure_controller_value(value: Value) -> Result<Value> {
    if value.is_null() {
        bail!("mihomo controller returned an empty response");
    }
    Ok(value)
}
