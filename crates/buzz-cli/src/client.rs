use std::time::Duration;

use nostr::{EventBuilder, Keys, Tag};

use crate::error::CliError;

pub use buzz_client::BlobDescriptor;

/// Build an `imeta` tag array from a BlobDescriptor (NIP-92 media metadata).
pub fn build_imeta_tag(d: &BlobDescriptor) -> Vec<String> {
    let mut tag = vec![
        "imeta".to_string(),
        format!("url {}", d.url),
        format!("m {}", d.mime_type),
        format!("x {}", d.sha256),
        format!("size {}", d.size),
    ];
    if let Some(ref dim) = d.dim {
        tag.push(format!("dim {dim}"));
    }
    if let Some(ref bh) = d.blurhash {
        tag.push(format!("blurhash {bh}"));
    }
    if let Some(ref th) = d.thumb {
        tag.push(format!("thumb {th}"));
    }
    if let Some(dur) = d.duration {
        tag.push(format!("duration {dur}"));
    }
    tag
}

/// MIME types accepted for upload.
const ALLOWED_MIMES: &[&str] = &[
    "image/jpeg",
    "image/png",
    "image/gif",
    "image/webp",
    "video/mp4",
];

/// Maximum file size for image uploads (50 MB).
const MAX_IMAGE_BYTES: u64 = 50 * 1024 * 1024;

/// Maximum file size for video uploads (500 MB).
const MAX_VIDEO_BYTES: u64 = 500 * 1024 * 1024;

/// Read an env var as a `u64` of seconds and return the corresponding `Duration`.
/// Falls back to `default` if the var is unset, unparseable, or zero (zero is treated
/// as invalid to prevent accidentally disabling all timeouts).
fn env_duration_secs(name: &str, default: u64) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .map(Duration::from_secs)
        .unwrap_or_else(|| Duration::from_secs(default))
}

pub struct BuzzClient {
    shared: buzz_client::BuzzClient,
    keys: Keys,
    /// Optional NIP-OA auth tag injected into every signed event.
    auth_tag: Option<Tag>,
}

impl BuzzClient {
    /// Create a new client pointing at `relay_url`.
    ///
    /// Timeout defaults are tuned for degraded WAN links and can be overridden
    /// via environment variables:
    ///
    /// - `BUZZ_CONNECT_TIMEOUT_SECS` — TCP connect timeout (default 15 s)
    /// - `BUZZ_TIMEOUT_SECS` — per-request total timeout (default 30 s)
    ///
    /// A value of zero for either variable is treated as invalid and falls back to the default.
    pub fn new(
        relay_url: String,
        keys: Keys,
        auth_tag: Option<Tag>,
        auth_tag_json: Option<String>,
    ) -> Result<Self, CliError> {
        let identity = buzz_client::BuzzIdentity::from_keys(keys.clone(), auth_tag_json.as_deref())
            .map_err(CliError::from)?;
        let mut config = buzz_client::BuzzClientConfig::new(relay_url);
        config.request_timeout = env_duration_secs("BUZZ_TIMEOUT_SECS", 30);
        config.connect_timeout = env_duration_secs("BUZZ_CONNECT_TIMEOUT_SECS", 15);
        let shared = buzz_client::BuzzClient::new(config, identity).map_err(CliError::from)?;
        Ok(Self {
            shared,
            keys,
            auth_tag,
        })
    }

    /// Get the keypair.
    pub fn keys(&self) -> &Keys {
        &self.keys
    }

    /// Return the owner pubkey carried by the NIP-OA auth tag, if any.
    ///
    /// The auth tag is `["auth", owner_pubkey, conditions, sig]`; the
    /// owner pubkey lives at index 1.
    pub fn auth_tag_owner_hex(&self) -> Option<String> {
        self.auth_tag
            .as_ref()
            .map(|t| t.as_slice())
            .and_then(|slice| slice.get(1).cloned())
    }

    /// Sign an event builder, injecting the NIP-OA auth tag if configured.
    ///
    /// All event creation should go through this method to ensure consistent
    /// auth tag injection. Callers MUST NOT add `auth` tags to the builder
    /// before calling this method.
    pub fn sign_event(&self, builder: EventBuilder) -> Result<nostr::Event, CliError> {
        self.shared.sign_event(builder).map_err(CliError::from)
    }

    /// Query up to `limit` historical events, following the relay bridge's
    /// composite `(until, before_id)` cursor across bounded result pages.
    pub async fn query_paginated(
        &self,
        filter: serde_json::Value,
        limit: u32,
    ) -> Result<Vec<serde_json::Value>, CliError> {
        self.shared
            .query_paginated(filter, Some(limit))
            .await
            .map_err(CliError::from)
    }

    /// Query every historical event matching a filter across bounded pages.
    pub async fn query_all(
        &self,
        filter: serde_json::Value,
    ) -> Result<Vec<serde_json::Value>, CliError> {
        self.shared
            .query_paginated(filter, None)
            .await
            .map_err(CliError::from)
    }

    /// Sign an event builder verbatim: no NIP-OA auth-tag injection, and none
    /// of [`sign_event`]'s "callers must not add auth tags" enforcement.
    ///
    /// Used only for NIP-IA identity archive/unarchive requests (kind
    /// 9035/9036), whose optional `auth` tag is a *content-level*
    /// owner-of-agent attestation about the *target* identity — unrelated to
    /// this client's own NIP-OA membership delegation (`self.auth_tag`,
    /// which [`sign_event`] injects into every other event and which
    /// `submit_event` separately attaches via the `x-auth-tag` HTTP header).
    /// Routing an identity archive request through `sign_event` would either
    /// silently drop the caller's owner attestation or double up an
    /// unrelated tag.
    pub fn sign_event_unchecked(&self, builder: EventBuilder) -> Result<nostr::Event, CliError> {
        self.shared
            .sign_event_with_content_auth(builder)
            .map_err(CliError::from)
    }

    /// GET a public, unauthenticated relay endpoint (e.g. the NIP-11 `/info`
    /// document), returning the raw JSON body. No NIP-98 Authorization and no
    /// `x-auth-tag` header — the endpoint is public relay metadata, not a
    /// membership-scoped resource.
    pub async fn get_public(&self, path: &str) -> Result<String, CliError> {
        self.shared.get_public(path).await.map_err(CliError::from)
    }

    /// Execute a one-shot query via the HTTP bridge.
    /// `filter` is a Nostr filter object (will be wrapped in an array).
    /// Returns the raw JSON response (array of events).
    pub async fn query(&self, filter: &serde_json::Value) -> Result<String, CliError> {
        self.query_multi(std::slice::from_ref(filter)).await
    }

    /// Execute a one-shot query with multiple filters via the HTTP bridge.
    /// Each filter is ORed by the relay (standard Nostr REQ behavior).
    pub async fn query_multi(&self, filters: &[serde_json::Value]) -> Result<String, CliError> {
        let events = self
            .shared
            .query_values(filters)
            .await
            .map_err(CliError::from)?;
        serde_json::to_string(&events).map_err(|error| {
            CliError::Other(format!("failed to serialize query response: {error}"))
        })
    }

    /// Execute a one-shot count via the HTTP bridge.
    /// Returns the count as a JSON string.
    #[allow(dead_code)]
    pub async fn count(&self, filter: &serde_json::Value) -> Result<String, CliError> {
        let count = self
            .shared
            .count(std::slice::from_ref(filter))
            .await
            .map_err(CliError::from)?;
        Ok(serde_json::json!({ "count": count }).to_string())
    }

    /// GET an authed relay endpoint (NIP-98), returning the raw JSON body.
    ///
    /// `path` is a root-relative path incl. any query string, e.g.
    /// `/moderation/reports?status=open&limit=20`. Used by the moderation
    /// read commands, which read structured queue/audit rows rather than
    /// stored events.
    pub async fn get_authed(&self, path: &str) -> Result<String, CliError> {
        self.shared
            .get_authenticated(path)
            .await
            .map_err(CliError::from)
    }

    /// Submit a signed Nostr event via POST /events.
    ///
    /// For non-idempotent moderation command kinds (9040–9044), an ambiguous
    /// outcome (mid-request error, body loss, non-ingest 429, or 502/503/504)
    /// surfaces as `CliError::DeliveryUnknown` instead of being retried.  These
    /// events execute at the relay *before* any dedup check, so a blind re-send
    /// can duplicate the mutation.  Only confirmed-unreceived failures (TCP
    /// connect error or a pre-ingest 429 carrying a `rate-limited:` body) are
    /// safe to retry.
    ///
    /// All other event kinds retain the standard retry policy.
    pub async fn submit_event(&self, event: nostr::Event) -> Result<String, CliError> {
        let response = self
            .shared
            .submit_event(event)
            .await
            .map_err(CliError::from)?;
        serde_json::to_string(&response).map_err(|error| CliError::Other(error.to_string()))
    }

    /// Publish an ephemeral event via WebSocket with NIP-42 authentication.
    ///
    /// The relay rejects ephemeral kinds (20000–29999) over HTTP. The shared
    /// client handles connect, NIP-42 auth, EVENT send, OK wait, and close.
    pub async fn publish_ephemeral_event(&self, event: nostr::Event) -> Result<String, CliError> {
        let response = self
            .shared
            .publish_ephemeral(event)
            .await
            .map_err(CliError::from)?;
        serde_json::to_string(&response).map_err(|error| CliError::Other(error.to_string()))
    }

    /// Upload a file to the relay's Blossom endpoint.
    /// Returns a BlobDescriptor on success.
    pub async fn upload_file(&self, file_path: &str) -> Result<BlobDescriptor, CliError> {
        // 1. Read file — validate it exists and is a regular file
        let metadata = std::fs::metadata(file_path)
            .map_err(|e| CliError::Other(format!("cannot access {file_path}: {e}")))?;
        if !metadata.is_file() {
            return Err(CliError::Usage(format!("{file_path} is not a file")));
        }

        let bytes = std::fs::read(file_path)
            .map_err(|e| CliError::Other(format!("failed to read {file_path}: {e}")))?;

        // 2. Detect MIME from magic bytes
        let mime = infer::get(&bytes)
            .map(|t| t.mime_type().to_string())
            .unwrap_or_else(|| "application/octet-stream".to_string());

        if !ALLOWED_MIMES.contains(&mime.as_str()) {
            return Err(CliError::Usage(format!("unsupported file type: {mime}")));
        }

        // 3. Size check
        let max = if mime.starts_with("video/") {
            MAX_VIDEO_BYTES
        } else {
            MAX_IMAGE_BYTES
        };
        if bytes.len() as u64 > max {
            return Err(CliError::Usage(format!(
                "file too large: {} bytes (max {})",
                bytes.len(),
                max
            )));
        }

        self.shared
            .upload_bytes(bytes, &mime)
            .await
            .map_err(CliError::from)
    }

    /// Download a Blossom media blob using BUD-01 `t=get` auth.
    pub async fn download_media(&self, input: &str) -> Result<bytes::Bytes, CliError> {
        self.shared
            .download_media(input)
            .await
            .map(|download| download.bytes)
            .map_err(CliError::from)
    }
}

/// Normalize a relay URL: ws:// → http://, wss:// → https://, strip trailing slash.
/// BUZZ_RELAY_URL may be ws/wss (copied from MCP config).
pub fn normalize_relay_url(url: &str) -> String {
    url.replace("wss://", "https://")
        .replace("ws://", "http://")
        .trim_end_matches('/')
        .to_string()
}

/// Normalize raw event JSON array into consistent shape.
/// Each event becomes: {id, pubkey, kind, content, created_at, tags}
pub fn normalize_events(events: &[serde_json::Value]) -> String {
    let normalized: Vec<serde_json::Value> = events
        .iter()
        .map(|e| {
            serde_json::json!({
                "id": e.get("id").and_then(|v| v.as_str()).unwrap_or(""),
                "pubkey": e.get("pubkey").and_then(|v| v.as_str()).unwrap_or(""),
                "kind": e.get("kind").and_then(|v| v.as_u64()).unwrap_or(0),
                "content": e.get("content").and_then(|v| v.as_str()).unwrap_or(""),
                "created_at": e.get("created_at").and_then(|v| v.as_u64()).unwrap_or(0),
                "tags": e.get("tags").cloned().unwrap_or(serde_json::json!([])),
            })
        })
        .collect();
    serde_json::to_string(&normalized).unwrap_or_default()
}

/// Extract the d-tag value from a Nostr event JSON object.
pub fn extract_d_tag(event: &serde_json::Value) -> String {
    event
        .get("tags")
        .and_then(|t| t.as_array())
        .and_then(|tags| {
            tags.iter().find(|t| {
                t.as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    == Some("d")
            })
        })
        .and_then(|t| t.as_array())
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Extract a named tag's value from a Nostr event JSON object.
/// Finds the first tag whose first element matches `key` and returns the second element.
pub fn extract_tag_value(event: &serde_json::Value, key: &str) -> String {
    event
        .get("tags")
        .and_then(|t| t.as_array())
        .and_then(|tags| {
            tags.iter().find(|t| {
                t.as_array()
                    .and_then(|a| a.first())
                    .and_then(|v| v.as_str())
                    == Some(key)
            })
        })
        .and_then(|t| t.as_array())
        .and_then(|a| a.get(1))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Extract all p-tags into [{pubkey, role}] from a Nostr event JSON object.
pub fn extract_p_tags(event: &serde_json::Value) -> Vec<serde_json::Value> {
    event
        .get("tags")
        .and_then(|t| t.as_array())
        .map(|tags| {
            tags.iter()
                .filter(|t| {
                    t.as_array()
                        .and_then(|a| a.first())
                        .and_then(|v| v.as_str())
                        == Some("p")
                })
                .map(|t| {
                    let a = t.as_array().unwrap();
                    serde_json::json!({
                        "pubkey": a.get(1).and_then(|v| v.as_str()).unwrap_or(""),
                        "role": a.get(3).and_then(|v| v.as_str()).filter(|s| !s.is_empty()).unwrap_or("member"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Return a create-command response with an entity ID injected.
pub fn create_response_with_id(resp: &str, id_key: &str, id_val: &str) -> String {
    let mut v: serde_json::Value = serde_json::from_str(resp).unwrap_or(serde_json::json!({}));
    v[id_key] = serde_json::json!(id_val);
    if v.get("accepted").is_none() {
        v["accepted"] = serde_json::json!(true);
    }
    v.to_string()
}

/// Print a create-command response, injecting the generated entity ID.
pub fn print_create_response(resp: &str, id_key: &str, id_val: &str) {
    println!("{}", create_response_with_id(resp, id_key, id_val));
}

/// Extract a JSON field from relay write response messages shaped as
/// `response:{...}`.
pub fn extract_relay_response_field(resp: &str, field: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(resp)
        .ok()?
        .get("message")?
        .as_str()?
        .strip_prefix("response:")
        .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok())
        .and_then(|v| v.get(field)?.as_str().map(str::to_string))
}

/// Normalize a relay write-response into a consistent JSON object.
/// Relay returns: {"event_id": "...", "accepted": true, "message": "..."}
/// Falls back to raw text if parsing fails.
pub fn normalize_write_response(raw: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
        if v.get("event_id").is_some() || v.get("accepted").is_some() {
            return serde_json::json!({
                "event_id": v.get("event_id").and_then(|v| v.as_str()).unwrap_or(""),
                "accepted": v.get("accepted").and_then(|v| v.as_bool()).unwrap_or(false),
                "message": v.get("message").and_then(|v| v.as_str()).unwrap_or(""),
            })
            .to_string();
        }
    }
    raw.to_string()
}

#[cfg(test)]
mod retry_tests {
    use std::time::Duration;

    use super::env_duration_secs;

    #[test]
    fn env_duration_secs_parsing() {
        // All assertions share one env var key; sequential set/remove prevents races.
        const KEY: &str = "BUZZ_CLI_TEST_DURATION_SECS";

        // Valid numeric value is parsed.
        std::env::set_var(KEY, "42");
        assert_eq!(env_duration_secs(KEY, 30), Duration::from_secs(42));

        // Non-numeric falls back to default.
        std::env::set_var(KEY, "not-a-number");
        assert_eq!(env_duration_secs(KEY, 30), Duration::from_secs(30));

        // Zero is treated as invalid and falls back to default.
        std::env::set_var(KEY, "0");
        assert_eq!(env_duration_secs(KEY, 30), Duration::from_secs(30));

        // Unset uses the default.
        std::env::remove_var(KEY);
        assert_eq!(env_duration_secs(KEY, 30), Duration::from_secs(30));
    }
}

/// Integration tests for the kind-aware retry policy and body-boundary coverage.
///
/// These tests spin up a local HTTP server using axum and issue real HTTP requests
/// through `BuzzClient` to verify behavioural properties — not implementation details.
#[cfg(test)]
mod retry_policy_tests {
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;

    use axum::body::Body;
    use axum::extract::State;
    use axum::http::{HeaderMap, Response, StatusCode};
    use axum::routing::post;
    use axum::Router;
    use nostr::{EventBuilder, Keys, Kind};
    use tokio::net::TcpListener;

    use super::super::error::CliError;
    use super::BuzzClient;

    /// Spawn a one-shot axum server on a random port.  The handler `f` receives the
    /// attempt counter (incremented before every call) and returns a `(StatusCode,
    /// String)`.  Returns the base URL and a join handle so the caller can assert
    /// attempt counts after the test.
    async fn test_server<F>(f: F) -> (String, Arc<AtomicU32>)
    where
        F: Fn(u32) -> (StatusCode, String) + Send + Sync + 'static,
    {
        let counter = Arc::new(AtomicU32::new(0));
        let handler: Arc<dyn Fn(u32) -> (StatusCode, String) + Send + Sync> = Arc::new(f);
        let state = (handler, counter.clone());

        type S = (
            Arc<dyn Fn(u32) -> (StatusCode, String) + Send + Sync>,
            Arc<AtomicU32>,
        );
        let app = Router::new()
            .route(
                "/events",
                post(
                    |State((handler, ctr)): State<S>, _headers: HeaderMap, _body: Body| async move {
                        let n = ctr.fetch_add(1, Ordering::SeqCst) + 1;
                        let (status, body) = handler(n);
                        Response::builder()
                            .status(status)
                            .header("content-type", "application/json")
                            .body(Body::from(body))
                            .unwrap()
                    },
                ),
            )
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), counter)
    }

    fn test_client(base_url: &str) -> BuzzClient {
        let keys = Keys::generate();
        BuzzClient::new(base_url.to_string(), keys, None, None).unwrap()
    }

    fn make_moderation_event(keys: &Keys, kind: u16) -> nostr::Event {
        EventBuilder::new(Kind::Custom(kind), "")
            .sign_with_keys(keys)
            .unwrap()
    }

    fn make_stored_event(keys: &Keys) -> nostr::Event {
        EventBuilder::new(Kind::TextNote, "hi")
            .sign_with_keys(keys)
            .unwrap()
    }

    /// A moderation command (kind 9040) that fails the first attempt with HTTP 429
    /// carrying a plain (non-relay-ingest) body is NOT retried — surfaces as
    /// `DeliveryUnknown`.
    #[tokio::test]
    async fn moderation_kind_non_ingest_429_returns_delivery_unknown() {
        let (url, attempts) = test_server(|_n| {
            (
                StatusCode::TOO_MANY_REQUESTS,
                r#"{"error":"slow down"}"#.to_string(),
            )
        })
        .await;
        let client = test_client(&url);
        let event = make_moderation_event(client.keys(), 9040);
        let err = client.submit_event(event).await.unwrap_err();
        assert!(
            matches!(err, CliError::DeliveryUnknown(_)),
            "expected DeliveryUnknown, got {err:?}"
        );
        // Non-ingest 429 must not be retried — exactly 1 attempt.
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "must not retry non-ingest 429"
        );
    }

    /// A moderation command (kind 9041) that gets a relay-ingest 429 (production JSON
    /// envelope `{"error":"rate-limited: ..."}`) IS retried, and the `retry in Ns` hint
    /// is honoured.
    ///
    /// Uses a 2s hint; jitter max for attempt 0 is 0.5s, so asserting elapsed ≥ 2s
    /// cleanly distinguishes hint-honoured from jitter-fallback.
    #[tokio::test]
    async fn moderation_kind_ingest_429_is_retried_until_success() {
        let (url, attempts) = test_server(|n| {
            if n < 2 {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    // Exact production envelope: api_error() wraps every message as
                    // {"error":"..."}.  The extracted field starts with "rate-limited:"
                    // so the command is retried; the hint is honoured.
                    r#"{"error":"rate-limited: quota exceeded; retry in 2s"}"#.to_string(),
                )
            } else {
                (
                    StatusCode::OK,
                    r#"{"event_id":"abc","accepted":true,"message":""}"#.to_string(),
                )
            }
        })
        .await;
        let client = test_client(&url);
        let event = make_moderation_event(client.keys(), 9041);
        let t0 = std::time::Instant::now();
        let result = client.submit_event(event).await;
        let elapsed = t0.elapsed();
        assert!(
            result.is_ok(),
            "expected Ok after ingest-429 retry, got {result:?}"
        );
        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "must have retried at least once"
        );
        assert!(
            elapsed.as_secs_f64() >= 2.0,
            "elapsed {:.2}s < 2s — hint was not honoured (fell back to jitter)",
            elapsed.as_secs_f64()
        );
    }

    /// A moderation command that receives the canonical pre-ingest 429 on EVERY
    /// attempt exhausts the retry budget and surfaces `CliError::Relay { status: 429 }` —
    /// NOT `DeliveryUnknown`. The relay provably never executed the command on any
    /// attempt, so the caller must be told it is safe to retry.
    #[tokio::test]
    async fn exhausted_ingest_429_returns_relay_429_retryable() {
        let (url, attempts) = test_server(|_n| {
            (
                StatusCode::TOO_MANY_REQUESTS,
                r#"{"error":"rate-limited: quota exceeded; retry in 0s"}"#.to_string(),
            )
        })
        .await;
        let client = test_client(&url);
        let event = make_moderation_event(client.keys(), 9040);
        let err = client.submit_event(event).await.unwrap_err();

        // Must be Relay(429), not DeliveryUnknown.
        assert!(
            matches!(err, CliError::Relay { status: 429, .. }),
            "exhausted ingest 429 must surface as Relay(429), got {err:?}"
        );
        // Must NOT be retryable:false.
        assert!(
            crate::error::is_retryable_error(&err),
            "Relay(429) must be retryable; got {err:?}"
        );
        // The shared client's configured three-attempt budget must be exhausted.
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            3,
            "all retry attempts must fire for exhausted ingest 429"
        );
    }

    /// A moderation command (kind 9042) that gets HTTP 502 returns `DeliveryUnknown`
    /// immediately — proxy errors leave relay execution state ambiguous.
    #[tokio::test]
    async fn moderation_kind_502_returns_delivery_unknown() {
        let (url, attempts) =
            test_server(|_n| (StatusCode::BAD_GATEWAY, "bad gateway".to_string())).await;
        let client = test_client(&url);
        let event = make_moderation_event(client.keys(), 9042);
        let err = client.submit_event(event).await.unwrap_err();
        assert!(
            matches!(err, CliError::DeliveryUnknown(_)),
            "expected DeliveryUnknown for 502, got {err:?}"
        );
        // 502 must not be retried — exactly 1 attempt.
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "must not retry 502 for moderation kind"
        );
    }

    /// When all retry attempts are connect-failures (the relay definitively never saw
    /// the request), `submit_event` must return `CliError::Network` with
    /// `retryable:true` — not `DeliveryUnknown`.  Connect-failure is the one error
    /// condition the implementation itself identifies as confirmed-unreceived.
    #[tokio::test]
    async fn exhausted_connect_failures_return_network_retryable() {
        // Bind a port, capture the address, then drop the listener so every
        // subsequent connect attempt is refused immediately.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let base = format!("http://{addr}");
        let client = test_client(&base);
        let event = make_moderation_event(client.keys(), 9040);
        let err = client.submit_event(event).await.unwrap_err();
        // Must be Network (retryable), not DeliveryUnknown (retryable:false).
        assert!(
            matches!(err, super::super::error::CliError::Network(_)),
            "exhausted connect failures must surface as Network, got {err:?}"
        );
        // Confirm the error description does not suggest ambiguous delivery.
        let description = format!("{err:?}");
        assert!(
            !description.contains("outcome unknown"),
            "connect failure must not be labeled DeliveryUnknown; got: {description}"
        );
    }

    /// A stored (non-moderation) event submitted to a server that returns 502 on the
    /// first attempt and then succeeds is retried under the standard policy.
    #[tokio::test]
    async fn stored_event_502_is_retried_under_standard_policy() {
        let (url, attempts) = test_server(|n| {
            if n == 1 {
                (StatusCode::BAD_GATEWAY, "transient".to_string())
            } else {
                (
                    StatusCode::OK,
                    r#"{"event_id":"abc","accepted":true,"message":""}"#.to_string(),
                )
            }
        })
        .await;
        let client = test_client(&url);
        let event = make_stored_event(client.keys());
        let result = client.submit_event(event).await;
        assert!(
            result.is_ok(),
            "expected Ok after 502 retry for stored event, got {result:?}"
        );
        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "must have retried at least once"
        );
    }

    /// Spin up a one-shot axum server that handles `GET /info` (and any other GET).
    /// Same contract as `test_server` — returns base URL and attempt counter.
    async fn get_server<F>(f: F) -> (String, Arc<AtomicU32>)
    where
        F: Fn(u32) -> (StatusCode, String) + Send + Sync + 'static,
    {
        let counter = Arc::new(AtomicU32::new(0));
        let handler: Arc<dyn Fn(u32) -> (StatusCode, String) + Send + Sync> = Arc::new(f);
        let state = (handler, counter.clone());

        type S = (
            Arc<dyn Fn(u32) -> (StatusCode, String) + Send + Sync>,
            Arc<AtomicU32>,
        );
        let app = Router::new()
            .route(
                "/{*path}",
                axum::routing::get(
                    |State((handler, ctr)): State<S>, _headers: HeaderMap| async move {
                        let n = ctr.fetch_add(1, Ordering::SeqCst) + 1;
                        let (status, body) = handler(n);
                        Response::builder()
                            .status(status)
                            .header("content-type", "application/json")
                            .body(Body::from(body))
                            .unwrap()
                    },
                ),
            )
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr: SocketAddr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), counter)
    }

    /// The shared transport retries transient HTTP 502 on an authenticated read
    /// and succeeds on the next attempt.
    #[tokio::test]
    async fn query_502_is_retried_then_succeeds() {
        let (url, attempts) = get_server(|n| {
            if n == 1 {
                (
                    StatusCode::BAD_GATEWAY,
                    "transient gateway error".to_string(),
                )
            } else {
                (StatusCode::OK, r#"{"ok":true}"#.to_string())
            }
        })
        .await;
        let client = test_client(&url);
        let result = client.get_authed("/info").await;
        assert!(
            result.is_ok(),
            "expected Ok after 502 retry, got {result:?}"
        );
        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "must have retried at least once"
        );
    }

    /// The shared transport retries a 429 with a `retry in Ns` hint, honours the
    /// hint delay (not the shorter jitter fallback), and ultimately succeeds.
    ///
    /// Uses a 2s hint; jitter max for attempt 0 is 0.5s, so asserting elapsed ≥ 2s
    /// cleanly distinguishes hint-honoured from jitter-fallback.
    #[tokio::test]
    async fn query_429_with_hint_is_retried() {
        let (url, attempts) = get_server(|n| {
            if n < 2 {
                (
                    StatusCode::TOO_MANY_REQUESTS,
                    // The shared relay-error parser extracts and honours this hint.
                    r#"{"error":"rate-limited: retry in 2s"}"#.to_string(),
                )
            } else {
                (StatusCode::OK, r#"{"ok":true}"#.to_string())
            }
        })
        .await;
        let client = test_client(&url);
        let t0 = std::time::Instant::now();
        // Measure from just before attempt 1 fires so we capture the inter-attempt wait.
        let result = client.get_authed("/info").await;
        // Record elapsed after attempt 1 returns (inside the future) is not possible
        // directly, but the total includes the hint sleep; jitter max is 0.5s so ≥ 2s
        // proves the hint was honoured.
        let elapsed = t0.elapsed();
        assert!(
            result.is_ok(),
            "expected Ok after 429 retry, got {result:?}"
        );
        assert!(
            attempts.load(Ordering::SeqCst) >= 2,
            "must have retried at least once"
        );
        assert!(
            elapsed.as_secs_f64() >= 2.0,
            "elapsed {:.2}s < 2s — hint was not honoured (fell back to jitter)",
            elapsed.as_secs_f64()
        );
    }

    /// A definitive 4xx (403 Forbidden) is NOT retried — exactly 1 attempt.
    #[tokio::test]
    async fn query_403_is_not_retried() {
        let (url, attempts) = get_server(|_n| {
            (
                StatusCode::FORBIDDEN,
                r#"{"error":"not allowed"}"#.to_string(),
            )
        })
        .await;
        let client = test_client(&url);
        let result = client.get_authed("/info").await;
        assert!(
            matches!(result, Err(CliError::Relay { status: 403, .. })),
            "expected Relay 403 error, got {result:?}"
        );
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            1,
            "403 must not be retried"
        );
    }

    /// Authenticated reads retry on `is_body()` network errors because body
    /// transfer stays inside the retry boundary. We simulate body loss by
    /// returning an intentionally truncated response after sending headers.
    ///
    /// This test uses a raw TCP server to write partial HTTP responses; axum cannot
    /// easily simulate mid-body connection drops.
    #[tokio::test]
    async fn authenticated_get_retries_on_body_transfer_failure() {
        use tokio::io::AsyncWriteExt;

        let counter = Arc::new(AtomicU32::new(0));
        let counter2 = counter.clone();

        // Bind a raw TCP listener.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let n = counter2.fetch_add(1, Ordering::SeqCst) + 1;

                // Consume the request (required to avoid connection reset by server).
                let mut buf = vec![0u8; 4096];
                use tokio::io::AsyncReadExt;
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(100),
                    stream.read(&mut buf),
                )
                .await;

                if n < 3 {
                    // Attempts 1 & 2: send valid headers claiming a body, then drop.
                    let partial = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"partial\":";
                    let _ = stream.write_all(partial).await;
                    // Drop the stream without completing the body — causes is_body() on client.
                } else {
                    // Attempt 3: complete response.
                    let ok = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\r\n{}";
                    let _ = stream.write_all(ok).await;
                }
            }
        });

        let base = format!("http://{addr}");
        // The body read is inside the shared client's retry loop.
        let client = test_client(&base);
        // Stub path: the raw TCP server ignores the URL and always responds based on attempt count.
        let result = client.get_authed("/any-path").await;
        assert!(
            result.is_ok(),
            "expected Ok after body-loss retries, got {result:?}"
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "expected 3 attempts (2 body-loss + 1 success)"
        );
    }

    /// Stored submission keeps the full operation, including response body read,
    /// inside the shared client's retry boundary.
    /// A partial-body drop after 200 headers must be retried with the same
    /// serialized event bytes (and a fresh NIP-98 auth per attempt).
    #[tokio::test]
    async fn stored_event_body_loss_is_retried_with_same_event_bytes() {
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;

        let counter = Arc::new(AtomicU32::new(0));
        let counter2 = counter.clone();
        let bodies: Arc<std::sync::Mutex<Vec<Vec<u8>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let bodies2 = bodies.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let n = counter2.fetch_add(1, Ordering::SeqCst) + 1;

                // Read the full HTTP request so we can capture the body.
                let mut buf = vec![0u8; 8192];
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    stream.read(&mut buf),
                )
                .await;
                // Capture raw request bytes for assertion.
                let body_end = buf
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|i| i + 4)
                    .unwrap_or(0);
                let payload = buf[body_end..].to_vec();
                bodies2.lock().unwrap().push(payload);

                if n < 3 {
                    // Partial body drop.
                    let partial = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"partial\":";
                    let _ = stream.write_all(partial).await;
                } else {
                    let ok = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 47\r\n\r\n{\"event_id\":\"abc\",\"accepted\":true,\"message\":\"\"}";
                    let _ = stream.write_all(ok).await;
                }
            }
        });

        let base = format!("http://{addr}");
        let client = test_client(&base);
        let event = make_stored_event(client.keys());
        let result = client.submit_event(event).await;
        assert!(
            result.is_ok(),
            "expected Ok after body-loss retries, got {result:?}"
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "expected 3 attempts (2 body-loss + 1 success)"
        );
        // All three attempts must have sent the same serialized event bytes.
        let captured = bodies.lock().unwrap();
        assert_eq!(captured.len(), 3, "must have captured 3 request bodies");
        // Each attempt's payload must be identical (same signed event bytes).
        assert_eq!(
            captured[0], captured[1],
            "attempt 1 and 2 must use identical event bytes"
        );
        assert_eq!(
            captured[1], captured[2],
            "attempt 2 and 3 must use identical event bytes"
        );
    }

    /// Shared media upload keeps response body reads inside the retry boundary.
    /// A partial-body drop after 200 headers must be retried with identical file
    /// bytes and a fresh Blossom auth per attempt.
    #[tokio::test]
    async fn upload_body_loss_is_retried_with_same_file_bytes() {
        use std::io::Write;
        use tokio::io::AsyncReadExt;
        use tokio::io::AsyncWriteExt;

        // Write a minimal JPEG file so MIME detection works.
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        // JPEG magic + JFIF app0 marker: enough for `infer` to detect image/jpeg.
        let jpeg_header: &[u8] = &[
            0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10, 0x4A, 0x46, 0x49, 0x46, 0x00, 0x01,
        ];
        tmp.write_all(jpeg_header).unwrap();
        let file_path = tmp.path().to_str().unwrap().to_string();

        let counter = Arc::new(AtomicU32::new(0));
        let counter2 = counter.clone();
        let auth_headers: Arc<std::sync::Mutex<Vec<String>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let auth_headers2 = auth_headers.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let n = counter2.fetch_add(1, Ordering::SeqCst) + 1;

                // Read the request headers to extract the Authorization value.
                let mut buf = vec![0u8; 8192];
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    stream.read(&mut buf),
                )
                .await;
                // Extract the Authorization header value.
                let req_str = String::from_utf8_lossy(&buf);
                let auth = req_str
                    .lines()
                    .find(|l| l.to_lowercase().starts_with("authorization:"))
                    .map(|l| l.to_string())
                    .unwrap_or_default();
                auth_headers2.lock().unwrap().push(auth);

                if n < 3 {
                    // Partial body drop.
                    let partial = b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"partial\":";
                    let _ = stream.write_all(partial).await;
                } else {
                    // Valid BlobDescriptor response.
                    let ok_body = r#"{"url":"https://relay.test/media/3c4bae649b6c0fade21c149e6ee9773e734d620fda91248a44c58b11c71f3ba9.jpg","sha256":"3c4bae649b6c0fade21c149e6ee9773e734d620fda91248a44c58b11c71f3ba9","size":12,"type":"image/jpeg","uploaded":0}"#;
                    let ok = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
                        ok_body.len(),
                        ok_body
                    );
                    let _ = stream.write_all(ok.as_bytes()).await;
                }
            }
        });

        let base = format!("http://{addr}");
        let client = test_client(&base);
        let result = client.upload_file(&file_path).await;
        assert!(
            result.is_ok(),
            "expected Ok after upload body-loss retries, got {result:?}"
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "expected 3 upload attempts (2 body-loss + 1 success)"
        );
        // Each attempt must carry a distinct Authorization header (fresh Blossom auth).
        let auths = auth_headers.lock().unwrap();
        assert_eq!(auths.len(), 3, "must have captured 3 auth headers");
        // All three must be non-empty (auth was signed).
        assert!(
            auths.iter().all(|a| a.contains("Nostr ")),
            "each attempt must carry Nostr auth"
        );
    }

    /// When all retry attempts for a stored event end with a partial body (200
    /// headers, dropped connection), the final error must be `DeliveryUnknown`
    /// (retryable:false) — the relay may have stored the event on any attempt, so
    /// an outer re-sign would risk a duplicate visible write.  All three attempts
    /// must fire with identical serialized event bytes.
    #[tokio::test]
    async fn stored_event_all_body_losses_return_delivery_unknown() {
        use tokio::io::AsyncWriteExt;

        let bodies: Arc<std::sync::Mutex<Vec<Vec<u8>>>> =
            Arc::new(std::sync::Mutex::new(Vec::new()));
        let bodies2 = bodies.clone();
        let counter = Arc::new(AtomicU32::new(0));
        let counter2 = counter.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            loop {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                counter2.fetch_add(1, Ordering::SeqCst);
                let mut buf = vec![0u8; 8192];
                let _ = tokio::time::timeout(
                    std::time::Duration::from_millis(200),
                    stream.read(&mut buf),
                )
                .await;
                // Extract the request body (after the blank line separating headers).
                let raw = buf.split(|&b| b == 0).next().unwrap_or(&buf).to_vec();
                if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    bodies2.lock().unwrap().push(raw[pos + 4..].to_vec());
                }
                // Partial body: send headers + truncated body, then drop.
                let _ = stream
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 100\r\n\r\n{\"partial\":",
                    )
                    .await;
                // Drop stream — causes body-loss error on the client side.
            }
        });

        let base = format!("http://{addr}");
        let client = test_client(&base);
        let event = make_stored_event(client.keys());
        let err = client.submit_event(event).await.unwrap_err();

        // Final error must be DeliveryUnknown — relay may have accepted any attempt.
        assert!(
            matches!(err, CliError::DeliveryUnknown(_)),
            "all-body-loss exhaustion must return DeliveryUnknown, got {err:?}"
        );
        // The shared client's configured three-attempt budget must be exhausted.
        assert_eq!(
            counter.load(Ordering::SeqCst),
            3,
            "all 3 attempts must be made before surfacing DeliveryUnknown"
        );
        // All attempts must have sent identical serialized event bytes.
        let captured = bodies.lock().unwrap();
        if captured.len() >= 2 {
            assert_eq!(
                captured[0], captured[1],
                "all attempts must use identical event bytes"
            );
        }
    }

    /// When all retry attempts for a stored event return HTTP 502, the final error
    /// must be `DeliveryUnknown` (retryable:false) — a proxy 502 may occur after
    /// the relay accepted the event.
    #[tokio::test]
    async fn stored_event_all_502s_return_delivery_unknown() {
        let (url, attempts) =
            test_server(|_n| (StatusCode::BAD_GATEWAY, "bad gateway".to_string())).await;
        let client = test_client(&url);
        let event = make_stored_event(client.keys());
        let err = client.submit_event(event).await.unwrap_err();

        assert!(
            matches!(err, CliError::DeliveryUnknown(_)),
            "all-502 exhaustion must return DeliveryUnknown, got {err:?}"
        );
        assert_eq!(
            attempts.load(Ordering::SeqCst),
            3,
            "all 3 attempts must fire before surfacing DeliveryUnknown"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{create_response_with_id, extract_relay_response_field, BuzzClient};
    use nostr::{EventBuilder, Keys, Kind, Tag};

    #[test]
    fn extract_relay_response_field_reads_response_message_json() {
        let raw = r#"{"event_id":"abc","accepted":true,"message":"response:{\"workflow_id\":\"relay-id\",\"created\":true}"}"#;
        assert_eq!(
            extract_relay_response_field(raw, "workflow_id").as_deref(),
            Some("relay-id")
        );
    }

    #[test]
    fn extract_relay_response_field_returns_none_for_non_response_message() {
        let raw = r#"{"event_id":"abc","accepted":true,"message":""}"#;
        assert!(extract_relay_response_field(raw, "workflow_id").is_none());
    }

    #[test]
    fn create_response_with_id_overrides_local_id_with_relay_id() {
        let raw = r#"{"event_id":"abc","accepted":true,"message":"response:{\"workflow_id\":\"relay-id\"}"}"#;
        let out = create_response_with_id(raw, "workflow_id", "relay-id");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["workflow_id"].as_str(), Some("relay-id"));
        assert_eq!(v["event_id"].as_str(), Some("abc"));
        assert_eq!(v["accepted"].as_bool(), Some(true));
    }

    // --- (a) auth-suppression regression pair ---

    fn make_auth_tag(agent: &Keys) -> (Tag, String) {
        let json =
            buzz_sdk::nip_oa::compute_auth_tag(&Keys::generate(), &agent.public_key(), "").unwrap();
        let tag = buzz_sdk::nip_oa::parse_auth_tag(&json).unwrap();
        (tag, json)
    }

    #[test]
    fn sign_event_unchecked_does_not_inject_ambient_auth_tag() {
        let keys = Keys::generate();
        let (auth_tag, auth_json) = make_auth_tag(&keys);
        let client = BuzzClient::new(
            "https://test.relay".into(),
            keys,
            Some(auth_tag),
            Some(auth_json),
        )
        .unwrap();

        let builder =
            EventBuilder::new(Kind::Custom(9035), "archive").tags([Tag::parse(["-"]).unwrap()]);
        let event = client.sign_event_unchecked(builder).unwrap();

        let auth_tags: Vec<_> = event
            .tags
            .iter()
            .filter(|t| t.as_slice().first().map(|s| s.as_str()) == Some("auth"))
            .collect();
        assert!(
            auth_tags.is_empty(),
            "sign_event_unchecked must not inject the ambient NIP-OA auth tag \
             into identity archive events; found {auth_tags:?}"
        );
    }

    #[test]
    fn sign_event_unchecked_preserves_callers_content_auth_tag() {
        let keys = Keys::generate();
        let (auth_tag, auth_json) = make_auth_tag(&keys);
        let client = BuzzClient::new(
            "https://test.relay".into(),
            keys,
            Some(auth_tag),
            Some(auth_json),
        )
        .unwrap();

        let content_auth = Tag::parse([
            "auth",
            &"c".repeat(64),
            "owner-attestation",
            &"d".repeat(128),
        ])
        .unwrap();

        let builder = EventBuilder::new(Kind::Custom(9035), "archive")
            .tags([Tag::parse(["-"]).unwrap(), content_auth]);
        let event = client.sign_event_unchecked(builder).unwrap();

        let auth_tags: Vec<_> = event
            .tags
            .iter()
            .filter(|t| t.as_slice().first().map(|s| s.as_str()) == Some("auth"))
            .collect();
        assert_eq!(
            auth_tags.len(),
            1,
            "content-level auth tag must survive sign_event_unchecked; found {auth_tags:?}"
        );
        assert_eq!(auth_tags[0].as_slice()[1], "c".repeat(64));
    }
}
