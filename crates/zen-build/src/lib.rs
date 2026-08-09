//! zen-build: the eval pipeline from diodeinc/pcb re-hosted as a library.
//!
//! resolve -> eval -> electrical checks -> schematic -> ERC -> diagnostics
//! passes, with all `pcb_*` types converted to plain serde data at the
//! boundary ([`model`]). Pin the pcb-* git tag deliberately; these are
//! internal APIs with no stability promise.

mod catalog;
mod circuit_json;
mod convert;
mod frozen;
mod layout;
mod layout_check;
mod model;
mod pipeline;
mod positions;
mod project;
mod route;
mod scaffold;
mod symbol_geom;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub use circuit_json::{to_circuit_json, CircuitJsonDoc};
pub use layout_check::{check_layout, LayoutProblem, LayoutReport};
pub use model::*;
pub use catalog::{
    list_library, resolve_library_path, symbol_pins, FootprintLibraryInfo, GenericInfo,
    LibraryListing, ProjectComponentInfo, SymbolLibraryInfo, SymbolPinInfo, SymbolPins,
};
pub use positions::{content_hash, write_positions};
pub use scaffold::{
    add_component, install_component, AddComponentRequest, AddComponentResult, ExtraAsset,
    InstallComponentRequest,
};
pub use project::{
    load_project, resolve_parts, scaffold_project, scaffold_project_detailed, ComponentCard,
    PartFields, ProjectDoc, ResolvedPart, ScaffoldResult, VendorSel, ETCH_MANIFEST,
};

/// How to open a workspace.
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// Skip network fetches; fail if a remote dependency isn't cached or
    /// vendored. Etchable projects declare no dependencies, so this is the
    /// normal mode.
    pub offline: bool,
    /// Explicit stdlib source directory (one containing `pcb.toml`). When
    /// set, it is materialized into `<root>/.pcb/stdlib` and upstream's
    /// exe-ancestor discovery is bypassed — the packaged-app path, where
    /// walking up from the executable can never find `lib/std`.
    pub stdlib_source: Option<PathBuf>,
}

/// An opened .zen workspace with resolved dependencies.
///
/// Cheap to `build_file` repeatedly (the watch loop does); call [`Workspace::reload`]
/// when `pcb.toml` changes so dependency resolution is redone.
pub struct Workspace {
    eval: pipeline::EvalState,
    root: PathBuf,
    opts: OpenOptions,
}

impl Workspace {
    /// Discover the workspace containing `path` (a .zen file or directory)
    /// and resolve its dependencies. `offline` skips network fetches and
    /// fails if a remote dependency is not already cached or vendored.
    pub fn open(path: &Path, offline: bool) -> Result<Self> {
        Self::open_with(
            path,
            &OpenOptions {
                offline,
                ..Default::default()
            },
        )
    }

    /// [`Workspace::open`] with explicit options — notably a bundled stdlib
    /// source for packaged apps.
    pub fn open_with(path: &Path, opts: &OpenOptions) -> Result<Self> {
        let start = if path.is_file() {
            path.parent().unwrap_or(path)
        } else {
            path
        };
        let start = start
            .canonicalize()
            .with_context(|| format!("no such path: {}", start.display()))?;
        let resolution = pipeline::resolve(&start, opts)?;
        let root = pipeline::workspace_root(&resolution);
        Ok(Self {
            eval: pipeline::EvalState::new(resolution),
            root,
            opts: opts.clone(),
        })
    }

    /// The materialized stdlib dir (`<root>/.pcb/stdlib`).
    pub fn stdlib_dir(&self) -> PathBuf {
        self.eval.stdlib_dir()
    }

    /// Workspace root (the directory containing `pcb.toml`, or the fallback
    /// root chosen by discovery).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Re-run workspace discovery + dependency resolution. Call on
    /// `pcb.toml` changes.
    pub fn reload(&mut self) -> Result<()> {
        let resolution = pipeline::resolve(&self.root.clone(), &self.opts.clone())?;
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
