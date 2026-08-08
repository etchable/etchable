//! Part search over the local library corpus (docs/decisions/0003, 0004).
//!
//! Works entirely offline: stdlib generics, vendored KiCad symbols, and
//! project components. The Diode-registry tier (a `pcb` CLI subprocess
//! behind a browser login) is gone — real parts come from the LCSC tier
//! added in decision 0004.

use std::path::Path;

use serde_json::{json, Value};
use zen_build::{LibraryListing, ProjectDoc};

const MAX_LOCAL: usize = 25;

/// One local hit: what it is and how to use it.
fn score(haystack: &str, tokens: &[String]) -> usize {
    let hay = haystack.to_ascii_lowercase();
    tokens
        .iter()
        .filter(|t| hay.contains(t.as_str()))
        .map(|t| t.len())
        .sum()
}

pub fn local_matches(listing: &LibraryListing, query: &str) -> Vec<Value> {
    let tokens: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    let mut hits: Vec<(usize, Value)> = Vec::new();

    for g in &listing.generics {
        let s = score(&g.name, &tokens);
        if s > 0 {
            hits.push((
                s,
                json!({
                    "kind": "generic",
                    "name": g.name,
                    "use": format!("Module(\"@stdlib/generics/{}.zen\")", g.name),
                    "params": g.params,
                }),
            ));
        }
    }
    for c in &listing.project_components {
        let hay = format!(
            "{} {} {}",
            c.name,
            c.description.clone().unwrap_or_default(),
            c.mpn.clone().unwrap_or_default()
        );
        let s = score(&hay, &tokens);
        if s > 0 {
            hits.push((
                s,
                json!({
                    "kind": "project_component",
                    "name": c.name,
                    "mpn": c.mpn,
                    "lcsc": c.lcsc,
                    "use": format!("Module(\"./components/{}.zen\")", c.name),
                }),
            ));
        }
    }

    hits.sort_by(|a, b| b.0.cmp(&a.0));
    hits.into_iter().take(MAX_LOCAL).map(|(_, v)| v).collect()
}

pub async fn search_parts(
    stdlib_dir: &Path,
    project: Option<&ProjectDoc>,
    query: &str,
) -> Value {
    let listing = zen_build::list_library(stdlib_dir, project, None);
    json!({ "local": local_matches(&listing, query) })
}
