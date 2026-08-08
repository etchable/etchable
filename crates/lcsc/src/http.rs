//! HTTP client policy: honest UA, serial gate with a 250 ms + jitter
//! spacing, one retry on 429/5xx, and a persisted 30-minute circuit breaker
//! on 403 (a CloudFront ban outlives polite backoff — retrying makes it
//! worse). Endpoints are consts with env overrides because the STEP bucket
//! prefix is undocumented and could rotate.

use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, Context};
use tokio::sync::Mutex;

/// The literal `Mozilla/5.0` is WAF-blocklisted; an honest product UA passes.
fn user_agent() -> String {
    format!(
        "etchable/{} (+https://github.com/fcjr/etchable)",
        env!("CARGO_PKG_VERSION")
    )
}

fn env_or(var: &str, default: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| default.to_string())
}

pub fn easyeda_api_base() -> String {
    env_or("ETCHABLE_EASYEDA_API", "https://easyeda.com/api")
}

pub fn modules_bases() -> [String; 2] {
    [
        env_or("ETCHABLE_EASYEDA_MODULES", "https://modules.easyeda.com"),
        env_or("ETCHABLE_EASYEDA_MODULES_MIRROR", "https://modules.lceda.cn"),
    ]
}

/// Fixed OSS bucket prefix for STEP downloads — community-discovered,
/// stable for years, undocumented; hence the override.
pub fn step_bucket_prefix() -> String {
    env_or("ETCHABLE_EASYEDA_STEP_PREFIX", "qAxj6KHrDKw4blvCG8QJPs7Y")
}

pub fn jlc_search_url() -> String {
    env_or(
        "ETCHABLE_JLC_SEARCH_URL",
        "https://jlcpcb.com/api/overseas-pcb-order/v1/shoppingCart/smtGood/selectSmtComponentList",
    )
}

pub fn jlc_detail_url() -> String {
    env_or(
        "ETCHABLE_JLC_DETAIL_URL",
        "https://cart.jlcpcb.com/shoppingCart/smtGood/getComponentDetail",
    )
}

const MIN_SPACING: Duration = Duration::from_millis(250);
const BREAKER_SECS: u64 = 30 * 60;

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("network access is disabled (ETCHABLE_LCSC_OFFLINE=1); only cached and local parts are available")]
    Offline,
    #[error("the part service blocked us (HTTP 403); backing off for {retry_after_secs}s — cached and local parts still work")]
    Blocked { retry_after_secs: u64 },
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub struct Client {
    http: reqwest::Client,
    /// Serial gate: at most one in-flight request, spaced by MIN_SPACING.
    gate: Mutex<Option<Instant>>,
    /// Circuit-breaker file holding a unix expiry timestamp. Persisted so a
    /// ban survives app restarts.
    breaker_path: PathBuf,
}

impl Client {
    pub fn new(cache_root: &std::path::Path) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .user_agent(user_agent())
            .timeout(Duration::from_secs(30))
            .build()
            .context("building http client")?;
        Ok(Self {
            http,
            gate: Mutex::new(None),
            breaker_path: cache_root.join("breaker"),
        })
    }

    /// Seconds until the breaker clears, if tripped.
    pub fn breaker_remaining(&self) -> Option<u64> {
        let text = std::fs::read_to_string(&self.breaker_path).ok()?;
        let until: u64 = text.trim().parse().ok()?;
        let now = SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs();
        (until > now).then(|| until - now)
    }

    fn trip_breaker(&self) {
        let until = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() + BREAKER_SECS)
            .unwrap_or(BREAKER_SECS);
        let _ = std::fs::write(&self.breaker_path, until.to_string());
    }

    async fn wait_turn(&self) {
        let mut last = self.gate.lock().await;
        if let Some(prev) = *last {
            let jitter = Duration::from_millis(
                (SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.subsec_nanos())
                    .unwrap_or(0)
                    % 100) as u64,
            );
            let spacing = MIN_SPACING + jitter;
            let elapsed = prev.elapsed();
            if elapsed < spacing {
                tokio::time::sleep(spacing - elapsed).await;
            }
        }
        *last = Some(Instant::now());
    }

    /// One request with the full policy applied. `body`: None = GET,
    /// Some((content_type, bytes)) = POST.
    pub async fn request(
        &self,
        url: &str,
        body: Option<(&str, Vec<u8>)>,
    ) -> Result<Vec<u8>, FetchError> {
        if crate::offline() {
            return Err(FetchError::Offline);
        }
        if let Some(secs) = self.breaker_remaining() {
            return Err(FetchError::Blocked { retry_after_secs: secs });
        }

        let mut attempt = 0;
        loop {
            self.wait_turn().await;
            let req = match &body {
                None => self.http.get(url),
                Some((ct, bytes)) => self
                    .http
                    .post(url)
                    .header(reqwest::header::CONTENT_TYPE, *ct)
                    .body(bytes.clone()),
            };
            let resp = req.send().await.map_err(|e| anyhow!("{url}: {e}"))?;
            let status = resp.status();

            if status == reqwest::StatusCode::FORBIDDEN {
                self.trip_breaker();
                tracing::warn!("403 from {url}; circuit breaker tripped for {BREAKER_SECS}s");
                return Err(FetchError::Blocked { retry_after_secs: BREAKER_SECS });
            }
            if status == reqwest::StatusCode::NOT_FOUND {
                return Err(FetchError::NotFound(url.to_string()));
            }
            let retryable = status == reqwest::StatusCode::TOO_MANY_REQUESTS || status.is_server_error();
            if retryable && attempt == 0 {
                attempt = 1;
                tracing::debug!("{status} from {url}; retrying once");
                tokio::time::sleep(Duration::from_millis(750)).await;
                continue;
            }
            if !status.is_success() {
                return Err(anyhow!("{url}: HTTP {status}").into());
            }

            let bytes = resp.bytes().await.map_err(|e| anyhow!("{url}: {e}"))?.to_vec();
            return Ok(maybe_gunzip(bytes)?);
        }
    }
}

/// Some EasyEDA paths return raw gzip regardless of Accept-Encoding; sniff
/// the magic instead of trusting headers.
pub fn maybe_gunzip(bytes: Vec<u8>) -> anyhow::Result<Vec<u8>> {
    if bytes.len() >= 2 && bytes[0] == 0x1f && bytes[1] == 0x8b {
        use std::io::Read;
        let mut out = Vec::new();
        flate2::read::GzDecoder::new(bytes.as_slice())
            .read_to_end(&mut out)
            .context("gunzip failed")?;
        return Ok(out);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gunzip_sniffs_magic() {
        use std::io::Write;
        let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(b"{\"ok\":true}").unwrap();
        let gz = enc.finish().unwrap();
        assert_eq!(maybe_gunzip(gz).unwrap(), b"{\"ok\":true}");
        // Plain bytes pass through untouched.
        assert_eq!(maybe_gunzip(b"plain".to_vec()).unwrap(), b"plain");
    }

    #[tokio::test]
    async fn tripped_breaker_short_circuits_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let client = Client::new(dir.path()).unwrap();
        assert!(client.breaker_remaining().is_none());
        client.trip_breaker();
        let secs = client.breaker_remaining().expect("breaker set");
        assert!(secs > BREAKER_SECS - 60 && secs <= BREAKER_SECS);
        // No server exists at this URL; a short-circuit is the only way
        // this returns instantly. Under ETCHABLE_LCSC_OFFLINE=1 (CI) the
        // offline gate fires first — also a short-circuit, also correct.
        match client.request("http://127.0.0.1:1/never", None).await {
            Err(FetchError::Blocked { .. }) => {}
            Err(FetchError::Offline) if crate::offline() => {}
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    #[test]
    fn honest_user_agent_never_spoofs_a_browser() {
        assert!(!user_agent().contains("Mozilla"));
        assert!(user_agent().starts_with("etchable/"));
    }
}
