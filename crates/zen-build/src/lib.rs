//! zen-build: the eval pipeline from diodeinc/pcb re-hosted as a library.
//!
//! resolve -> eval -> electrical checks -> schematic -> ERC -> diagnostics
//! passes, with all `pcb_*` types converted to plain serde data at the
//! boundary ([`model`]). Pin the pcb-* git tag deliberately; these are
//! internal APIs with no stability promise.

mod circuit_json;
mod convert;
mod model;
mod pipeline;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use circuit_json::{to_circuit_json, CircuitJsonDoc};
pub use model::*;

/// An opened .zen workspace with resolved dependencies.
///
/// Cheap to `build_file` repeatedly (the watch loop does); call [`Workspace::reload`]
/// when `pcb.toml` changes so dependency resolution is redone.
pub struct Workspace {
    eval: pipeline::EvalState,
    root: PathBuf,
    offline: bool,
}

impl Workspace {
    /// Discover the workspace containing `path` (a .zen file or directory)
    /// and resolve its dependencies. `offline` skips network fetches and
    /// fails if a remote dependency is not already cached or vendored.
    pub fn open(path: &Path, offline: bool) -> Result<Self> {
        let start = if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let start = start
            .canonicalize()
            .with_context(|| format!("no such path: {}", start.display()))?;
        let resolution = pipeline::resolve(&start, offline)?;
        let root = pipeline::workspace_root(&resolution);
        Ok(Self {
            eval: pipeline::EvalState::new(resolution),
            root,
            offline,
        })
    }

    /// Workspace root (the directory containing `pcb.toml`, or the fallback
    /// root chosen by discovery).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Re-run workspace discovery + dependency resolution. Call on
    /// `pcb.toml` changes.
    pub fn reload(&mut self) -> Result<()> {
        let resolution = pipeline::resolve(&self.root.clone(), self.offline)?;
        self.root = pipeline::workspace_root(&resolution);
        self.eval = pipeline::EvalState::new(resolution);
        Ok(())
    }

    /// Build one .zen file: full pipeline, returns plain-data output.
    /// Evaluation failures are reported through `BuildOutput::diagnostics`,
    /// not `Err` — `Err` is reserved for infrastructure problems.
    pub fn build_file(
        &self,
        zen_path: &Path,
        inputs: &BTreeMap<String, serde_json::Value>,
    ) -> Result<BuildOutput> {
        let zen_path = zen_path
            .canonicalize()
            .with_context(|| format!("no such file: {}", zen_path.display()))?;

        let mut input_map = starlark::collections::SmallMap::new();
        for (k, v) in inputs {
            input_map.insert(k.clone(), v.clone());
        }

        let raw = self.eval.build(&zen_path, input_map);

        let source = zen_path
            .strip_prefix(&self.root)
            .unwrap_or(&zen_path)
            .display()
            .to_string();
        let schematic = raw
            .schematic
            .map(|mut s| convert::convert_schematic(&mut s, &self.root));
        let diagnostics = convert::convert_diagnostics(&raw.diagnostics, &self.root);

        Ok(BuildOutput {
            source,
            schematic,
            diagnostics,
        })
    }
}
