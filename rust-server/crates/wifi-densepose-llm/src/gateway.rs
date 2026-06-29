//! API Gateway — OpenAI-compatible chat completion with circuit breaker.
//!
//! POST → SSE → tokio Stream. Circuit breaker protects against cascading failures.

use crate::types::{GatewayConfig, Prompt};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Duration;

// ── Lock-Free Circuit Breaker ──────────────────────────────────────────────────

const STATE_CLOSED: u8 = 0;
const STATE_OPEN: u8 = 1;
const STATE_HALF_OPEN: u8 = 2;

pub struct CircuitBreaker {
    state: AtomicU8,
    failure_count: AtomicU64,
    success_count: AtomicU64,
    opened_at_ms: AtomicU64,
    config: BreakerConfig,
}

#[derive(Debug, Clone)]
pub struct BreakerConfig {
    failure_threshold: u8,
    open_duration_secs: u64,
}

impl Default for BreakerConfig {
    fn default() -> Self {
        Self { failure_threshold: 3, open_duration_secs: 300 }
    }
}

impl CircuitBreaker {
    pub fn new(config: BreakerConfig) -> Self {
        Self {
            state: AtomicU8::new(STATE_CLOSED),
            failure_count: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            opened_at_ms: AtomicU64::new(0),
            config,
        }
    }

    pub fn check(&self) -> Result<(), GatewayError> {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED | STATE_HALF_OPEN => Ok(()),
            STATE_OPEN => {
                let opened_ms = self.opened_at_ms.load(Ordering::Acquire);
                let elapsed_ms = now_ms().saturating_sub(opened_ms);
                let cooldown_ms = (self.config.open_duration_secs as u64) * 1000;
                if elapsed_ms >= cooldown_ms {
                    // Try CAS Open → HalfOpen
                    if self.state.compare_exchange(
                        STATE_OPEN, STATE_HALF_OPEN,
                        Ordering::AcqRel, Ordering::Acquire,
                    ).is_ok() {
                        self.success_count.store(0, Ordering::Release);
                        return Ok(());
                    }
                    // CAS failed — another thread did the transition, retry
                    self.check()
                } else {
                    Err(GatewayError::CircuitBreakerOpen)
                }
            }
            _ => Err(GatewayError::CircuitBreakerOpen),
        }
    }

    pub fn on_success(&self) {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED => {
                self.failure_count.store(0, Ordering::Release);
            }
            STATE_HALF_OPEN => {
                // Standard circuit-breaker pattern: a single success in half-open
                // is enough to prove the downstream is healthy again.
                self.state.store(STATE_CLOSED, Ordering::Release);
                self.failure_count.store(0, Ordering::Release);
                self.success_count.store(0, Ordering::Release);
            }
            _ => {}
        }
    }

    pub fn on_failure(&self) {
        match self.state.load(Ordering::Acquire) {
            STATE_CLOSED => {
                if self.failure_count.fetch_add(1, Ordering::AcqRel) + 1
                    >= self.config.failure_threshold as u64
                {
                    let _ = self.state.compare_exchange(
                        STATE_CLOSED, STATE_OPEN,
                        Ordering::AcqRel, Ordering::Acquire,
                    );
                    self.opened_at_ms.store(now_ms(), Ordering::Release);
                }
            }
            STATE_HALF_OPEN => {
                // Any failure in half-open → back to open
                self.state.store(STATE_OPEN, Ordering::Release);
                self.opened_at_ms.store(now_ms(), Ordering::Release);
            }
            _ => {}
        }
    }

    pub fn is_open(&self) -> bool {
        self.state.load(Ordering::Acquire) == STATE_OPEN
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Error Types ──────────────────────────────────────────────────────────────

#[derive(Debug)]
pub enum GatewayError {
    CircuitBreakerOpen,
    Network(String),
    HttpStatus(u16),
    Parse(String),
    Timeout,
}

impl std::fmt::Display for GatewayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CircuitBreakerOpen => write!(f, "circuit breaker is open"),
            Self::Network(e) => write!(f, "network error: {}", e),
            Self::HttpStatus(code) => write!(f, "HTTP {}", code),
            Self::Parse(e) => write!(f, "parse error: {}", e),
            Self::Timeout => write!(f, "request timed out"),
        }
    }
}

impl std::error::Error for GatewayError {}

#[derive(Debug, Clone)]
pub struct StreamError {
    pub message: String,
}

impl std::fmt::Display for StreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "stream error: {}", self.message)
    }
}

// ── Gateway ──────────────────────────────────────────────────────────────────

const SSE_CHUNK_TIMEOUT_SECS: u64 = 45;
const CONNECT_TIMEOUT_SECS: u64 = 5;

pub struct LlmGateway {
    pub(crate) client: reqwest::Client,
    pub(crate) stream_client: reqwest::Client,
    pub(crate) circuit_breaker: Arc<CircuitBreaker>,
    pub(crate) config: GatewayConfig,
    base_url: String,
}

impl LlmGateway {
    const DEFAULT_BASE_URL: &'static str = "https://api.openai.com/v1";

    pub fn new(config: GatewayConfig) -> Result<Self, anyhow::Error> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(std::time::Duration::from_secs(config.timeout_secs))
            .build()?;

        let stream_client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(CONNECT_TIMEOUT_SECS))
            .timeout(Duration::from_secs(300)) // 5 min total ceiling for streaming
            .tcp_keepalive(Duration::from_secs(60))
            .build()?;

        let breaker_config = BreakerConfig {
            failure_threshold: config.failure_threshold,
            open_duration_secs: config.breaker_open_secs,
        };

        Ok(Self {
            client,
            stream_client,
            circuit_breaker: Arc::new(CircuitBreaker::new(breaker_config)),
            config,
            base_url: Self::DEFAULT_BASE_URL.to_string(),
        })
    }

    /// Resolve the API endpoint URL.
    fn resolve_endpoint(&self) -> String {
        let ep = &self.config.endpoint;
        if ep.is_empty() || (ep.starts_with('/') && !ep.starts_with("//")) {
            format!("{}/chat/completions", self.base_url)
        } else if ep.starts_with("http://") || ep.starts_with("https://") {
            ep.clone()
        } else {
            format!("{}/chat/completions", self.base_url)
        }
    }

    /// Stream LLM response via SSE. Returns a tokio Stream of text chunks.
    pub async fn stream(
        &self,
        prompt: &Prompt,
        route_max_tokens: u16,
    ) -> Result<
        impl tokio_stream::Stream<Item = Result<String, StreamError>>,
        GatewayError,
    > {
        self.circuit_breaker.check()?;

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                {"role": "system", "content": prompt.system},
                {"role": "user", "content": format!("{}\n\n{}", prompt.context, prompt.task)}
            ],
            "max_tokens": route_max_tokens,
            "temperature": self.config.temperature,
            "stream": true
        });

        let endpoint = self.resolve_endpoint();

        let mut request = self.stream_client.post(&endpoint).json(&body);
        if !self.config.api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.config.api_key));
        }

        let resp = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                self.circuit_breaker.on_failure();
                let err = if e.is_timeout() {
                    GatewayError::Timeout
                } else {
                    GatewayError::Network(e.to_string())
                };
                return Err(err);
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let _body = resp.text().await.unwrap_or_default();
            tracing::warn!("HTTP {} from LLM API: {}", status.as_u16(), _body);
            self.circuit_breaker.on_failure();
            return Err(GatewayError::HttpStatus(status.as_u16()));
        }

        let byte_stream = resp.bytes_stream();
        let breaker = self.circuit_breaker.clone();
        let (tx, rx) = tokio::sync::mpsc::channel::<Result<String, StreamError>>(64);

        tokio::spawn(async move {
            let mut buffer = String::new();
            tokio::pin!(byte_stream);
            use tokio_stream::StreamExt;

            loop {
                match tokio::time::timeout(
                    Duration::from_secs(SSE_CHUNK_TIMEOUT_SECS),
                    byte_stream.next(),
                ).await {
                    Ok(Some(Ok(bytes))) => {
                        buffer.push_str(&String::from_utf8_lossy(&bytes));

                        while let Some(line_end) = buffer.find('\n') {
                            let line = buffer[..line_end].trim().to_string();
                            buffer = buffer[line_end + 1..].to_string();

                            if line.is_empty() || line.starts_with(':') {
                                continue;
                            }

                            if line == "data: [DONE]" {
                                let _ = tx.send(Ok(String::new())).await;
                                break;
                            }

                            if let Some(data) = line.strip_prefix("data: ") {
                                match extract_delta_content(data) {
                                    Some(text) if !text.is_empty() => {
                                        if tx.send(Ok(text)).await.is_err() {
                                            break;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                    Ok(Some(Err(e))) => {
                        let _ = tx.send(Err(StreamError { message: e.to_string() })).await;
                        breaker.on_failure();
                        break;
                    }
                    Ok(None) => break, // stream ended normally
                    Err(_elapsed) => {
                        // SSE idle timeout — no data for SSE_CHUNK_TIMEOUT_SECS seconds
                        tracing::warn!(
                            "SSE stream idle timeout after {}s, reconnecting",
                            SSE_CHUNK_TIMEOUT_SECS
                        );
                        let _ = tx.send(Err(StreamError {
                            message: "SSE read timeout".into(),
                        })).await;
                        breaker.on_failure();
                        break;
                    }
                }
            }

            breaker.on_success();
        });

        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    /// Non-streaming completion for sync usage (returns full text).
    pub async fn complete(
        &self,
        prompt: &Prompt,
        max_tokens: u16,
    ) -> Result<String, GatewayError> {
        self.circuit_breaker.check()?;

        let body = serde_json::json!({
            "model": self.config.model,
            "messages": [
                {"role": "system", "content": prompt.system},
                {"role": "user", "content": format!("{}\n\n{}", prompt.context, prompt.task)}
            ],
            "max_tokens": max_tokens,
            "temperature": self.config.temperature,
            "stream": false
        });

        let endpoint = self.resolve_endpoint();

        let mut request = self.client.post(&endpoint).json(&body);
        if !self.config.api_key.is_empty() {
            request = request.header("Authorization", format!("Bearer {}", self.config.api_key));
        }

        let resp = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                self.circuit_breaker.on_failure();
                let err = if e.is_timeout() {
                    GatewayError::Timeout
                } else {
                    GatewayError::Network(e.to_string())
                };
                return Err(err);
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let _body = resp.text().await.unwrap_or_default();
            tracing::warn!("HTTP {} from LLM API: {}", status.as_u16(), _body);
            self.circuit_breaker.on_failure();
            return Err(GatewayError::HttpStatus(status.as_u16()));
        }

        let json: serde_json::Value = resp.json().await.map_err(|e| {
            GatewayError::Parse(e.to_string())
        })?;

        let text = json["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        self.circuit_breaker.on_success();
        Ok(text)
    }

    pub async fn ping(&self) -> bool {
        if self.config.api_key.is_empty() {
            return false;
        }
        !self.circuit_breaker.is_open()
    }

    pub async fn is_breaker_open(&self) -> bool {
        self.circuit_breaker.is_open()
    }
}

// ── SSE Parsing Helpers ──────────────────────────────────────────────────────

fn extract_delta_content(data: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(data).ok()?;
    let text = v["choices"][0]["delta"]["content"].as_str()?;
    if text.is_empty() { None } else { Some(text.to_string()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_delta_content_normal() {
        let json = r#"{"choices":[{"delta":{"content":"心动过速"}}]}"#;
        assert_eq!(extract_delta_content(json), Some("心动过速".into()));
    }

    #[test]
    fn test_extract_delta_content_empty() {
        let json = r#"{"choices":[{"delta":{"content":""}}]}"#;
        assert_eq!(extract_delta_content(json), None);
    }

    #[test]
    fn test_extract_delta_content_missing() {
        let json = r#"{"choices":[{"delta":{}}]}"#;
        assert_eq!(extract_delta_content(json), None);
    }

    #[test]
    fn test_extract_delta_content_invalid_json() {
        assert_eq!(extract_delta_content("not json"), None);
    }

    #[test]
    fn test_circuit_breaker_open_after_failures() {
        let config = BreakerConfig::default();
        let cb = CircuitBreaker::new(config);
        assert!(cb.check().is_ok());
        cb.on_failure();
        cb.on_failure();
        cb.on_failure();
        assert!(cb.is_open());
        assert!(matches!(cb.check(), Err(GatewayError::CircuitBreakerOpen)));
    }

    #[test]
    fn test_circuit_breaker_doesnt_open_below_threshold() {
        let config = BreakerConfig::default();
        let cb = CircuitBreaker::new(config);
        cb.on_failure();
        cb.on_failure();
        assert!(!cb.is_open());
        assert!(cb.check().is_ok());
    }

    #[test]
    fn test_circuit_breaker_success_resets() {
        let config = BreakerConfig::default();
        let cb = CircuitBreaker::new(config);
        cb.on_failure();
        cb.on_failure();
        cb.on_success();
        cb.on_failure();
        cb.on_failure();
        assert!(!cb.is_open());
    }

    #[test]
    fn test_circuit_breaker_url_resolution() {
        let gw = LlmGateway {
            client: reqwest::Client::new(),
            stream_client: reqwest::Client::new(),
            circuit_breaker: Arc::new(CircuitBreaker::new(BreakerConfig::default())),
            config: GatewayConfig {
                endpoint: "http://localhost:11434/v1/chat/completions".into(),
                model: "test".into(),
                api_key: String::new(),
                timeout_secs: 20,
                max_retries: 2,
                temperature: 0.3,
                failure_threshold: 3,
                breaker_open_secs: 300,
            },
            base_url: LlmGateway::DEFAULT_BASE_URL.into(),
        };
        assert_eq!(gw.resolve_endpoint(), "http://localhost:11434/v1/chat/completions");
    }

    #[test]
    fn test_circuit_breaker_url_resolution_default() {
        let gw = LlmGateway {
            client: reqwest::Client::new(),
            stream_client: reqwest::Client::new(),
            circuit_breaker: Arc::new(CircuitBreaker::new(BreakerConfig::default())),
            config: GatewayConfig {
                endpoint: String::new(),
                model: "test".into(),
                api_key: String::new(),
                timeout_secs: 20,
                max_retries: 2,
                temperature: 0.3,
                failure_threshold: 3,
                breaker_open_secs: 300,
            },
            base_url: LlmGateway::DEFAULT_BASE_URL.into(),
        };
        assert!(gw.resolve_endpoint().contains("/chat/completions"));
    }
}
