//! A minimal Anthropic Messages API client (blocking, no SDK: there is no
//! official Rust SDK, so this speaks the HTTP API directly).

use serde_json::{json, Value};
use std::time::Duration;

pub struct Request<'a> {
    pub system: &'a str,
    pub messages: &'a [Value],
    pub tools: &'a [Value],
}

/// What the agent loop needs from a model. Tests implement this with scripted replies.
pub trait Llm {
    /// Returns the raw `/v1/messages` response body.
    fn complete(&self, req: &Request) -> Result<Value, String>;
}

pub struct Anthropic {
    api_key: String,
    base_url: String,
    pub model: String,
    effort: Option<String>,
    max_tokens: u32,
    http: reqwest::blocking::Client,
}

impl Anthropic {
    /// Reads `ANTHROPIC_API_KEY` (required) and `ANTHROPIC_BASE_URL` (optional).
    pub fn from_env(model: &str, effort: Option<String>) -> Result<Self, String> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .ok()
            .filter(|k| !k.trim().is_empty())
            .ok_or("ANTHROPIC_API_KEY is not set. Export your API key to use the agent \
                    (`ezt-agent check` works without one).")?;
        let base_url = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_else(|_| "https://api.anthropic.com".into());
        let http = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            api_key,
            base_url: base_url.trim_end_matches('/').to_owned(),
            model: model.to_owned(),
            effort,
            max_tokens: 16_000,
            http,
        })
    }

    fn body(&self, req: &Request) -> Value {
        let mut body = json!({
            "model": self.model,
            "max_tokens": self.max_tokens,
            "system": req.system,
            "messages": req.messages,
            "tools": req.tools,
            // The system prompt, tools and earlier turns are identical on every
            // turn of the loop, so let the API cache the prefix.
            "cache_control": { "type": "ephemeral" },
        });
        if req.tools.is_empty() {
            body.as_object_mut().unwrap().remove("tools");
        }
        if let Some(effort) = &self.effort {
            body["output_config"] = json!({ "effort": effort });
        }
        body
    }
}

impl Llm for Anthropic {
    fn complete(&self, req: &Request) -> Result<Value, String> {
        let body = self.body(req);
        let mut last_error = String::new();
        for attempt in 0..4u32 {
            let resp = self
                .http
                .post(format!("{}/v1/messages", self.base_url))
                .header("x-api-key", &self.api_key)
                .header("anthropic-version", "2023-06-01")
                .json(&body)
                .send();
            let resp = match resp {
                Ok(r) => r,
                Err(e) => {
                    last_error = format!("request failed: {e}");
                    std::thread::sleep(Duration::from_secs(1 << attempt));
                    continue;
                }
            };
            let status = resp.status();
            let retry_after = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok());
            let text = resp.text().map_err(|e| e.to_string())?;
            let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            if status.is_success() {
                return Ok(value);
            }
            let message = value["error"]["message"].as_str().unwrap_or(text.as_str()).to_owned();
            last_error = format!("API error {status}: {message}");
            // Retry rate limits, overload (529) and server errors; fail fast on the rest.
            if status.as_u16() == 429 || status.is_server_error() {
                std::thread::sleep(Duration::from_secs(retry_after.unwrap_or(1 << (attempt + 1)).min(60)));
                continue;
            }
            break;
        }
        Err(last_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_body_has_expected_shape() {
        let client = Anthropic {
            api_key: "k".into(),
            base_url: "http://x".into(),
            model: "m".into(),
            effort: Some("high".into()),
            max_tokens: 10,
            http: reqwest::blocking::Client::new(),
        };
        let msgs = [json!({"role": "user", "content": "hi"})];
        let tools = [json!({"name": "t"})];
        let body = client.body(&Request { system: "sys", messages: &msgs, tools: &tools });
        assert_eq!(body["model"], "m");
        assert_eq!(body["system"], "sys");
        assert_eq!(body["output_config"]["effort"], "high");
        assert_eq!(body["messages"][0]["content"], "hi");
        assert!(body.get("temperature").is_none());
    }
}
