//! Public output model. No `pcb_*` type crosses this boundary — everything is
//! plain serde data that the desktop app, MCP server, and UI consume as-is.
//!
//! Instance addressing: every instance gets a stable dotted path rooted at
//! `"root"` (e.g. `root.power.ldo.C1`). This is the shared vocabulary between
//! canvas selection, diagnostics, and the agent's MCP tools.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub const ROOT_PATH: &str = "root";

/// Join a parent instance path with a child name.
pub fn child_path(parent: &str, name: &str) -> String {
    format!("{parent}.{name}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildOutput {
    /// Workspace-relative path of the .zen file that was built.
    pub source: String,
    /// `None` when evaluation failed before producing a netlist.
    pub schematic: Option<SchematicDoc>,
    pub diagnostics: Vec<Diag>,
}

impl BuildOutput {
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| !d.suppressed && d.severity == Severity::Error)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchematicDoc {
    /// Name of the root module (the .zen file's module).
    pub root_module: String,
    /// Every instance keyed by dotted path; the root is `"root"`.
    pub instances: BTreeMap<String, InstanceDoc>,
    /// Nets keyed by their unique name.
    pub nets: BTreeMap<String, NetDoc>,
    /// refdes (e.g. `R1`) -> instance path, for quick lookup.
    pub by_refdes: BTreeMap<String, String>,
}

impl SchematicDoc {
    pub fn instance(&self, path: &str) -> Option<&InstanceDoc> {
        self.instances.get(path)
    }

    /// Resolve either an instance path or a refdes to an instance path.
    pub fn resolve_path<'a>(&'a self, path_or_refdes: &'a str) -> Option<&'a str> {
        if self.instances.contains_key(path_or_refdes) {
            return Some(path_or_refdes);
        }
        self.by_refdes.get(path_or_refdes).map(String::as_str)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceKind {
    Module,
    Component,
    Interface,
    Port,
    Pin,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceDoc {
    pub path: String,
    pub kind: InstanceKind,
    /// Module/type name this instance was created from (e.g. `Resistor`).
    pub type_name: String,
    /// Workspace-relative source file of the type definition, if resolvable.
    pub source_file: Option<String>,
    /// Reference designator for components (e.g. `R1`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refdes: Option<String>,
    /// Flattened attribute map (value, package, mpn, ...).
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub attributes: BTreeMap<String, serde_json::Value>,
    /// child name -> child instance path.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub children: BTreeMap<String, String>,
    /// For components: pins with their connected net (if any).
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub pins: Vec<PinDoc>,
    /// Optional authored position from `# pcb:sch` comments in the source.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<PositionDoc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PinDoc {
    pub name: String,
    /// Net name this pin is connected to; `None` = unconnected.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionDoc {
    pub x: f64,
    pub y: f64,
    pub rotation: f64,
    /// "x" | "y" when the authored comment carried a mirror axis. Round-
    /// tripped so save-all write-back never destroys an authored mirror.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mirror: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetDoc {
    pub name: String,
    /// Net type name (`Net`, `Power`, `Ground`, ...).
    pub kind: String,
    pub ports: Vec<PortRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortRef {
    /// Instance path of the owning component.
    pub component: String,
    /// Pin/port name on that component (e.g. `P1`, `NC.2`).
    pub pin: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Advice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diag {
    pub severity: Severity,
    pub message: String,
    /// Diagnostic kind/category when available (e.g. `electrical.voltage_mismatch`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Workspace-relative file path.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based line/column of the primary span.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub col: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_col: Option<u32>,
    #[serde(default)]
    pub suppressed: bool,
    /// Outer-to-inner context chain (call sites) as human-readable frames.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub stack: Vec<String>,
}

/// A compact per-build summary suitable for MCP tool responses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildSummary {
    pub source: String,
    pub ok: bool,
    pub components: usize,
    pub nets: usize,
    pub errors: usize,
    pub warnings: usize,
}

impl BuildSummary {
    pub fn from_output(out: &BuildOutput) -> Self {
        let (components, nets) = out
            .schematic
            .as_ref()
            .map(|s| {
                (
                    s.instances
                        .values()
                        .filter(|i| i.kind == InstanceKind::Component)
                        .count(),
                    s.nets.len(),
                )
            })
            .unwrap_or((0, 0));
        let errors = out
            .diagnostics
            .iter()
            .filter(|d| !d.suppressed && d.severity == Severity::Error)
            .count();
        let warnings = out
            .diagnostics
            .iter()
            .filter(|d| !d.suppressed && d.severity == Severity::Warning)
            .count();
        Self {
            source: out.source.clone(),
            ok: errors == 0 && out.schematic.is_some(),
            components,
            nets,
            errors,
            warnings,
        }
    }
}
