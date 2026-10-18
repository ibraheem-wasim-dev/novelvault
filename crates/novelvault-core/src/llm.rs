//! Local language-model client.
//!
//! The novel never leaves the computer, so this module will only ever connect to the
//! loopback interface. That is enforced here, in `validate`, not left to the UI: a
//! config pointing at any other host is refused before a socket is opened.
//!
//! Two local servers are supported, both of which run on the user's own machine:
//!
//! * `ollama`        — Ollama (`/api/chat`, `/api/tags`)
//! * `openai_local`  — anything that speaks the OpenAI chat format locally
//!                     (llama.cpp `llama-server`, LM Studio, Jan, ...)
//!
//! The HTTP client is deliberately tiny and dependency-free: plain `std::net` over
//! `127.0.0.1`, no TLS, no proxy support, no way to reach the internet.

use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, TcpStream};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::{Result, VaultError};

pub const PROVIDERS: [&str; 3] = ["none", "ollama", "openai_local"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LlmConfig {
    pub provider: String,
    pub host: String,
    pub port: u16,
    pub model: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        LlmConfig {
            provider: "none".into(),
            host: "127.0.0.1".into(),
            port: 11434,
            model: String::new(),
        }
    }
}

impl LlmConfig {
    pub fn enabled(&self) -> bool {
        self.provider != "none" && !self.model.trim().is_empty()
    }

    /// Sensible port for a provider when the user has not chosen one.
    pub fn default_port(provider: &str) -> u16 {
        match provider {
            "openai_local" => 8080,
            _ => 11434,
        }
    }
}

fn loopback_addr(host: &str, port: u16) -> Option<SocketAddr> {
    let h = host.trim().to_ascii_lowercase();
    let ip = match h.as_str() {
        "localhost" | "127.0.0.1" => IpAddr::V4(Ipv4Addr::LOCALHOST),
        "::1" | "[::1]" => IpAddr::V6(Ipv6Addr::LOCALHOST),
        _ => return None,
    };
    Some(SocketAddr::new(ip, port))
}

/// Refuse anything that is not an on-this-computer model server.
pub fn validate(cfg: &LlmConfig) -> Result<()> {
    if !PROVIDERS.contains(&cfg.provider.as_str()) {
        return Err(VaultError::Other(format!(
            "unknown AI provider '{}' (expected one of: {})",
            cfg.provider,
            PROVIDERS.join(", ")
        )));
    }
    if cfg.provider != "none" && loopback_addr(&cfg.host, cfg.port).is_none() {
        return Err(VaultError::Other(format!(
            "'{}' is not on this computer. NovelVault only talks to a local AI (localhost / 127.0.0.1), \
             so your unpublished novel never leaves your machine.",
            cfg.host
        )));
    }
    if cfg.port == 0 {
        return Err(VaultError::Other("AI port must be between 1 and 65535".into()));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Minimal HTTP/1.1 over loopback
// ---------------------------------------------------------------------------

fn request(
    cfg: &LlmConfig,
    method: &str,
    path: &str,
    body: Option<&str>,
    read_timeout: Duration,
) -> Result<(u16, String)> {
    validate(cfg)?;
    let addr = loopback_addr(&cfg.host, cfg.port).ok_or_else(|| VaultError::Other("not a loopback address".into()))?;
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3)).map_err(|e| {
        VaultError::Other(format!(
            "could not reach the local AI at {}:{} ({e}). Is it running?",
            cfg.host, cfg.port
        ))
    })?;
    stream.set_read_timeout(Some(read_timeout))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let payload = body.unwrap_or("");
    let head = format!(
        "{method} {path} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nAccept: application/json\r\n\
         Content-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        cfg.host,
        cfg.port,
        payload.len()
    );
    stream.write_all(head.as_bytes())?;
    stream.write_all(payload.as_bytes())?;
    stream.flush()?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).map_err(|e| {
        VaultError::Other(format!("the local AI stopped responding ({e}). A large model can be slow — try again."))
    })?;
    parse_response(&raw)
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

fn parse_response(raw: &[u8]) -> Result<(u16, String)> {
    let split = find(raw, b"\r\n\r\n").ok_or_else(|| VaultError::Other("malformed response from the local AI".into()))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_string();
    let body = &raw[split + 4..];
    let status: u16 = head
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| VaultError::Other("malformed status line from the local AI".into()))?;
    let chunked = head
        .lines()
        .any(|l| l.to_ascii_lowercase().starts_with("transfer-encoding:") && l.to_ascii_lowercase().contains("chunked"));
    let bytes = if chunked { dechunk(body) } else { body.to_vec() };
    Ok((status, String::from_utf8_lossy(&bytes).to_string()))
}

pub(crate) fn dechunk(mut body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(eol) = find(body, b"\r\n") else { break };
        let size_line = String::from_utf8_lossy(&body[..eol]).to_string();
        let size_hex = size_line.split(';').next().unwrap_or("").trim().to_string();
        let Ok(size) = usize::from_str_radix(&size_hex, 16) else { break };
        body = &body[eol + 2..];
        if size == 0 || body.len() < size {
            if size != 0 {
                out.extend_from_slice(body); // truncated stream: keep what arrived
            }
            break;
        }
        out.extend_from_slice(&body[..size]);
        body = &body[size..];
        if body.starts_with(b"\r\n") {
            body = &body[2..];
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Provider operations
// ---------------------------------------------------------------------------

/// Models the local server has installed. A failure here is how the app learns the
/// AI is not running, so it is reported rather than hidden.
pub fn list_models(cfg: &LlmConfig) -> Result<Vec<String>> {
    let path = match cfg.provider.as_str() {
        "ollama" => "/api/tags",
        "openai_local" => "/v1/models",
        _ => return Ok(Vec::new()),
    };
    let (status, body) = request(cfg, "GET", path, None, Duration::from_secs(5))?;
    if status != 200 {
        return Err(VaultError::Other(format!("local AI answered HTTP {status}")));
    }
    let v: Value = serde_json::from_str(&body)?;
    let mut names: Vec<String> = match cfg.provider.as_str() {
        "ollama" => v["models"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["name"].as_str().map(String::from)).collect())
            .unwrap_or_default(),
        _ => v["data"]
            .as_array()
            .map(|a| a.iter().filter_map(|m| m["id"].as_str().map(String::from)).collect())
            .unwrap_or_default(),
    };
    names.sort();
    Ok(names)
}

/// One non-streaming chat completion. `messages` are (role, content) pairs and must not
/// include the system prompt.
pub fn chat(cfg: &LlmConfig, system: &str, messages: &[(String, String)]) -> Result<String> {
    if !cfg.enabled() {
        return Err(VaultError::Other("no local AI model is configured".into()));
    }
    let mut msgs = vec![json!({ "role": "system", "content": system })];
    for (role, content) in messages {
        msgs.push(json!({ "role": role, "content": content }));
    }
    let (path, body) = match cfg.provider.as_str() {
        "ollama" => (
            "/api/chat",
            json!({ "model": cfg.model, "messages": msgs, "stream": false,
                    "options": { "temperature": 0.2, "num_ctx": 8192 } }),
        ),
        _ => (
            "/v1/chat/completions",
            json!({ "model": cfg.model, "messages": msgs, "stream": false, "temperature": 0.2 }),
        ),
    };
    let (status, text) = request(cfg, "POST", path, Some(&body.to_string()), Duration::from_secs(600))?;
    if status != 200 {
        let detail: String = text.chars().take(300).collect();
        return Err(VaultError::Other(format!("local AI answered HTTP {status}: {detail}")));
    }
    let v: Value = serde_json::from_str(&text)?;
    let answer = match cfg.provider.as_str() {
        "ollama" => v["message"]["content"].as_str(),
        _ => v["choices"][0]["message"]["content"].as_str(),
    };
    answer
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| VaultError::Other("the local AI returned an empty answer".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_hosts_are_accepted() {
        let mut cfg = LlmConfig { provider: "ollama".into(), model: "m".into(), ..Default::default() };
        assert!(validate(&cfg).is_ok());
        for bad in ["192.168.1.20", "example.com", "0.0.0.0", "10.0.0.5", "api.openai.com"] {
            cfg.host = bad.into();
            assert!(validate(&cfg).is_err(), "{bad} must be refused");
        }
        for good in ["localhost", "127.0.0.1", "::1"] {
            cfg.host = good.into();
            assert!(validate(&cfg).is_ok(), "{good} must be accepted");
        }
    }

    #[test]
    fn chunked_bodies_are_reassembled() {
        let body = b"5\r\nhello\r\n7\r\n, world\r\n0\r\n\r\n";
        assert_eq!(dechunk(body), b"hello, world");
    }

    #[test]
    fn unknown_providers_are_refused() {
        let cfg = LlmConfig { provider: "cloud".into(), ..Default::default() };
        assert!(validate(&cfg).is_err());
    }
}
