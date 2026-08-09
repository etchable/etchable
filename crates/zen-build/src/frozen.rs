//! Dep-less frozen resolution — the replacement for `pcb-zen`'s resolver
//! (docs/decisions/0005).
//!
//! Etchable workspaces are self-contained: workspace packages plus the
//! bundled/vendored stdlib, never remote `[dependencies]` (git-fetched
//! packages). That constraint lets this resolver be ~150 lines of pure
//! filesystem logic over `pcb-zen-core`'s public types — and dropping the
//! `pcb-zen` crate removes `rusqlite`/`libsqlite3-sys` (its remote-package
//! cache index) from the binary, freeing the one `links = "sqlite3"` slot
//! for the app's own sqlx/sea-orm state store.
//!
//! Faithful to upstream `package_resolver/resolve.rs` for everything kept;
//! a manifest that declares `[dependencies]` fails with a clear error
//! instead of resolving.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use pcb_zen_core::config::{ManifestPart, PcbToml};
use pcb_zen_core::resolution::{
    build_package_roots, selected_remote_from_hydrated_manifest, FrozenPackage,
    FrozenPackageIdentity, FrozenResolutionMap, FrozenResolutionSet, ResolutionResult,
};
use pcb_zen_core::workspace::WorkspaceInfo;
use pcb_zen_core::{is_stdlib_module_path, DefaultFileProvider, STDLIB_MODULE_PATH};

/// Upstream's URL for a workspace with no package manifests.
const STANDALONE_PACKAGE_URL: &str = "workspace";

/// Best-effort canonicalization (matches upstream: unresolvable paths pass
/// through untouched).
fn canonicalize(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn resolve(workspace_info: WorkspaceInfo, path: &Path) -> Result<ResolutionResult> {
    let package_urls = target_package_urls(&workspace_info, path)?;
    if package_urls.is_empty() {
        bail!(
            "no workspace package target found for {}",
            path.display()
        );
    }

    let mut resolution_set = FrozenResolutionSet::new();
    let mut symbol_parts: HashMap<String, Vec<ManifestPart>> = HashMap::new();
    for url in package_urls {
        let map = if is_stdlib_module_path(&url) {
            stdlib_resolution_map(&workspace_info)
        } else {
            package_resolution_map(&workspace_info, &url)
                .with_context(|| format!("while resolving {url}"))?
        };
        symbol_parts.extend(frozen_symbol_parts(&workspace_info, &map));
        resolution_set.insert(url, map);
    }

    Ok(ResolutionResult::frozen(
        workspace_info,
        resolution_set,
        symbol_parts,
    ))
}

fn stdlib_frozen_package() -> FrozenPackage {
    FrozenPackage {
        identity: FrozenPackageIdentity::Stdlib,
        deps: BTreeMap::new(),
        parts: Vec::new(),
    }
}

fn stdlib_resolution_map(ws: &WorkspaceInfo) -> FrozenResolutionMap {
    FrozenResolutionMap {
        selected_remote: BTreeMap::new(),
        packages: BTreeMap::from([(
            canonicalize(&ws.workspace_stdlib_dir()),
            stdlib_frozen_package(),
        )]),
    }
}

/// One workspace package + the stdlib. Any remote dependency surface —
/// a `[dependencies]` table or a hydrated closure — is a hard error.
fn package_resolution_map(ws: &WorkspaceInfo, url: &str) -> Result<FrozenResolutionMap> {
    let (package_root, config) = workspace_manifest(ws, url)?;

    let selected = selected_remote_from_hydrated_manifest(ws, url)
        .with_context(|| format!("while reading the dependency closure for {url}"))?;
    if !selected.is_empty() || !config.dependencies.direct.is_empty() {
        bail!(
            "pcb.toml declares package [dependencies] — remote packages are not \
             supported in etchable (projects are self-contained: the stdlib plus \
             vendored components). Remove the [dependencies] table or vendor the \
             code into the project."
        );
    }

    let mut packages = BTreeMap::from([(
        canonicalize(&package_root),
        FrozenPackage {
            identity: FrozenPackageIdentity::Workspace(url.to_string()),
            deps: BTreeMap::new(),
            parts: config.parts.clone(),
        },
    )]);
    packages.insert(
        canonicalize(&ws.workspace_stdlib_dir()),
        stdlib_frozen_package(),
    );

    Ok(FrozenResolutionMap {
        selected_remote: BTreeMap::new(),
        packages,
    })
}

/// Mirror of upstream's `workspace_manifest`.
fn workspace_manifest(ws: &WorkspaceInfo, url: &str) -> Result<(PathBuf, PcbToml)> {
    if ws.packages.is_empty() && url == STANDALONE_PACKAGE_URL {
        return Ok((ws.root.clone(), ws.config.clone().unwrap_or_default()));
    }
    if let Some(pkg) = ws.packages.get(url) {
        return Ok((pkg.dir(&ws.root), pkg.config.clone()));
    }
    if ws.workspace_base_url().as_deref() == Some(url) {
        if let Some(config) = ws.config.clone() {
            return Ok((ws.root.clone(), config));
        }
    }
    bail!("unknown workspace package: {url}")
}

/// Mirror of upstream's `target_package_urls_for_path`, minus the
/// walk-every-zen-file fallback: for an unrecognized directory we resolve
/// every workspace package, a superset that evaluates identically.
fn target_package_urls(ws: &WorkspaceInfo, path: &Path) -> Result<Vec<String>> {
    let path = path
        .canonicalize()
        .with_context(|| format!("no such path: {}", path.display()))?;

    // The materialized stdlib lives inside the workspace tree but is never
    // a workspace package.
    if path.starts_with(canonicalize(&ws.workspace_stdlib_dir())) {
        return Ok(vec![STDLIB_MODULE_PATH.to_string()]);
    }
    if ws.packages.is_empty() {
        return Ok(vec![STANDALONE_PACKAGE_URL.to_string()]);
    }

    let provider = DefaultFileProvider::new();
    if path.is_file() {
        return match ws.package_url_for_path(&provider, &path) {
            Some(url) => Ok(vec![url.to_string()]),
            None => bail!("no workspace package contains {}", path.display()),
        };
    }
    if path == canonicalize(&ws.root) {
        return Ok(ws.packages.keys().cloned().collect());
    }
    if let Some(url) = ws.package_url_for_path(&provider, &path) {
        return Ok(vec![url.to_string()]);
    }
    Ok(ws.packages.keys().cloned().collect())
}

/// Mirror of upstream's `build_frozen_symbol_parts` +
/// `add_parts_to_symbol_map`: `[parts]` manifest entries keyed by the
/// `package://` URI of their `.kicad_sym`.
fn frozen_symbol_parts(
    ws: &WorkspaceInfo,
    map: &FrozenResolutionMap,
) -> HashMap<String, Vec<ManifestPart>> {
    let mut result: HashMap<String, Vec<ManifestPart>> = HashMap::new();
    let package_roots = build_package_roots(ws, map.packages.values().map(|p| &p.deps));
    for (pkg_root, package) in &map.packages {
        for part in &package.parts {
            let abs_symbol = pkg_root.join(&part.symbol);
            match pcb_sch::format_package_uri(&abs_symbol, &package_roots) {
                Some(uri) => result.entry(uri).or_default().push(part.clone()),
                None => tracing::warn!(
                    "could not resolve symbol path '{}' in {} to a package URI",
                    part.symbol,
                    pkg_root.display()
                ),
            }
        }
    }
    result
}
