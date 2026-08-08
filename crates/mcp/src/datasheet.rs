//! `fetch_datasheet` (docs/decisions/0003) — the sanctioned way to get a
//! PDF into a project, so datasheet retrieval never needs a shell.
//! https-only, size- and type-capped, written to the project's conventional
//! `datasheets/<component>.pdf` path (which the card layer then picks up).

use std::path::Path;

use anyhow::{bail, Context, Result};

const MAX_BYTES: usize = 25 * 1024 * 1024;
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

pub fn valid_component_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

pub struct Fetched {
    pub path: String,
    pub bytes: usize,
    pub already_existed: bool,
}

pub async fn fetch_datasheet(project_root: &Path, url: &str, component: &str) -> Result<Fetched> {
    if !valid_component_name(component) {
        bail!("invalid component name {component:?}");
    }
    if !url.starts_with("https://") {
        bail!("datasheet URL must be https");
    }

    let dir = project_root.join("datasheets");
    let dest = dir.join(format!("{component}.pdf"));
    let rel = format!("datasheets/{component}.pdf");
    if dest.is_file() {
        return Ok(Fetched {
            path: rel,
            bytes: dest.metadata().map(|m| m.len() as usize).unwrap_or(0),
            already_existed: true,
        });
    }

    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .context("http client")?;
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("fetching {url}"))?;
    if !resp.status().is_success() {
        bail!("{url} returned HTTP {}", resp.status());
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if let Some(len) = resp.content_length() {
        if len as usize > MAX_BYTES {
            bail!("datasheet is {len} bytes; the cap is {MAX_BYTES}");
        }
    }
    let body = resp.bytes().await.context("reading response body")?;
    if body.len() > MAX_BYTES {
        bail!("datasheet is {} bytes; the cap is {MAX_BYTES}", body.len());
    }
    let looks_pdf = content_type.contains("pdf") || body.starts_with(b"%PDF");
    if !looks_pdf {
        bail!("that URL did not return a PDF (content-type {content_type:?})");
    }

    std::fs::create_dir_all(&dir).context("creating datasheets/")?;
    std::fs::write(&dest, &body).with_context(|| format!("writing {}", dest.display()))?;
    Ok(Fetched {
        path: rel,
        bytes: body.len(),
        already_existed: false,
    })
}
