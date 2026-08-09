//! `zen_build::edit` — the canvas semantic-edit layer (docs/decisions/0009).
//!
//! Two pieces live here:
//!
//! - The **editability map** (Phase 0): a per-build, purely static
//!   classification of every instance and net as *literal* (a structured
//!   writer can safely target the creating call / assignment in authored
//!   source) or *generated* (loops, comprehensions, computed names, library
//!   internals — the canvas greys out affordances and refusals carry the
//!   reason).
//! - The **structured writers** (Phase 1+): [`add_instance`] and
//!   [`rename_instance`] — span surgery over the parsed source. Untouched
//!   bytes stay byte-identical, insertions land at line boundaries, and a
//!   gesture's program-text edit and position update fold into ONE file
//!   write. Output is re-parsed before the write; a writer that would
//!   produce unparseable source fails the gesture, never the board.
//!
//! Classification is deliberately conservative: only a top-level statement
//! in an authored file — a call with a literal `name="…"` kwarg, or a
//! `VAR = Net("…")`-shaped assignment — counts as literal. Anything the
//! classifier can't prove is refused with a reason, never guessed
//! (docs/prd-canvas-editing.md §4.2).

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use starlark::syntax::ast::{ArgumentP, AssignTargetP, AstLiteral, AstPayload, ExprP, StmtP};
use starlark::syntax::{AstModule, Dialect};

use crate::model::{InstanceKind, PositionDoc, SchematicDoc};

/// Per-build editability classification, keyed like the schematic itself:
/// instances by dotted path (modules and components only — ports and pins
/// resolve through their component), nets by name.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditabilityDoc {
    pub instances: BTreeMap<String, InstanceEdit>,
    pub nets: BTreeMap<String, NetEdit>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceEdit {
    /// A structured writer can target this instance's creating call.
    pub editable: bool,
    /// Workspace-relative file holding the creating call (when literal).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based line of the call statement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// Why the instance is not editable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// For non-editable instances: the nearest ancestor whose creating call
    /// IS literal — where a structured edit reaching this instance must
    /// land (e.g. a component hoisted out of a stdlib generic anchors to
    /// the generic's call). `None` when no ancestor is editable either.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anchor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetEdit {
    /// A structured writer can rename/rewire this net's defining assignment.
    pub editable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// 1-based line of the defining assignment.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    /// The variable the net is bound to in its defining file.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl InstanceEdit {
    fn refused(reason: String) -> Self {
        Self {
            editable: false,
            file: None,
            line: None,
            reason: Some(reason),
            anchor: None,
        }
    }
}

/// One keyword argument of an instantiation call.
struct Kwarg {
    name: String,
    /// Byte span of the value expression.
    value_span: (usize, usize),
    /// The value when it is a plain identifier (a net variable reference).
    value_ident: Option<String>,
    /// The value when it is an inline `Net("X")`-shaped call (a placeholder
    /// from placement): (callee, net name).
    value_net_literal: Option<(String, String)>,
    /// Byte span of the whole `name=value` argument.
    arg_span: (usize, usize),
    /// End offset of the argument preceding this one — deleting
    /// `prev_end..arg_span.1` removes the kwarg and its comma.
    prev_end: usize,
}

/// One top-level instantiation call with a literal `name=` kwarg.
struct CallSite {
    /// Statement byte span.
    stmt: (usize, usize),
    /// Byte span of the `name=` string literal, quotes included.
    name_literal: (usize, usize),
    /// 1-based line of the statement.
    line: u32,
    /// Callee identifier (the module binding the call goes through).
    callee: String,
    kwargs: Vec<Kwarg>,
    /// End offset of the last argument — where a new kwarg inserts.
    last_arg_end: usize,
}

/// A top-level `IDENT = Module("SPEC")` binding.
struct ModuleBinding {
    ident: String,
    spec: String,
    stmt: (usize, usize),
    stmt_end: usize,
}

struct NetDef {
    variable: String,
    callee: String,
    name: String,
    line: u32,
    /// Statement byte span (for pruning).
    stmt: (usize, usize),
    /// Byte span of the variable identifier on the LHS.
    ident_span: (usize, usize),
    /// Byte span of the name string literal, quotes included.
    string_span: (usize, usize),
}

/// One authored file's top-level shape, parsed once.
#[derive(Default)]
struct FileIndex {
    /// `name=` literal of top-level instantiation calls (Board excluded —
    /// its `name=` is the board title, not an instance).
    named_calls: BTreeMap<String, Vec<CallSite>>,
    module_bindings: Vec<ModuleBinding>,
    /// Top-level `IDENT = Callee("name", …)` assignments.
    net_defs: Vec<NetDef>,
    /// Every name bound at top level (assignments, loads, defs) — the
    /// collision set for new variables.
    bound_idents: std::collections::BTreeSet<String>,
    /// The trailing `Board(...)` statement's start, if any — insertions of
    /// instantiation calls stay above it.
    board_start: Option<usize>,
    /// End offset of the last `load(...)` statement.
    last_load_end: Option<usize>,
    /// End offset of a leading docstring statement.
    docstring_end: Option<usize>,
    /// Parse/read failure, verbatim — everything in the file refuses with it.
    error: Option<String>,
}

/// Files the canvas may write: workspace-relative, outside the materialized
/// stdlib (`.pcb/`). Vendored `components/*.assets` hold no instantiations,
/// so no extra carve-out is needed.
fn is_authored(rel: &str) -> bool {
    !rel.starts_with('/') && !rel.starts_with(".pcb/") && rel.ends_with(".zen")
}

/// The evaluator's dialect exactly (pcb-zen-core::eval), so the edit layer
/// accepts precisely what the build accepts.
fn dialect() -> Dialect {
    let mut dialect = Dialect::Extended;
    dialect.enable_f_strings = true;
    dialect
}

fn parse(rel: &str, content: &str) -> Result<AstModule> {
    AstModule::parse(rel, content.to_string(), &dialect())
        .map_err(|e| anyhow::anyhow!("{rel} does not parse: {e}"))
}

/// Extract a [`CallSite`] from a call expression carrying a literal `name=`.
fn call_site<P: AstPayload>(
    expr: &ExprP<P>,
    stmt: (usize, usize),
    line: u32,
) -> Option<CallSite> {
    let ExprP::Call(callee, args) = expr else {
        return None;
    };
    let ExprP::Identifier(callee_ident) = &callee.node else {
        return None;
    };
    let name_lit = call_name_literal(expr)?;
    let mut kwargs = Vec::new();
    let mut prev_end = 0usize;
    for a in &args.args {
        let arg_span = (a.span.begin().get() as usize, a.span.end().get() as usize);
        if let ArgumentP::Named(k, v) = &a.node {
            let value_net_literal = match &v.node {
                ExprP::Call(callee, net_args) => match &callee.node {
                    ExprP::Identifier(c)
                        if matches!(c.node.ident.as_str(), "Net" | "Power" | "Ground") =>
                    {
                        net_args
                            .args
                            .first()
                            .and_then(|na| match &na.node {
                                ArgumentP::Positional(e) => string_literal(&e.node),
                                _ => None,
                            })
                            .map(|lit| (c.node.ident.clone(), string_value(lit)))
                    }
                    _ => None,
                },
                _ => None,
            };
            kwargs.push(Kwarg {
                name: k.node.clone(),
                value_span: (v.span.begin().get() as usize, v.span.end().get() as usize),
                value_ident: match &v.node {
                    ExprP::Identifier(id) => Some(id.node.ident.clone()),
                    _ => None,
                },
                value_net_literal,
                arg_span,
                prev_end,
            });
        }
        prev_end = arg_span.1;
    }
    let last_arg_end = args
        .args
        .iter()
        .map(|a| a.expr().span.end().get() as usize)
        .max()
        .unwrap_or(stmt.1);
    Some(CallSite {
        stmt,
        name_literal: literal_span(name_lit),
        line,
        callee: callee_ident.node.ident.clone(),
        kwargs,
        last_arg_end,
    })
}

fn index_content(rel: &str, content: &str) -> FileIndex {
    let ast = match parse(rel, content) {
        Ok(a) => a,
        Err(e) => {
            return FileIndex {
                error: Some(e.to_string()),
                ..Default::default()
            }
        }
    };

    let mut index = FileIndex::default();
    let top: Vec<_> = match &ast.statement().node {
        StmtP::Statements(v) => v.iter().collect(),
        _ => vec![ast.statement()],
    };
    for (i, stmt) in top.iter().enumerate() {
        let span = (
            stmt.span.begin().get() as usize,
            stmt.span.end().get() as usize,
        );
        let line = ast.file_span(stmt.span).resolve_span().begin.line as u32 + 1;
        match &stmt.node {
            StmtP::Load(load) => {
                index.last_load_end = Some(span.1);
                for arg in &load.args {
                    index.bound_idents.insert(arg.local.node.ident.clone());
                }
            }
            StmtP::Def(def) => {
                index.bound_idents.insert(def.name.node.ident.clone());
            }
            // A leading docstring.
            StmtP::Expression(e)
                if i == 0 && matches!(&e.node, ExprP::Literal(AstLiteral::String(_))) =>
            {
                index.docstring_end = Some(span.1);
            }
            // `Resistor(name="R_LIMIT", …)` — a bare instantiation call.
            StmtP::Expression(expr) => {
                if callee_name(&expr.node) == Some("Board") {
                    index.board_start.get_or_insert(span.0);
                } else if let Some(site) = call_site(&expr.node, span, line) {
                    index
                        .named_calls
                        .entry(string_value(call_name_literal(&expr.node).expect("checked")))
                        .or_default()
                        .push(site);
                }
            }
            StmtP::Assign(assign) => {
                let AssignTargetP::Identifier(ident) = &assign.lhs.node else {
                    continue;
                };
                index.bound_idents.insert(ident.node.ident.clone());
                let ExprP::Call(callee, args) = &assign.rhs.node else {
                    continue;
                };
                let ExprP::Identifier(callee_ident) = &callee.node else {
                    continue;
                };
                let first_string = args.args.first().and_then(|a| match &a.node {
                    ArgumentP::Positional(e) => string_literal(&e.node),
                    _ => None,
                });
                // `Resistor = Module("@stdlib/…")` — a module binding.
                if callee_ident.node.ident == "Module" {
                    if let Some(lit) = first_string {
                        index.module_bindings.push(ModuleBinding {
                            ident: ident.node.ident.clone(),
                            spec: string_value(lit),
                            stmt: span,
                            stmt_end: span.1,
                        });
                        continue;
                    }
                }
                // `x = Resistor(name="R1", …)` still counts as an
                // instantiation; record it under its name= like a bare call.
                if let Some(site) = call_site(&assign.rhs.node, span, line) {
                    index
                        .named_calls
                        .entry(string_value(
                            call_name_literal(&assign.rhs.node).expect("checked"),
                        ))
                        .or_default()
                        .push(site);
                }
                // `LED_A = Net("LED_A")` / `VCC = Power("VCC_3V3")` — a
                // net-shaped assignment.
                if let Some(lit) = first_string {
                    index.net_defs.push(NetDef {
                        variable: ident.node.ident.clone(),
                        callee: callee_ident.node.ident.clone(),
                        name: string_value(lit),
                        line,
                        stmt: span,
                        ident_span: (
                            assign.lhs.span.begin().get() as usize,
                            assign.lhs.span.end().get() as usize,
                        ),
                        string_span: literal_span(lit),
                    });
                }
            }
            _ => {}
        }
    }
    index
}

fn index_file(workspace_root: &Path, rel: &str) -> FileIndex {
    let abs = workspace_root.join(rel);
    match std::fs::read_to_string(&abs) {
        Ok(content) => index_content(rel, &content),
        Err(e) => FileIndex {
            error: Some(format!("cannot read {rel}: {e}")),
            ..Default::default()
        },
    }
}

fn callee_name<P: AstPayload>(expr: &ExprP<P>) -> Option<&str> {
    let ExprP::Call(callee, _) = expr else {
        return None;
    };
    match &callee.node {
        ExprP::Identifier(ident) => Some(&ident.node.ident),
        _ => None,
    }
}

/// `Callee(…, name="literal", …)` -> the literal's AST node, for a call
/// whose callee is a plain identifier.
fn call_name_literal<P: AstPayload>(
    expr: &ExprP<P>,
) -> Option<&starlark::syntax::ast::AstString> {
    let ExprP::Call(callee, args) = expr else {
        return None;
    };
    if !matches!(&callee.node, ExprP::Identifier(_)) {
        return None;
    }
    args.args.iter().find_map(|a| match &a.node {
        ArgumentP::Named(k, v) if k.node == "name" => string_literal(&v.node),
        _ => None,
    })
}

fn string_literal<P: AstPayload>(
    expr: &ExprP<P>,
) -> Option<&starlark::syntax::ast::AstString> {
    match expr {
        ExprP::Literal(AstLiteral::String(s)) => Some(s),
        _ => None,
    }
}

fn string_value(lit: &starlark::syntax::ast::AstString) -> String {
    lit.node.clone()
}

/// Byte span of the literal, quotes included.
fn literal_span(lit: &starlark::syntax::ast::AstString) -> (usize, usize) {
    (lit.span.begin().get() as usize, lit.span.end().get() as usize)
}

/// Build the editability map for a schematic. Purely static — reads authored
/// files, never fails: anything unclassifiable is refused with a reason.
pub fn analyze_editability(sch: &SchematicDoc, workspace_root: &Path) -> EditabilityDoc {
    // Parse each referenced authored file once. The scan set is the parents'
    // source files — every call site lives in the file defining the parent's
    // module body.
    let mut indexes: BTreeMap<&str, FileIndex> = BTreeMap::new();
    for inst in sch.instances.values() {
        if let Some(f) = inst.source_file.as_deref() {
            if is_authored(f) && !indexes.contains_key(f) {
                indexes.insert(f, index_file(workspace_root, f));
            }
        }
    }

    let mut doc = EditabilityDoc::default();

    for (path, inst) in &sch.instances {
        if !matches!(inst.kind, InstanceKind::Module | InstanceKind::Component) {
            continue;
        }
        let Some((parent_path, local)) = path.rsplit_once('.') else {
            continue; // the root itself is not an edit target
        };
        let entry = match sch
            .instances
            .get(parent_path)
            .and_then(|p| p.source_file.as_deref())
        {
            None => InstanceEdit::refused(
                "the enclosing module has no resolvable source file".into(),
            ),
            Some(parent_file) if !is_authored(parent_file) => InstanceEdit::refused(format!(
                "instantiated inside library source ({parent_file}) — structured edits land on \
                 the nearest authored ancestor"
            )),
            Some(parent_file) => {
                let index = &indexes[parent_file];
                if let Some(e) = &index.error {
                    InstanceEdit::refused(e.clone())
                } else {
                    match index.named_calls.get(local).map(Vec::as_slice) {
                        Some([site]) => InstanceEdit {
                            editable: true,
                            file: Some(parent_file.to_string()),
                            line: Some(site.line),
                            reason: None,
                            anchor: None,
                        },
                        Some(sites) => InstanceEdit::refused(format!(
                            "ambiguous: {} top-level calls in {parent_file} carry \
                             name=\"{local}\" (lines {})",
                            sites.len(),
                            sites
                                .iter()
                                .map(|s| s.line.to_string())
                                .collect::<Vec<_>>()
                                .join(", "),
                        )),
                        None => InstanceEdit::refused(format!(
                            "no top-level call with a literal name=\"{local}\" in {parent_file} \
                             — the instance is likely generated (loop, comprehension, or \
                             computed name); edit the source or ask the agent"
                        )),
                    }
                }
            }
        };
        doc.instances.insert(path.clone(), entry);
    }

    // Anchor pass: nearest editable ancestor for everything refused. Walks
    // the map just built, so ancestors classify before descendants resolve.
    let anchors: Vec<(String, String)> = doc
        .instances
        .iter()
        .filter(|(_, e)| !e.editable)
        .filter_map(|(path, _)| {
            let mut cursor = path.as_str();
            while let Some((parent, _)) = cursor.rsplit_once('.') {
                if doc.instances.get(parent).is_some_and(|e| e.editable) {
                    return Some((path.clone(), parent.to_string()));
                }
                cursor = parent;
            }
            None
        })
        .collect();
    for (path, anchor) in anchors {
        doc.instances.get_mut(&path).expect("just iterated").anchor = Some(anchor);
    }

    for (name, net) in &sch.nets {
        let defs: Vec<(&str, &NetDef)> = indexes
            .iter()
            .flat_map(|(file, idx)| idx.net_defs.iter().map(move |d| (*file, d)))
            .filter(|(_, d)| d.name == *name && d.callee == net.kind)
            .collect();
        let entry = match defs.as_slice() {
            [(file, def)] => NetEdit {
                editable: true,
                file: Some((*file).to_string()),
                line: Some(def.line),
                variable: Some(def.variable.clone()),
                reason: None,
            },
            [] => NetEdit {
                editable: false,
                file: None,
                line: None,
                variable: None,
                reason: Some(format!(
                    "no top-level `VAR = {}(\"{name}\")` assignment in authored source — the \
                     net may be generated or scoped through io(); edit the source or ask the \
                     agent",
                    net.kind
                )),
            },
            many => NetEdit {
                editable: false,
                file: None,
                line: None,
                variable: None,
                reason: Some(format!(
                    "ambiguous: {} assignments define {}(\"{name}\") ({})",
                    many.len(),
                    net.kind,
                    many.iter()
                        .map(|(f, d)| format!("{f}:{}", d.line))
                        .collect::<Vec<_>>()
                        .join(", "),
                )),
            },
        };
        doc.nets.insert(name.clone(), entry);
    }

    doc
}

// ---------------------------------------------------------------------------
// Structured writers (Phase 1)
// ---------------------------------------------------------------------------

/// Instance names: same shape scaffold enforces for component names.
fn valid_instance_name(name: &str) -> bool {
    let mut chars = name.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// What a module file declares about itself — statically, from literals.
/// Everything is optional: a module that computes these stays usable, it
/// just loses position write-through (child unknown) or name suggestions.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ModuleFacts {
    /// The single top-level `Component(name="X")` literal — the child
    /// component's instance name, which position keys are built from.
    pub component_child: Option<String>,
    /// `Component(…, prefix="R")` — refdes-style name suggestions.
    pub prefix: Option<String>,
    /// Top-level `IDENT = io(…)` declarations without `optional=…`, in
    /// order. These MUST be bound at instantiation, so placement
    /// synthesizes a placeholder net per pin (`Net("{name}_{pin}")` — the
    /// same names the wiring verbs will later replace).
    pub required_ios: Vec<String>,
    /// The Component call's literal `pins={"1": P1, …}` dict: component pin
    /// name -> io identifier. How canvas pins translate to call kwargs.
    pub pin_map: BTreeMap<String, String>,
    /// `IDENT = config(Type, …)` declarations: parameter -> type name
    /// (`Resistance`, `Package`, `str`, …). What attribute values must be.
    pub config_types: BTreeMap<String, String>,
    /// Configs with neither `default=` nor `optional=` — the instantiation
    /// fails without them, so placement must collect them up front.
    pub config_required: Vec<String>,
    /// `IDENT = enum("a", "b", …)` declarations: enum type -> variants.
    pub enums: BTreeMap<String, Vec<String>>,
    /// Loaded symbols in the module file: symbol -> load module string,
    /// with intra-stdlib relative paths normalized to `@stdlib/…`. How the
    /// board file can load an interface constructor (`Analog`) a typed
    /// placeholder needs.
    pub type_loads: BTreeMap<String, String>,
}

/// Read [`ModuleFacts`] from a module spec (`@stdlib/generics/Resistor.zen`
/// or a workspace-relative path). Never fails — unknown is `None`.
pub fn module_facts(spec: &str, workspace_root: &Path, stdlib_dir: &Path) -> ModuleFacts {
    let Ok(path) = crate::catalog::resolve_library_path(spec, workspace_root, stdlib_dir) else {
        return ModuleFacts::default();
    };
    let Ok(content) = std::fs::read_to_string(&path) else {
        return ModuleFacts::default();
    };
    let Ok(ast) = parse(spec, &content) else {
        return ModuleFacts::default();
    };
    let top: Vec<_> = match &ast.statement().node {
        StmtP::Statements(v) => v.iter().collect(),
        _ => vec![ast.statement()],
    };
    let mut facts = ModuleFacts::default();
    for stmt in top {
        // Loads: how the module names its imported types — replicated in
        // the board when a typed placeholder needs a constructor.
        if let StmtP::Load(load) = &stmt.node {
            let module = normalize_load(spec, &load.module.node);
            for arg in &load.args {
                facts
                    .type_loads
                    .insert(arg.local.node.ident.clone(), module.clone());
            }
            continue;
        }
        // Assignments carry the module's parameter surface: `P1 = io(Net)`
        // (required unless `optional=…`), `value = config(Resistance)`,
        // `Package = enum("0201", …)`.
        if let StmtP::Assign(assign) = &stmt.node {
            if let (AssignTargetP::Identifier(ident), ExprP::Call(callee, args)) =
                (&assign.lhs.node, &assign.rhs.node)
            {
                let callee_name = match &callee.node {
                    ExprP::Identifier(c) => Some(c.node.ident.as_str()),
                    _ => None,
                };
                match callee_name {
                    Some("io")
                        if !args.args.iter().any(|a| {
                            matches!(&a.node, ArgumentP::Named(k, _) if k.node == "optional")
                        }) =>
                    {
                        facts.required_ios.push(ident.node.ident.clone());
                    }
                    Some("config") => {
                        if let Some(ArgumentP::Positional(e)) =
                            args.args.first().map(|a| &a.node)
                        {
                            if let ExprP::Identifier(ty) = &e.node {
                                facts.config_types.insert(
                                    ident.node.ident.clone(),
                                    ty.node.ident.clone(),
                                );
                                let has_escape = args.args.iter().any(|a| {
                                    matches!(&a.node, ArgumentP::Named(k, _)
                                        if k.node == "default" || k.node == "optional")
                                });
                                if !has_escape {
                                    facts.config_required.push(ident.node.ident.clone());
                                }
                            }
                        }
                    }
                    Some("enum") => {
                        let variants: Vec<String> = args
                            .args
                            .iter()
                            .filter_map(|a| match &a.node {
                                ArgumentP::Positional(e) => {
                                    string_literal(&e.node).map(string_value)
                                }
                                _ => None,
                            })
                            .collect();
                        if !variants.is_empty() {
                            facts.enums.insert(ident.node.ident.clone(), variants);
                        }
                    }
                    _ => {}
                }
            }
            continue;
        }
        let StmtP::Expression(expr) = &stmt.node else {
            continue;
        };
        if callee_name(&expr.node) != Some("Component") {
            continue;
        }
        let ExprP::Call(_, args) = &expr.node else {
            continue;
        };
        let named_string = |key: &str| {
            args.args.iter().find_map(|a| match &a.node {
                ArgumentP::Named(k, v) if k.node == key => {
                    string_literal(&v.node).map(string_value)
                }
                _ => None,
            })
        };
        // Exactly one top-level Component call is the resolvable shape;
        // a second one makes the child ambiguous — drop the fact.
        if facts.component_child.is_some() {
            return ModuleFacts::default();
        }
        facts.component_child = named_string("name");
        facts.prefix = named_string("prefix");
        // pins={"1": P1, "2": P2} — literal keys to identifier values.
        if let Some(pins_expr) = args.args.iter().find_map(|a| match &a.node {
            ArgumentP::Named(k, v) if k.node == "pins" => Some(&v.node),
            _ => None,
        }) {
            if let ExprP::Dict(entries) = pins_expr {
                for (key, value) in entries {
                    if let (Some(pin), ExprP::Identifier(io)) =
                        (string_literal(&key.node), &value.node)
                    {
                        facts
                            .pin_map
                            .insert(string_value(pin), io.node.ident.clone());
                    }
                }
            }
        }
    }
    facts
}

/// Resolve a load module string as seen from the BOARD file: package refs
/// (`@stdlib/…`) pass through; relative loads resolve against the module
/// spec's own directory (`../interfaces.zen` inside
/// `@stdlib/generics/Resistor.zen` -> `@stdlib/interfaces.zen`).
fn normalize_load(spec: &str, load_module: &str) -> String {
    if load_module.starts_with('@') {
        return load_module.to_string();
    }
    // Everything else — "./x", "../x", AND bare names like "types.zen" —
    // resolves relative to the loading module's own directory (pcb-zen's
    // LoadSpec treats bare names as raw relative paths, not packages).
    let base = spec.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
    let mut parts: Vec<&str> = base.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    for seg in load_module.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let joined = parts.join("/");
    if spec.starts_with('@') || spec.starts_with("./") {
        // Keep the spec's own addressing style.
        if spec.starts_with('@') {
            joined
        } else {
            format!("./{joined}")
        }
    } else {
        joined
    }
}

/// An example literal for a physical-unit config type, for error copy.
fn type_example(ty: &str) -> Option<&'static str> {
    Some(match ty {
        "Resistance" | "Impedance" => "10kohm",
        "Capacitance" => "100nF",
        "Inductance" => "10uH",
        "Voltage" => "3.3V",
        "Current" => "1A",
        "Frequency" => "16MHz",
        "Time" => "10ms",
        "Power" => "125mW",
        "Charge" => "1C",
        "Conductance" => "1S",
        "Energy" => "1J",
        "Temperature" => "300K",
        _ => return None,
    })
}

/// Validate one attribute value against the module's declared config type
/// BEFORE anything is written — with the REAL unit parser (pcb_sch), not a
/// reimplementation. Unknown/unresolvable types pass (tolerant); known
/// types refuse with the expected unit and an example.
pub fn validate_attr(facts: &ModuleFacts, key: &str, value: &str) -> Result<()> {
    let Some(ty) = facts.config_types.get(key) else {
        if facts.config_types.is_empty() {
            return Ok(()); // module surface unresolvable — the build decides
        }
        bail!(
            "{key} is not a parameter of this module (has: {})",
            facts.config_types.keys().cloned().collect::<Vec<_>>().join(", ")
        );
    };
    if let Some(variants) = facts.enums.get(ty) {
        if !variants.iter().any(|v| v == value) {
            bail!("{key} must be one of {}", variants.join(", "));
        }
        return Ok(());
    }
    let (unit, example) = match ty.as_str() {
        "str" => return Ok(()),
        "bool" => {
            if matches!(value, "True" | "False" | "true" | "false") {
                return Ok(());
            }
            bail!("{key} must be True or False");
        }
        "int" => {
            if value.parse::<i64>().is_ok() {
                return Ok(());
            }
            bail!("{key} must be a whole number");
        }
        "float" => {
            if value.parse::<f64>().is_ok() {
                return Ok(());
            }
            bail!("{key} must be a number");
        }
        "Resistance" | "Impedance" => (pcb_sch::PhysicalUnit::Ohms, "10kohm"),
        "Capacitance" => (pcb_sch::PhysicalUnit::Farads, "100nF"),
        "Inductance" => (pcb_sch::PhysicalUnit::Henries, "10uH"),
        "Voltage" => (pcb_sch::PhysicalUnit::Volts, "3.3V"),
        "Current" => (pcb_sch::PhysicalUnit::Amperes, "1A"),
        "Frequency" => (pcb_sch::PhysicalUnit::Hertz, "16MHz"),
        "Time" => (pcb_sch::PhysicalUnit::Seconds, "10ms"),
        "Power" => (pcb_sch::PhysicalUnit::Watts, "125mW"),
        "Charge" => (pcb_sch::PhysicalUnit::Coulombs, "1C"),
        "Conductance" => (pcb_sch::PhysicalUnit::Siemens, "1S"),
        "Energy" => (pcb_sch::PhysicalUnit::Joules, "1J"),
        "Temperature" => (pcb_sch::PhysicalUnit::Kelvin, "300K"),
        _ => return Ok(()), // user-defined type — the build decides
    };
    // Bare numbers are valid for any physical type: the Starlark
    // constructor interprets them in its own unit ("100" -> 100F for a
    // Capacitance), while the untyped parser would default them to ohms.
    if value.trim().parse::<f64>().is_ok() {
        return Ok(());
    }
    use std::str::FromStr;
    let parsed = pcb_sch::physical::PhysicalValue::from_str(value)
        .map_err(|_| anyhow::anyhow!("{value:?} is not a valid {ty} — try {example}"))?;
    parsed
        .check_unit(unit.into())
        .map_err(|_| anyhow::anyhow!("{value:?} is not a {ty} — try {example}"))?;
    Ok(())
}

/// Ask the EVALUATOR what a module's nets look like standalone: unbound
/// ios auto-bind into nets named after them, carrying their TYPE. Used as
/// constructor HINTS for the preflight (the map may also contain internal
/// nets, so it is never treated as the io list itself). `stdlib_source`
/// keeps packaged apps working, where exe-ancestor discovery can't find
/// lib/std.
pub fn probe_module_ios(
    module_abs: &Path,
    stdlib_source: &Path,
) -> Result<BTreeMap<String, String>> {
    let ws = crate::Workspace::open_with(
        module_abs,
        &crate::OpenOptions {
            offline: true,
            stdlib_source: Some(stdlib_source.to_path_buf()),
        },
    )
    .with_context(|| format!("probing {}", module_abs.display()))?;
    let out = ws
        .build_file(module_abs, &BTreeMap::new())
        .with_context(|| format!("probing {}", module_abs.display()))?;
    if out.has_errors() {
        let first = out
            .diagnostics
            .iter()
            .find(|d| d.severity == crate::model::Severity::Error)
            .map(|d| d.message.clone())
            .unwrap_or_default();
        bail!("the module itself does not build: {first}");
    }
    let sch = out
        .schematic
        .context("module probe produced no schematic")?;
    Ok(sch
        .nets
        .into_iter()
        .map(|(name, net)| (name, net.kind))
        .collect())
}

/// The real rendered shape of a module's component, from the preflight's
/// own successful eval — so ghosts and provisional stand-ins draw the
/// TRUE outline and pin positions, and the swap to the real symbol is
/// seamless instead of a jump.
#[derive(Debug, Clone, Serialize)]
pub struct GhostGeometry {
    pub width: f64,
    pub height: f64,
    /// Pin offsets from the component center, schematic units (y-up).
    pub pins: Vec<GhostPin>,
}

#[derive(Debug, Clone, Serialize)]
pub struct GhostPin {
    pub name: String,
    pub x: f64,
    pub y: f64,
}

/// The io kwargs a placement will write, each proven against the evaluator.
#[derive(Clone)]
struct Preflight {
    /// io name -> constructor identifier (`Net`/`Power`/`Ground`/interface).
    pins: Vec<(String, String)>,
    /// Interface constructors' loads, board-form (symbol -> module string).
    loads: BTreeMap<String, String>,
    ghost: Option<GhostGeometry>,
}

/// Preflights are cached per (workspace, spec, module mtime): the pin set
/// doesn't depend on attribute values, so a palette arm can pre-warm the
/// cache and drops become instant instead of paying evaluator round-trips
/// inside the click.
type PreflightKey = (std::path::PathBuf, String, Option<std::time::SystemTime>);
fn preflight_cache(
) -> &'static std::sync::Mutex<std::collections::HashMap<PreflightKey, Preflight>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<PreflightKey, Preflight>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(Default::default)
}

fn preflight_key(workspace_root: &Path, stdlib_dir: &Path, spec: &str) -> PreflightKey {
    let mtime = crate::catalog::resolve_library_path(spec, workspace_root, stdlib_dir)
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok());
    (workspace_root.to_path_buf(), spec.to_string(), mtime)
}

/// Deletes the scratch probe file even on early returns.
struct ScratchGuard(std::path::PathBuf);
impl Drop for ScratchGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// The hard guarantee behind placement (review finding): evaluate the
/// EXACT instantiation in a hidden scratch file at the workspace root —
/// the watcher ignores dot-paths, and the scratch shares the board's
/// workspace so module specs and loads work verbatim — BEFORE the board
/// is touched. Required inputs come from the evaluator's own
/// "Input 'X' is required" diagnostics (comprehension-built ios
/// included), wrong-constructor guesses self-correct from
/// "expected T, got U", and anything unresolvable refuses with the
/// diagnostic verbatim. Returns `None` when the module doesn't evaluate
/// at all in this environment (the caller falls back to static analysis).
fn preflight_instantiation(
    workspace_root: &Path,
    stdlib_dir: &Path,
    spec: &str,
    attrs: &[(String, String)],
    facts: &ModuleFacts,
    hints: &BTreeMap<String, String>,
) -> Result<Option<Preflight>> {
    let scratch = workspace_root.join(".etchable-probe.zen");
    let _guard = ScratchGuard(scratch.clone());

    let mut pins: BTreeMap<String, String> = BTreeMap::new();
    let mut loads: BTreeMap<String, String> = BTreeMap::new();
    let mut ws: Option<crate::Workspace> = None;
    let mut solved_any = false;
    let mut last_err = String::new();

    let ensure_ctor = |ctor: &str,
                       loads: &mut BTreeMap<String, String>|
     -> Result<()> {
        if matches!(ctor, "Net" | "Power" | "Ground") || loads.contains_key(ctor) {
            return Ok(());
        }
        match facts.type_loads.get(ctor) {
            Some(module) => {
                loads.insert(ctor.to_string(), module.clone());
                Ok(())
            }
            None => bail!(
                "this part needs a {ctor} connection the canvas can't construct — ask \
                 the agent to place and wire it"
            ),
        }
    };

    for _round in 0..6 {
        let mut text = String::new();
        for (sym, module) in &loads {
            text.push_str(&format!("load({module:?}, {sym:?})\n"));
        }
        text.push_str(&format!("M = Module({spec:?})\n"));
        let mut call = String::from("M(name=\"PROBE\"");
        for (k, v) in attrs {
            call.push_str(&format!(", {k}={v:?}"));
        }
        for (io, ctor) in &pins {
            call.push_str(&format!(", {io}={ctor}(\"PROBE_{io}\")"));
        }
        call.push_str(")\n");
        text.push_str(&call);
        std::fs::write(&scratch, &text)
            .with_context(|| format!("writing {}", scratch.display()))?;

        if ws.is_none() {
            // stdlib_source keeps packaged apps working, where the scratch
            // workspace can't discover lib/std by walking up from the exe.
            let opts = crate::OpenOptions {
                offline: true,
                stdlib_source: Some(stdlib_dir.to_path_buf()),
            };
            match crate::Workspace::open_with(&scratch, &opts) {
                Ok(w) => ws = Some(w),
                // No workspace for the scratch — environment, not the part.
                Err(_) => return Ok(None),
            }
        }
        let workspace = ws.as_ref().expect("just set");
        let Ok(out) = workspace.build_file(&scratch, &BTreeMap::new()) else {
            return Ok(None);
        };
        let errors: Vec<&crate::model::Diag> = out
            .diagnostics
            .iter()
            .filter(|d| !d.suppressed && d.severity == crate::model::Severity::Error)
            .collect();
        if errors.is_empty() {
            // The successful eval also yields the REAL rendered geometry of
            // the part — the outline and pin offsets ghosts should draw.
            let ghost = out.schematic.as_ref().map(|sch| {
                let layout = crate::layout::compute_layout(sch);
                layout
                    .comps
                    .iter()
                    .find(|(path, _)| path.starts_with("root.PROBE"))
                    .map(|(_, cl)| GhostGeometry {
                        width: cl.size.0,
                        height: cl.size.1,
                        pins: cl
                            .pins
                            .iter()
                            .map(|p| GhostPin {
                                name: p.name.clone(),
                                x: p.x - cl.center.0,
                                y: -(p.y - cl.center.1),
                            })
                            .collect(),
                    })
            });
            return Ok(Some(Preflight {
                pins: pins.into_iter().collect(),
                loads,
                ghost: ghost.flatten(),
            }));
        }

        let mut progressed = false;
        for e in &errors {
            // "Input 'P1' is required but was not provided"
            if let Some(input) = between(&e.message, "Input '", "' is required") {
                if !pins.contains_key(input) && !attrs.iter().any(|(k, _)| k == input) {
                    let ctor = match hints.get(input).map(String::as_str) {
                        Some("Power") => "Power",
                        Some("Ground") => "Ground",
                        _ => "Net",
                    };
                    ensure_ctor(ctor, &mut loads)?;
                    pins.insert(input.to_string(), ctor.to_string());
                    progressed = true;
                    solved_any = true;
                }
            }
            // "Input 'VIN' has wrong net type: expected Power, got Net"
            else if let Some(input) = between(&e.message, "Input '", "' has wrong net type")
            {
                if let Some(expected) = between(&e.message, "expected ", ",") {
                    if pins.contains_key(input) {
                        ensure_ctor(expected, &mut loads)?;
                        pins.insert(input.to_string(), expected.to_string());
                        progressed = true;
                        solved_any = true;
                    }
                }
            }
        }
        last_err = errors[0].message.clone();
        if !progressed {
            break;
        }
    }

    if solved_any {
        // The evaluator engaged with the instantiation but something it
        // reported can't be auto-solved: an honest refusal, never a write.
        bail!("this part can't be placed as-is: {last_err}");
    }
    // The module never evaluated (broken standalone, or a fixture-grade
    // environment) — let the caller fall back to static analysis.
    Ok(None)
}

/// Pre-warm the placement preflight for a module — called when a palette
/// item is ARMED, so by the time the user drops, the pins and geometry are
/// cached and the drop is instant. Required configs are stood in with
/// type-appropriate examples (the pin set doesn't depend on their values).
/// Best-effort: returns the part's real geometry when the module evaluates.
pub fn warm_placement(
    workspace_root: &Path,
    stdlib_dir: &Path,
    spec: &str,
) -> Option<GhostGeometry> {
    let key = preflight_key(workspace_root, stdlib_dir, spec);
    if let Some(p) = preflight_cache().lock().expect("preflight cache").get(&key) {
        return p.ghost.clone();
    }
    let facts = module_facts(spec, workspace_root, stdlib_dir);
    let attrs: Vec<(String, String)> = facts
        .config_required
        .iter()
        .map(|k| {
            let ty = facts.config_types.get(k).map(String::as_str).unwrap_or("str");
            let value = facts
                .enums
                .get(ty)
                .and_then(|v| v.first().cloned())
                .or_else(|| type_example(ty).map(str::to_string))
                .unwrap_or_else(|| "1".to_string());
            (k.clone(), value)
        })
        .collect();
    let hints = crate::catalog::resolve_library_path(spec, workspace_root, stdlib_dir)
        .ok()
        .and_then(|abs| probe_module_ios(&abs, stdlib_dir).ok())
        .unwrap_or_default();
    let preflight =
        preflight_instantiation(workspace_root, stdlib_dir, spec, &attrs, &facts, &hints)
            .ok()
            .flatten()?;
    let ghost = preflight.ghost.clone();
    preflight_cache()
        .lock()
        .expect("preflight cache")
        .insert(key, preflight);
    ghost
}

/// The text between `start` and the next occurrence of `end` after it.
fn between<'a>(s: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let from = s.find(start)? + start.len();
    let len = s[from..].find(end)?;
    Some(&s[from..from + len])
}

/// A drop position in schematic space (y-up, `get_circuit_json` units).
#[derive(Debug, Clone, Copy, Deserialize)]
pub struct PlacedPosition {
    pub x: f64,
    pub y: f64,
    #[serde(default)]
    pub rotation: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddInstanceRequest {
    /// Module spec exactly as it appears in `Module("…")`.
    pub module: String,
    pub name: String,
    /// Ordered kwargs after `name=`; values are emitted as string literals.
    #[serde(default)]
    pub attrs: Vec<(String, String)>,
    /// When set, the drop point becomes the new instance's authored
    /// position and every existing component's current spot is snapshotted
    /// in the same write (the all-or-nothing authored rule).
    pub position: Option<PlacedPosition>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddInstanceResult {
    /// 1-based line of the inserted call.
    pub line: u32,
    /// The inserted statement text.
    pub inserted: String,
    /// The module-binding identifier the call uses.
    pub binding: String,
    /// `# pcb:sch` key written for the new instance; `None` when the module
    /// file's component child could not be statically resolved (the layout
    /// derives the position instead).
    pub position_key: Option<String>,
    /// Placeholder nets synthesized for required-but-unwired pins
    /// (`{name}_{pin}`); wiring later replaces them.
    pub placeholder_nets: Vec<String>,
    /// The instance's connection points (io names), for provisional
    /// rendering while a rebuild is pending or failing.
    pub pins: Vec<String>,
    /// The part's real rendered outline and pin offsets (schematic units,
    /// y-up), when the preflight evaluated — stand-ins draw the true shape.
    pub ghost: Option<GhostGeometry>,
}

/// Append-shaped program edit: ensure a `Module("…")` binding, insert the
/// instantiation call after the last instantiation (above `Board(...)`),
/// and fold the position snapshot into the same single file write.
///
/// `sch` is the last good build — used for the save-all position snapshot
/// and cross-file name collisions. The caller wraps this in the write gate.
pub fn add_instance(
    zen_file: &Path,
    workspace_root: &Path,
    stdlib_dir: &Path,
    sch: Option<&SchematicDoc>,
    req: &AddInstanceRequest,
) -> Result<AddInstanceResult> {
    let rel = zen_file
        .strip_prefix(workspace_root)
        .unwrap_or(zen_file)
        .display()
        .to_string();
    if !valid_instance_name(&req.name) {
        bail!(
            "invalid instance name {:?} (want [A-Za-z][A-Za-z0-9_-]*, max 64)",
            req.name
        );
    }
    let content = std::fs::read_to_string(zen_file)
        .with_context(|| format!("reading {}", zen_file.display()))?;
    let index = index_content(&rel, &content);
    if let Some(e) = &index.error {
        bail!("{e}");
    }
    if index.named_calls.contains_key(&req.name) {
        bail!("an instance named {:?} already exists in {rel}", req.name);
    }
    if let Some(root) = sch.and_then(|s| s.instances.get("root")) {
        if root.children.contains_key(&req.name) {
            bail!(
                "the board already has a child named {:?} (possibly generated)",
                req.name
            );
        }
    }

    // The callee: an existing binding for this spec, or a new one.
    let existing = index.module_bindings.iter().find(|b| b.spec == req.module);
    let binding = match existing {
        Some(b) => b.ident.clone(),
        None => {
            let stem = req
                .module
                .rsplit('/')
                .next()
                .unwrap_or(&req.module)
                .trim_end_matches(".zen");
            let mut ident: String = stem
                .chars()
                .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                .collect();
            if !ident.starts_with(|c: char| c.is_ascii_alphabetic()) {
                ident.insert(0, 'M');
            }
            while index.module_bindings.iter().any(|b| b.ident == ident) {
                ident.push('_');
            }
            ident
        }
    };

    // Insertion anchors, both at line boundaries.
    let binding_stmt = existing.is_none().then(|| {
        let anchor = index
            .module_bindings
            .iter()
            .map(|b| b.stmt_end)
            .max()
            .or(index.last_load_end)
            .or(index.docstring_end);
        let text = format!("{binding} = Module({:?})\n", req.module);
        (anchor, text)
    });

    let facts = module_facts(&req.module, workspace_root, stdlib_dir);

    // Validate every attribute BEFORE anything is written: real unit
    // parsing, enum variants, and required configs — a bad value refuses
    // here, it never becomes a red board.
    for (k, v) in &req.attrs {
        if !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || k.is_empty() {
            bail!("invalid attribute name {k:?}");
        }
        validate_attr(&facts, k, v)?;
    }
    for required in &facts.config_required {
        if !req.attrs.iter().any(|(k, _)| k == required) {
            let ty = facts
                .config_types
                .get(required)
                .map(String::as_str)
                .unwrap_or("value");
            match type_example(ty) {
                Some(ex) => bail!("{required} is required ({ty}, e.g. {ex})"),
                None => bail!("{required} is required ({ty})"),
            }
        }
    }

    // The EVALUATOR decides the pins: the preflight evaluates the exact
    // instantiation in a hidden scratch file, discovering required inputs
    // from its own diagnostics (comprehension-built ios included) and
    // proving each placeholder constructor. The standalone probe supplies
    // constructor hints. Cached per module (a palette arm pre-warms it, so
    // the drop itself is instant). Only when the module doesn't evaluate
    // at all (fixture-grade environments) do the statically visible ios
    // apply.
    let key = preflight_key(workspace_root, stdlib_dir, &req.module);
    let cached = preflight_cache().lock().expect("preflight cache").get(&key).cloned();
    let preflight = match cached {
        Some(p) => Some(p),
        None => {
            let hints =
                crate::catalog::resolve_library_path(&req.module, workspace_root, stdlib_dir)
                    .ok()
                    .and_then(|abs| probe_module_ios(&abs, stdlib_dir).ok())
                    .unwrap_or_default();
            let fresh = preflight_instantiation(
                workspace_root,
                stdlib_dir,
                &req.module,
                &req.attrs,
                &facts,
                &hints,
            )?;
            if let Some(p) = &fresh {
                preflight_cache()
                    .lock()
                    .expect("preflight cache")
                    .insert(key, p.clone());
            }
            fresh
        }
    };
    let ghost = preflight.as_ref().and_then(|p| p.ghost.clone());
    let (io_ctors, needed_loads): (Vec<(String, String)>, BTreeMap<String, String>) =
        match preflight {
            Some(p) => (p.pins, p.loads),
            None => (
                facts
                    .required_ios
                    .iter()
                    .map(|io| (io.clone(), "Net".to_string()))
                    .collect(),
                BTreeMap::new(),
            ),
        };
    // Loads already bound in the board are not re-inserted.
    let needed_loads: BTreeMap<String, String> = needed_loads
        .into_iter()
        .filter(|(sym, _)| !index.bound_idents.contains(sym))
        .collect();

    let mut call = format!("{binding}(name={:?}", req.name);
    for (k, v) in &req.attrs {
        call.push_str(&format!(", {k}={v:?}"));
    }
    let mut placeholder_nets = Vec::new();
    let mut pins = Vec::new();
    for (io, ctor) in &io_ctors {
        pins.push(io.clone());
        if req.attrs.iter().any(|(k, _)| k == io) {
            continue;
        }
        let net = format!("{}_{io}", req.name);
        call.push_str(&format!(", {io}={ctor}({net:?})"));
        placeholder_nets.push(net);
    }
    call.push_str(")\n");

    let last_call_end = index
        .named_calls
        .values()
        .flatten()
        .map(|s| s.stmt.1)
        .max();

    // All insertion offsets are computed against the ORIGINAL content and
    // applied as one back-to-front batch — mixed-order files (a load after
    // Board, say) can't shift an anchor out from under a later insert.
    let call_at = match last_call_end {
        Some(end) => line_boundary_after(&content, end),
        // No calls yet: below the (possibly new) binding area.
        None => match binding_stmt.as_ref().map(|(a, _)| *a) {
            Some(Some(anchor)) => line_boundary_after(&content, anchor),
            _ => index
                .module_bindings
                .iter()
                .map(|b| b.stmt_end)
                .max()
                .or(index.last_load_end)
                .or(index.docstring_end)
                .map(|o| line_boundary_after(&content, o))
                .unwrap_or(0),
        },
    };
    // Keep instantiations above Board(...): if the anchor fell at or past
    // the Board statement, insert at the line Board starts on instead.
    let call_at = match index.board_start {
        Some(b) if call_at > b => line_boundary_before(&content, b),
        _ => call_at,
    };

    let mut edits: Vec<TextEdit> = Vec::new();
    // Interface constructors the placeholders use must be loaded; new load
    // lines join the file's load block (duplicate loads of one module are
    // legal, so this never edits an existing statement).
    if !needed_loads.is_empty() {
        let at = index
            .last_load_end
            .map(|o| line_boundary_after(&content, o))
            .or_else(|| index.docstring_end.map(|o| line_boundary_after(&content, o)))
            .unwrap_or(0)
            .min(call_at);
        let mut block = String::new();
        for (symbol, module) in &needed_loads {
            block.push_str(&format!("load({module:?}, {symbol:?})\n"));
        }
        edits.push(TextEdit {
            start: at,
            end: at,
            text: format!("{}{block}", separator(&content, at)),
        });
    }
    if let Some((anchor, binding_text)) = &binding_stmt {
        let at = match anchor {
            Some(offset) => line_boundary_after(&content, *offset),
            None => 0,
        }
        .min(call_at);
        edits.push(TextEdit {
            start: at,
            end: at,
            text: format!("{}{binding_text}", separator(&content, at)),
        });
    }
    edits.push(TextEdit {
        start: call_at,
        end: call_at,
        text: format!("{}{call}", separator(&content, call_at)),
    });
    let mut text = apply_edits(&content, edits)?;

    // Position write-through, folded into the same write.
    let position_key = match (&req.position, &facts.component_child) {
        (Some(pos), Some(child)) => {
            use crate::layout::AUTHORED_DIVISOR;
            let mut upserts = match sch {
                Some(s) => crate::positions::merge_positions(s, &BTreeMap::new())?,
                None => BTreeMap::new(),
            };
            let key = format!("{}.{child}", req.name);
            upserts.insert(
                key.clone(),
                PositionDoc {
                    x: pos.x * AUTHORED_DIVISOR,
                    y: -pos.y * AUTHORED_DIVISOR,
                    rotation: pos.rotation,
                    mirror: None,
                },
            );
            text = crate::positions::edit_positions_in_text(&text, &upserts, &[]);
            Some(key)
        }
        _ => None,
    };

    // A writer that would produce unparseable source is a bug — fail the
    // gesture, never the board.
    let reparsed = index_content(&rel, &text);
    if let Some(e) = reparsed.error {
        bail!("writer produced invalid source ({e}) — refusing to write");
    }
    let line = reparsed
        .named_calls
        .get(&req.name)
        .and_then(|s| s.first())
        .map(|s| s.line)
        .context("inserted call not found on re-parse")?;

    std::fs::write(zen_file, &text)
        .with_context(|| format!("writing {}", zen_file.display()))?;

    Ok(AddInstanceResult {
        line,
        inserted: call.trim_end().to_string(),
        binding,
        position_key,
        placeholder_nets,
        pins,
        ghost,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct RenameInstanceResult {
    /// `# pcb:sch` keys migrated to the new name.
    pub migrated_positions: Vec<String>,
}

/// Rename the `name="…"` literal of the unique top-level call named `old`,
/// migrating the instance's `# pcb:sch` position keys in the same write —
/// never orphan a position.
pub fn rename_instance(
    zen_file: &Path,
    workspace_root: &Path,
    old: &str,
    new: &str,
) -> Result<RenameInstanceResult> {
    let rel = zen_file
        .strip_prefix(workspace_root)
        .unwrap_or(zen_file)
        .display()
        .to_string();
    if !valid_instance_name(new) {
        bail!(
            "invalid instance name {new:?} (want [A-Za-z][A-Za-z0-9_-]*, max 64)"
        );
    }
    let content = std::fs::read_to_string(zen_file)
        .with_context(|| format!("reading {}", zen_file.display()))?;
    let index = index_content(&rel, &content);
    if let Some(e) = &index.error {
        bail!("{e}");
    }
    let site = match index.named_calls.get(old).map(Vec::as_slice) {
        Some([site]) => site,
        Some(sites) => bail!(
            "ambiguous: {} top-level calls in {rel} carry name={old:?}",
            sites.len()
        ),
        None => bail!("no top-level call with a literal name={old:?} in {rel}"),
    };
    if index.named_calls.contains_key(new) {
        bail!("an instance named {new:?} already exists in {rel}");
    }

    let mut text = content.clone();
    text.replace_range(site.name_literal.0..site.name_literal.1, &format!("{new:?}"));

    // Migrate position keys ("OLD" itself or "OLD.…") to the new name.
    let existing = crate::positions::parse_positions_in_text(&text);
    let mut upserts = BTreeMap::new();
    let mut removals = Vec::new();
    for (key, pos) in existing {
        let suffix = if key == old {
            Some(String::new())
        } else {
            key.strip_prefix(old)
                .and_then(|r| r.starts_with('.').then(|| r.to_string()))
        };
        if let Some(suffix) = suffix {
            upserts.insert(format!("{new}{suffix}"), pos);
            removals.push(key);
        }
    }
    let migrated: Vec<String> = upserts.keys().cloned().collect();
    if !removals.is_empty() {
        text = crate::positions::edit_positions_in_text(&text, &upserts, &removals);
    }

    let reparsed = index_content(&rel, &text);
    if let Some(e) = reparsed.error {
        bail!("writer produced invalid source ({e}) — refusing to write");
    }
    std::fs::write(zen_file, &text)
        .with_context(|| format!("writing {}", zen_file.display()))?;

    Ok(RenameInstanceResult {
        migrated_positions: migrated,
    })
}

// ---------------------------------------------------------------------------
// Net writers (Phase 2)
// ---------------------------------------------------------------------------

/// A batch of non-overlapping span replacements, applied back-to-front.
struct TextEdit {
    start: usize,
    end: usize,
    text: String,
}

fn apply_edits(content: &str, mut edits: Vec<TextEdit>) -> Result<String> {
    edits.sort_by_key(|e| e.start);
    for pair in edits.windows(2) {
        if pair[0].end > pair[1].start {
            bail!(
                "internal: overlapping edits at {}..{} and {}..{}",
                pair[0].start,
                pair[0].end,
                pair[1].start,
                pair[1].end
            );
        }
    }
    let mut text = content.to_string();
    for e in edits.iter().rev() {
        text.replace_range(e.start..e.end, &e.text);
    }
    Ok(text)
}

/// Net names share the instance-name shape; the bound variable is the name
/// with `-` mapped to `_` (variable and net name are ONE identity the
/// writers keep in sync — PRD §7.2).
fn net_variable(name: &str) -> String {
    name.replace('-', "_")
}

fn valid_net_kind(kind: &str) -> Result<()> {
    if !matches!(kind, "Net" | "Power" | "Ground") {
        bail!("kind must be Net, Power, or Ground (got {kind:?})");
    }
    Ok(())
}

/// Where a new net definition inserts: after the last one, else below the
/// module bindings / loads / docstring.
fn net_def_anchor(index: &FileIndex) -> Option<usize> {
    index
        .net_defs
        .iter()
        .map(|d| d.stmt.1)
        .max()
        .or_else(|| index.module_bindings.iter().map(|b| b.stmt_end).max())
        .or(index.last_load_end)
        .or(index.docstring_end)
}

/// Every reference span of top-level variable `var` in the file (assign
/// targets excluded — those are bindings, not references).
fn ident_ref_spans(ast: &AstModule, var: &str) -> Vec<(usize, usize)> {
    fn walk<P: AstPayload>(
        expr: &starlark::syntax::ast::AstExprP<P>,
        var: &str,
        out: &mut Vec<(usize, usize)>,
    ) {
        if let ExprP::Identifier(id) = &expr.node {
            if id.node.ident == var {
                out.push((
                    expr.span.begin().get() as usize,
                    expr.span.end().get() as usize,
                ));
            }
        }
        expr.node.visit_expr(|child| walk(child, var, out));
    }
    let mut out = Vec::new();
    let top: Vec<_> = match &ast.statement().node {
        StmtP::Statements(v) => v.iter().collect(),
        _ => vec![ast.statement()],
    };
    for stmt in top {
        stmt.node.visit_expr(|e| walk(e, var, &mut out));
    }
    out
}

/// Renaming a variable is only safe when nothing rebinds the same name in a
/// nested scope (def/lambda params, for targets, comprehension clauses).
fn ident_is_shadowed(ast: &AstModule, var: &str) -> bool {
    fn target_binds<P: AstPayload>(
        t: &starlark::syntax::ast::AstAssignTargetP<P>,
        var: &str,
    ) -> bool {
        match &t.node {
            AssignTargetP::Identifier(id) => id.node.ident == var,
            AssignTargetP::Tuple(items) => items.iter().any(|i| target_binds(i, var)),
            _ => false,
        }
    }
    fn expr_shadows<P: AstPayload>(
        expr: &starlark::syntax::ast::AstExprP<P>,
        var: &str,
    ) -> bool {
        let own = match &expr.node {
            ExprP::Lambda(l) => l
                .params
                .iter()
                .any(|p| p.node.ident().is_some_and(|i| i.node.ident == var)),
            ExprP::ListComprehension(_, first, clauses) => {
                target_binds(&first.var, var)
                    || clauses.iter().any(|c| match c {
                        starlark::syntax::ast::ClauseP::For(f) => target_binds(&f.var, var),
                        starlark::syntax::ast::ClauseP::If(_) => false,
                    })
            }
            ExprP::DictComprehension(_, first, clauses) => {
                target_binds(&first.var, var)
                    || clauses.iter().any(|c| match c {
                        starlark::syntax::ast::ClauseP::For(f) => target_binds(&f.var, var),
                        starlark::syntax::ast::ClauseP::If(_) => false,
                    })
            }
            _ => false,
        };
        if own {
            return true;
        }
        let mut found = false;
        expr.node.visit_expr(|child| {
            if !found {
                found = expr_shadows(child, var);
            }
        });
        found
    }
    fn stmt_shadows<P: AstPayload>(
        stmt: &starlark::syntax::ast::AstStmtP<P>,
        var: &str,
    ) -> bool {
        let own = match &stmt.node {
            StmtP::Def(def) => def
                .params
                .iter()
                .any(|p| p.node.ident().is_some_and(|i| i.node.ident == var)),
            StmtP::For(f) => target_binds(&f.var, var),
            _ => false,
        };
        if own {
            return true;
        }
        let mut found = false;
        stmt.node.visit_stmt(|child| {
            if !found {
                found = stmt_shadows(child, var);
            }
        });
        if found {
            return true;
        }
        stmt.node.visit_expr(|e| {
            if !found {
                found = expr_shadows(e, var);
            }
        });
        found
    }
    stmt_shadows(ast.statement(), var)
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateNetResult {
    pub variable: String,
    /// 1-based line of the inserted definition.
    pub line: u32,
}

/// Insert `VAR = Kind("NAME")` after the last net definition.
pub fn create_net(
    zen_file: &Path,
    workspace_root: &Path,
    name: &str,
    kind: &str,
) -> Result<CreateNetResult> {
    let rel = zen_file
        .strip_prefix(workspace_root)
        .unwrap_or(zen_file)
        .display()
        .to_string();
    valid_net_kind(kind)?;
    if !valid_instance_name(name) {
        bail!("invalid net name {name:?} (want [A-Za-z][A-Za-z0-9_-]*, max 64)");
    }
    let content = std::fs::read_to_string(zen_file)
        .with_context(|| format!("reading {}", zen_file.display()))?;
    let index = index_content(&rel, &content);
    if let Some(e) = &index.error {
        bail!("{e}");
    }
    if let Some(d) = index.net_defs.iter().find(|d| d.name == name) {
        bail!("a net named {name:?} is already defined at {rel}:{}", d.line);
    }
    let variable = net_variable(name);
    if index.bound_idents.contains(&variable) {
        bail!("the name {variable:?} is already bound in {rel} — pick another net name");
    }

    let at = net_def_anchor(&index)
        .map(|o| line_boundary_after(&content, o))
        .unwrap_or(0);
    let mut text = content.clone();
    let def = format!("{}{variable} = {kind}({name:?})\n", separator(&text, at));
    text.insert_str(at, &def);

    let reparsed = index_content(&rel, &text);
    if let Some(e) = reparsed.error {
        bail!("writer produced invalid source ({e}) — refusing to write");
    }
    let line = reparsed
        .net_defs
        .iter()
        .find(|d| d.name == name)
        .map(|d| d.line)
        .context("inserted definition not found on re-parse")?;
    std::fs::write(zen_file, &text)
        .with_context(|| format!("writing {}", zen_file.display()))?;
    Ok(CreateNetResult { variable, line })
}

#[derive(Debug, Clone, Serialize)]
pub struct RenameNetResult {
    pub variable: String,
    /// Reference sites rewritten (kwargs etc.), the definition excluded.
    pub references: usize,
    pub migrated_positions: Vec<String>,
}

/// Rename a net: the defining assignment's variable AND string literal,
/// plus every reference in the defining file — variable and net name are
/// one identity. Net-symbol position keys (`NAME.2`) migrate in the same
/// write. Refuses when the definition is missing/ambiguous or the variable
/// is shadowed in a nested scope.
pub fn rename_net(
    zen_file: &Path,
    workspace_root: &Path,
    old: &str,
    new: &str,
) -> Result<RenameNetResult> {
    let rel = zen_file
        .strip_prefix(workspace_root)
        .unwrap_or(zen_file)
        .display()
        .to_string();
    if !valid_instance_name(new) {
        bail!("invalid net name {new:?} (want [A-Za-z][A-Za-z0-9_-]*, max 64)");
    }
    let content = std::fs::read_to_string(zen_file)
        .with_context(|| format!("reading {}", zen_file.display()))?;
    let index = index_content(&rel, &content);
    if let Some(e) = &index.error {
        bail!("{e}");
    }
    let defs: Vec<&NetDef> = index.net_defs.iter().filter(|d| d.name == old).collect();
    let def = match defs.as_slice() {
        [d] => *d,
        [] => bail!("no top-level net definition for {old:?} in {rel}"),
        many => bail!("ambiguous: {} definitions of {old:?} in {rel}", many.len()),
    };
    if index.net_defs.iter().any(|d| d.name == new) {
        bail!("a net named {new:?} already exists in {rel}");
    }
    let old_var = def.variable.clone();
    let new_var = net_variable(new);
    if new_var != old_var && index.bound_idents.contains(&new_var) {
        bail!("the name {new_var:?} is already bound in {rel} — pick another net name");
    }
    let ast = parse(&rel, &content)?;
    if ident_is_shadowed(&ast, &old_var) {
        bail!(
            "{old_var:?} is rebound in a nested scope (def/lambda/for/comprehension) — \
             renaming it structurally is ambiguous; edit the source or ask the agent"
        );
    }

    let mut edits = vec![TextEdit {
        start: def.string_span.0,
        end: def.string_span.1,
        text: format!("{new:?}"),
    }];
    let refs = ident_ref_spans(&ast, &old_var);
    let references = refs.len();
    if new_var != old_var {
        edits.push(TextEdit {
            start: def.ident_span.0,
            end: def.ident_span.1,
            text: new_var.clone(),
        });
        for (start, end) in refs {
            edits.push(TextEdit {
                start,
                end,
                text: new_var.clone(),
            });
        }
    }
    let mut text = apply_edits(&content, edits)?;

    // Net-symbol position keys: `NAME` or `NAME.<n>` (numeric suffixes only
    // — dotted component keys like `R1.R` must never match a net rename).
    let existing = crate::positions::parse_positions_in_text(&text);
    let mut upserts = BTreeMap::new();
    let mut removals = Vec::new();
    for (key, pos) in existing {
        let suffix = if key == old {
            Some(String::new())
        } else {
            key.strip_prefix(old).and_then(|r| {
                (r.starts_with('.') && r[1..].chars().all(|c| c.is_ascii_digit()))
                    .then(|| r.to_string())
            })
        };
        if let Some(suffix) = suffix {
            upserts.insert(format!("{new}{suffix}"), pos);
            removals.push(key);
        }
    }
    let migrated: Vec<String> = upserts.keys().cloned().collect();
    if !removals.is_empty() {
        text = crate::positions::edit_positions_in_text(&text, &upserts, &removals);
    }

    let reparsed = index_content(&rel, &text);
    if let Some(e) = reparsed.error {
        bail!("writer produced invalid source ({e}) — refusing to write");
    }
    std::fs::write(zen_file, &text)
        .with_context(|| format!("writing {}", zen_file.display()))?;
    Ok(RenameNetResult {
        variable: new_var,
        references,
        migrated_positions: migrated,
    })
}

#[derive(Debug, Clone, Deserialize)]
pub struct AttachPinRequest {
    /// Top-level instance name in the target file (the editability anchor's
    /// local name, e.g. `R5`).
    pub instance: String,
    /// Component pin name (`1`) or io identifier (`P1`) — pins map through
    /// the module's literal `pins={…}` dict.
    pub pin: String,
    /// Net identity (the string name). An existing definition is reused
    /// regardless of kind; otherwise one is created.
    pub net_name: String,
    /// `Net` | `Power` | `Ground` — used only when creating.
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AttachPinResult {
    /// The io kwarg the pin resolved to.
    pub io: String,
    pub variable: String,
    pub created_def: bool,
    /// Net definitions pruned because this reattachment orphaned them.
    pub pruned_defs: Vec<String>,
}

/// Attach one pin to a net: ensure the definition, set the call's io kwarg
/// to the net variable, and prune a definition the replacement orphaned
/// (auto-prune, PRD §12.2) — one write.
pub fn attach_pin_net(
    zen_file: &Path,
    workspace_root: &Path,
    stdlib_dir: &Path,
    req: &AttachPinRequest,
) -> Result<AttachPinResult> {
    let rel = zen_file
        .strip_prefix(workspace_root)
        .unwrap_or(zen_file)
        .display()
        .to_string();
    valid_net_kind(&req.kind)?;
    if !valid_instance_name(&req.net_name) {
        bail!(
            "invalid net name {:?} (want [A-Za-z][A-Za-z0-9_-]*, max 64)",
            req.net_name
        );
    }
    let content = std::fs::read_to_string(zen_file)
        .with_context(|| format!("reading {}", zen_file.display()))?;
    let index = index_content(&rel, &content);
    if let Some(e) = &index.error {
        bail!("{e}");
    }
    let site = match index.named_calls.get(&req.instance).map(Vec::as_slice) {
        Some([site]) => site,
        Some(sites) => bail!(
            "ambiguous: {} top-level calls in {rel} carry name={:?}",
            sites.len(),
            req.instance
        ),
        None => bail!(
            "no top-level call with a literal name={:?} in {rel} — the instance may be \
             generated; edit the source or ask the agent",
            req.instance
        ),
    };
    let Some(binding) = index
        .module_bindings
        .iter()
        .find(|b| b.ident == site.callee)
    else {
        bail!(
            "{} is not instantiated through a Module binding — pins can't be resolved",
            req.instance
        );
    };
    let facts = module_facts(&binding.spec, workspace_root, stdlib_dir);
    let existing_kwarg = |name: &str| {
        site.kwargs.iter().any(|k| k.name == name)
            && name != "name"
            && !facts.config_types.contains_key(name)
    };
    let io = if facts.required_ios.iter().any(|i| i == &req.pin)
        || facts.pin_map.values().any(|i| i == &req.pin)
        // The placed call already carries this kwarg — proof enough even
        // when the module builds its ios in a comprehension (NetTie).
        || existing_kwarg(&req.pin)
    {
        req.pin.clone()
    } else if let Some(io) = facts.pin_map.get(&req.pin) {
        io.clone()
    } else {
        bail!(
            "pin {:?} does not resolve on {} ({}): known pins {:?}, ios {:?}",
            req.pin,
            req.instance,
            binding.spec,
            facts.pin_map.keys().collect::<Vec<_>>(),
            facts.required_ios,
        );
    };

    // The net: reuse an existing definition (typed or not), else create.
    let existing_def = index.net_defs.iter().find(|d| d.name == req.net_name);
    let (variable, created_def) = match existing_def {
        Some(d) => (d.variable.clone(), false),
        None => {
            let variable = net_variable(&req.net_name);
            if index.bound_idents.contains(&variable) {
                bail!(
                    "the name {variable:?} is already bound in {rel} — pick another net name"
                );
            }
            (variable, true)
        }
    };

    let mut edits = Vec::new();
    if created_def {
        // The definition must precede the call it's referenced from.
        let at = net_def_anchor(&index)
            .map(|o| line_boundary_after(&content, o))
            .unwrap_or(0)
            .min(line_boundary_before(&content, site.stmt.0));
        edits.push(TextEdit {
            start: at,
            end: at,
            text: format!(
                "{}{variable} = {}({:?})\n",
                separator(&content, at),
                req.kind,
                req.net_name
            ),
        });
    }

    let old_ident = match site.kwargs.iter().find(|k| k.name == io) {
        Some(kwarg) => {
            edits.push(TextEdit {
                start: kwarg.value_span.0,
                end: kwarg.value_span.1,
                text: variable.clone(),
            });
            kwarg.value_ident.clone()
        }
        None => {
            edits.push(TextEdit {
                start: site.last_arg_end,
                end: site.last_arg_end,
                text: format!(", {io}={variable}"),
            });
            None
        }
    };

    // Auto-prune: a replaced net variable whose ONLY reference was this
    // kwarg loses its definition in the same write.
    let mut pruned_defs = Vec::new();
    if let Some(old_var) = old_ident {
        if old_var != variable {
            if let Some(old_def) = index.net_defs.iter().find(|d| d.variable == old_var) {
                let ast = parse(&rel, &content)?;
                if ident_ref_spans(&ast, &old_var).len() == 1 {
                    let mut start = line_boundary_before(&content, old_def.stmt.0);
                    // Absorb one preceding blank line so the deletion never
                    // leaves a double blank behind.
                    if content[..start].ends_with("\n\n") {
                        start -= 1;
                    }
                    edits.push(TextEdit {
                        start,
                        end: line_boundary_after(&content, old_def.stmt.1),
                        text: String::new(),
                    });
                    pruned_defs.push(old_def.name.clone());
                }
            }
        }
    }

    let text = apply_edits(&content, edits)?;
    let reparsed = index_content(&rel, &text);
    if let Some(e) = reparsed.error {
        bail!("writer produced invalid source ({e}) — refusing to write");
    }
    std::fs::write(zen_file, &text)
        .with_context(|| format!("writing {}", zen_file.display()))?;
    Ok(AttachPinResult {
        io,
        variable,
        created_def,
        pruned_defs,
    })
}

// ---------------------------------------------------------------------------
// Wiring writers (Phase 3)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct PinEndpoint {
    /// Top-level instance name in the target file.
    pub instance: String,
    /// Component pin name or io identifier.
    pub pin: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConnectPinsRequest {
    pub a: PinEndpoint,
    pub b: PinEndpoint,
    /// Explicit name for a created net (default: pin-derived from `a`).
    pub net: Option<String>,
    /// Permit a net merge (both endpoints on different shared nets). When
    /// false, such a connect returns [`ConnectOutcome::NeedsMerge`] so the
    /// caller can confirm — never silently (PRD §12.4).
    #[serde(default)]
    pub allow_merge: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum ConnectOutcome {
    Applied {
        net: String,
        variable: String,
        created_def: bool,
        /// The pins were already on this net — nothing was written.
        already: bool,
        /// A merge happened: the net that disappeared.
        merged_from: Option<String>,
        /// Reference sites rewritten by the merge.
        moved_refs: usize,
        pruned_defs: Vec<String>,
    },
    NeedsMerge {
        /// The net that would disappear (b's side).
        from: String,
        into: String,
        /// How many reference sites would move.
        from_refs: usize,
    },
}

/// How a pin's kwarg currently reads.
enum PinState {
    Unbound,
    /// Inline `Net("X")` placeholder from placement.
    Placeholder { name: String },
    /// A reference to a top-level net definition.
    Def {
        variable: String,
        name: String,
        refs: usize,
    },
}

/// Resolve one endpoint on an indexed file: the call site, the io kwarg
/// name, and the pin's current net state.
fn resolve_endpoint<'a>(
    index: &'a FileIndex,
    ast: &AstModule,
    workspace_root: &Path,
    stdlib_dir: &Path,
    rel: &str,
    ep: &PinEndpoint,
) -> Result<(&'a CallSite, String, PinState)> {
    let site = match index.named_calls.get(&ep.instance).map(Vec::as_slice) {
        Some([site]) => site,
        Some(sites) => bail!(
            "ambiguous: {} top-level calls in {rel} carry name={:?}",
            sites.len(),
            ep.instance
        ),
        None => bail!(
            "no top-level call with a literal name={:?} in {rel} — the instance may be \
             generated; edit the source or ask the agent",
            ep.instance
        ),
    };
    let Some(binding) = index
        .module_bindings
        .iter()
        .find(|b| b.ident == site.callee)
    else {
        bail!(
            "{} is not instantiated through a Module binding — pins can't be resolved",
            ep.instance
        );
    };
    let facts = module_facts(&binding.spec, workspace_root, stdlib_dir);
    let existing_kwarg = |name: &str| {
        site.kwargs.iter().any(|k| k.name == name)
            && name != "name"
            && !facts.config_types.contains_key(name)
    };
    let io = if facts.required_ios.iter().any(|i| i == &ep.pin)
        || facts.pin_map.values().any(|i| i == &ep.pin)
        // The call already carries this kwarg — proof enough even for
        // comprehension-built ios (NetTie).
        || existing_kwarg(&ep.pin)
    {
        ep.pin.clone()
    } else if let Some(io) = facts.pin_map.get(&ep.pin) {
        io.clone()
    } else {
        bail!(
            "pin {:?} does not resolve on {} ({}): known pins {:?}, ios {:?}",
            ep.pin,
            ep.instance,
            binding.spec,
            facts.pin_map.keys().collect::<Vec<_>>(),
            facts.required_ios,
        );
    };
    let state = match site.kwargs.iter().find(|k| k.name == io) {
        None => PinState::Unbound,
        Some(kwarg) => {
            if let Some(var) = &kwarg.value_ident {
                match index.net_defs.iter().find(|d| &d.variable == var) {
                    Some(def) => PinState::Def {
                        variable: var.clone(),
                        name: def.name.clone(),
                        refs: ident_ref_spans(ast, var).len(),
                    },
                    None => bail!(
                        "{}'s {io} is bound to {var:?}, which is not a top-level net \
                         definition — edit the source or ask the agent",
                        ep.instance
                    ),
                }
            } else if let Some((_, name)) = &kwarg.value_net_literal {
                PinState::Placeholder { name: name.clone() }
            } else {
                bail!(
                    "{}'s {io} is a computed expression — edit the source or ask the agent",
                    ep.instance
                );
            }
        }
    };
    Ok((site, io, state))
}

/// Which port of `instance`'s call (in this file) carries `net_name`?
/// Reads the call's kwargs: an identifier resolves through the file's net
/// definitions, an inline `Net("…")` placeholder matches by its literal.
pub fn port_for_net(
    zen_file: &Path,
    workspace_root: &Path,
    instance: &str,
    net_name: &str,
) -> Result<Option<String>> {
    let rel = zen_file
        .strip_prefix(workspace_root)
        .unwrap_or(zen_file)
        .display()
        .to_string();
    let content = std::fs::read_to_string(zen_file)
        .with_context(|| format!("reading {}", zen_file.display()))?;
    let index = index_content(&rel, &content);
    if let Some(e) = &index.error {
        bail!("{e}");
    }
    let Some([site]) = index.named_calls.get(instance).map(Vec::as_slice) else {
        return Ok(None);
    };
    for kwarg in &site.kwargs {
        if let Some(var) = &kwarg.value_ident {
            if index
                .net_defs
                .iter()
                .any(|d| &d.variable == var && d.name == net_name)
            {
                return Ok(Some(kwarg.name.clone()));
            }
        }
        if let Some((_, literal)) = &kwarg.value_net_literal {
            if literal == net_name {
                return Ok(Some(kwarg.name.clone()));
            }
        }
    }
    Ok(None)
}

/// Wire-to-inner-pin, resolved the way a human means it: a pin deep inside
/// a module sits on some net — when that net flows OUT through one of the
/// module's ports, the board-level endpoint is (module instance, port),
/// and wiring there is an ordinary entry-file edit. `None` means the net
/// stays internal to the module (exposing it is an interface change the
/// canvas won't do silently).
pub fn translate_endpoint_via_port(
    sch: &SchematicDoc,
    entry_file: &Path,
    workspace_root: &Path,
    path: &str,
    pin: &str,
) -> Result<Option<PinEndpoint>> {
    let Some(inst) = sch.instance(path) else {
        return Ok(None);
    };
    let Some(net) = inst
        .pins
        .iter()
        .find(|p| p.name == pin)
        .and_then(|p| p.net.clone())
    else {
        return Ok(None);
    };
    let mut segs = path.split('.');
    let (Some("root"), Some(top)) = (segs.next(), segs.next()) else {
        return Ok(None);
    };
    Ok(port_for_net(entry_file, workspace_root, top, &net)?.map(|port| PinEndpoint {
        instance: top.to_string(),
        pin: port,
    }))
}

/// The compound wiring edit (the PRD's endorsed example): choose or create
/// the shared net, point both pin kwargs at it, prune what that orphans —
/// one write. A connect that would merge two shared nets requires
/// `allow_merge` and otherwise reports back for confirmation.
///
/// `sch` (the last good build) ranks connectedness by REAL port counts —
/// a net passed into a submodule is single-referenced in this file but
/// spans many pins; without it only file-local references count.
pub fn connect_pins(
    zen_file: &Path,
    workspace_root: &Path,
    stdlib_dir: &Path,
    sch: Option<&SchematicDoc>,
    req: &ConnectPinsRequest,
) -> Result<ConnectOutcome> {
    let rel = zen_file
        .strip_prefix(workspace_root)
        .unwrap_or(zen_file)
        .display()
        .to_string();
    let content = std::fs::read_to_string(zen_file)
        .with_context(|| format!("reading {}", zen_file.display()))?;
    let index = index_content(&rel, &content);
    if let Some(e) = &index.error {
        bail!("{e}");
    }
    let ast = parse(&rel, &content)?;
    let (site_a, io_a, state_a) =
        resolve_endpoint(&index, &ast, workspace_root, stdlib_dir, &rel, &req.a)?;
    let (site_b, io_b, state_b) =
        resolve_endpoint(&index, &ast, workspace_root, stdlib_dir, &rel, &req.b)?;
    if req.a.instance == req.b.instance && io_a == io_b {
        bail!("both endpoints are the same pin ({}.{io_a})", req.a.instance);
    }

    // Already on one net?
    if let (
        PinState::Def {
            variable: va,
            name: na,
            ..
        },
        PinState::Def { variable: vb, .. },
    ) = (&state_a, &state_b)
    {
        if va == vb {
            return Ok(ConnectOutcome::Applied {
                net: na.clone(),
                variable: va.clone(),
                created_def: false,
                already: true,
                merged_from: None,
                moved_refs: 0,
                pruned_defs: Vec::new(),
            });
        }
    }

    let shared = |s: &PinState| match s {
        PinState::Def {
            refs, name, ..
        } => {
            *refs > 1
                || sch
                    .and_then(|s| s.nets.get(name))
                    .is_some_and(|n| n.ports.len() > 1)
        }
        _ => false,
    };

    // Two shared nets: a merge. Confirmed or reported, never silent.
    if shared(&state_a) && shared(&state_b) {
        let (PinState::Def {
            variable: var_a,
            name: name_a,
            ..
        }, PinState::Def {
            variable: var_b,
            name: name_b,
            refs: refs_b,
        }) = (&state_a, &state_b)
        else {
            unreachable!("shared() checked Def");
        };
        if !req.allow_merge {
            return Ok(ConnectOutcome::NeedsMerge {
                from: name_b.clone(),
                into: name_a.clone(),
                from_refs: *refs_b,
            });
        }
        if ident_is_shadowed(&ast, var_b) {
            bail!(
                "{var_b:?} is rebound in a nested scope — merging it structurally is \
                 ambiguous; edit the source or ask the agent"
            );
        }
        let def_b = index
            .net_defs
            .iter()
            .find(|d| &d.variable == var_b)
            .expect("Def state has a def");
        let mut edits = Vec::new();
        let refs = ident_ref_spans(&ast, var_b);
        let moved_refs = refs.len();
        for (start, end) in refs {
            edits.push(TextEdit {
                start,
                end,
                text: var_a.clone(),
            });
        }
        let mut start = line_boundary_before(&content, def_b.stmt.0);
        if content[..start].ends_with("\n\n") {
            start -= 1;
        }
        edits.push(TextEdit {
            start,
            end: line_boundary_after(&content, def_b.stmt.1),
            text: String::new(),
        });
        let mut text = apply_edits(&content, edits)?;
        // The vanished net's symbol positions go with it.
        let removals: Vec<String> = crate::positions::parse_positions_in_text(&text)
            .into_keys()
            .filter(|k| {
                k == name_b
                    || k.strip_prefix(name_b.as_str())
                        .is_some_and(|r| r.starts_with('.') && r[1..].chars().all(|c| c.is_ascii_digit()))
            })
            .collect();
        if !removals.is_empty() {
            text = crate::positions::edit_positions_in_text(&text, &BTreeMap::new(), &removals);
        }
        let reparsed = index_content(&rel, &text);
        if let Some(e) = reparsed.error {
            bail!("writer produced invalid source ({e}) — refusing to write");
        }
        std::fs::write(zen_file, &text)
            .with_context(|| format!("writing {}", zen_file.display()))?;
        return Ok(ConnectOutcome::Applied {
            net: name_a.clone(),
            variable: var_a.clone(),
            created_def: false,
            already: false,
            merged_from: Some(name_b.clone()),
            moved_refs,
            pruned_defs: vec![name_b.clone()],
        });
    }

    // Otherwise: pick the target net — a shared def wins, then a
    // deliberately-named def (not `{inst}_{io}`-shaped), then a's side —
    // or create one, named from the drag-start pin (PRD §12.1).
    let placeholder_shaped = |name: &str, ep: &PinEndpoint, io: &str| {
        name == format!("{}_{io}", ep.instance)
    };
    let candidates: Vec<(&PinEndpoint, &str, &PinState)> = [
        (&req.a, io_a.as_str(), &state_a),
        (&req.b, io_b.as_str(), &state_b),
    ]
    .into_iter()
    .filter(|(_, _, s)| matches!(s, PinState::Def { .. }))
    .collect();
    let target = candidates
        .iter()
        .find(|(_, _, s)| shared(s))
        .or_else(|| {
            candidates.iter().find(|(ep, io, s)| {
                matches!(s, PinState::Def { name, .. } if !placeholder_shaped(name, ep, io))
            })
        })
        .or_else(|| candidates.first());

    let mut edits = Vec::new();
    let mut replaced_idents: BTreeMap<String, usize> = BTreeMap::new();
    let (target_net, target_var, created_def) = match target {
        Some((_, _, PinState::Def { variable, name, .. })) => {
            (name.clone(), variable.clone(), false)
        }
        _ => {
            // No definition on either side: create one.
            let name = req
                .net
                .clone()
                .or_else(|| match &state_a {
                    PinState::Placeholder { name } => Some(name.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| format!("{}_{io_a}", req.a.instance));
            if !valid_instance_name(&name) {
                bail!("invalid net name {name:?} (want [A-Za-z][A-Za-z0-9_-]*, max 64)");
            }
            match index.net_defs.iter().find(|d| d.name == name) {
                Some(d) => (d.name.clone(), d.variable.clone(), false),
                None => {
                    let variable = net_variable(&name);
                    if index.bound_idents.contains(&variable) {
                        bail!(
                            "the name {variable:?} is already bound in {rel} — pass an \
                             explicit net name"
                        );
                    }
                    let at = net_def_anchor(&index)
                        .map(|o| line_boundary_after(&content, o))
                        .unwrap_or(0)
                        .min(line_boundary_before(
                            &content,
                            site_a.stmt.0.min(site_b.stmt.0),
                        ));
                    edits.push(TextEdit {
                        start: at,
                        end: at,
                        text: format!(
                            "{}{variable} = Net({name:?})\n",
                            separator(&content, at)
                        ),
                    });
                    (name, variable, true)
                }
            }
        }
    };

    for (site, io, state) in [(site_a, &io_a, &state_a), (site_b, &io_b, &state_b)] {
        match state {
            PinState::Def { variable, .. } if variable == &target_var => continue,
            _ => {}
        }
        match site.kwargs.iter().find(|k| &k.name == io) {
            Some(kwarg) => {
                edits.push(TextEdit {
                    start: kwarg.value_span.0,
                    end: kwarg.value_span.1,
                    text: target_var.clone(),
                });
                if let Some(old) = &kwarg.value_ident {
                    *replaced_idents.entry(old.clone()).or_default() += 1;
                }
            }
            None => edits.push(TextEdit {
                start: site.last_arg_end,
                end: site.last_arg_end,
                text: format!(", {io}={target_var}"),
            }),
        }
    }

    // Auto-prune every definition these replacements orphaned.
    let mut pruned_defs = Vec::new();
    for (old_var, replaced) in &replaced_idents {
        if old_var == &target_var {
            continue;
        }
        let Some(def) = index.net_defs.iter().find(|d| &d.variable == old_var) else {
            continue;
        };
        if ident_ref_spans(&ast, old_var).len() == *replaced {
            let mut start = line_boundary_before(&content, def.stmt.0);
            if content[..start].ends_with("\n\n") {
                start -= 1;
            }
            edits.push(TextEdit {
                start,
                end: line_boundary_after(&content, def.stmt.1),
                text: String::new(),
            });
            pruned_defs.push(def.name.clone());
        }
    }

    let text = apply_edits(&content, edits)?;
    let reparsed = index_content(&rel, &text);
    if let Some(e) = reparsed.error {
        bail!("writer produced invalid source ({e}) — refusing to write");
    }
    std::fs::write(zen_file, &text)
        .with_context(|| format!("writing {}", zen_file.display()))?;
    Ok(ConnectOutcome::Applied {
        net: target_net,
        variable: target_var,
        created_def,
        already: false,
        merged_from: None,
        moved_refs: 0,
        pruned_defs,
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct DisconnectResult {
    pub io: String,
    /// Required pins revert to a fresh placeholder net of this name;
    /// optional pins drop the kwarg entirely.
    pub placeholder: Option<String>,
    pub pruned_defs: Vec<String>,
}

/// Detach one pin from its net: required ios get a fresh placeholder
/// (`Net("{inst}_{io}")`), optional ios lose the kwarg; a definition the
/// detachment orphaned prunes in the same write.
pub fn disconnect_pin(
    zen_file: &Path,
    workspace_root: &Path,
    stdlib_dir: &Path,
    instance: &str,
    pin: &str,
) -> Result<DisconnectResult> {
    let rel = zen_file
        .strip_prefix(workspace_root)
        .unwrap_or(zen_file)
        .display()
        .to_string();
    let content = std::fs::read_to_string(zen_file)
        .with_context(|| format!("reading {}", zen_file.display()))?;
    let index = index_content(&rel, &content);
    if let Some(e) = &index.error {
        bail!("{e}");
    }
    let ast = parse(&rel, &content)?;
    let ep = PinEndpoint {
        instance: instance.to_string(),
        pin: pin.to_string(),
    };
    let (site, io, state) =
        resolve_endpoint(&index, &ast, workspace_root, stdlib_dir, &rel, &ep)?;
    let kwarg = match &state {
        PinState::Unbound => bail!("{instance}.{io} is already unconnected"),
        PinState::Placeholder { .. } => {
            bail!("{instance}.{io} is already unconnected (placeholder net)")
        }
        PinState::Def { .. } => site
            .kwargs
            .iter()
            .find(|k| k.name == io)
            .expect("Def state has a kwarg"),
    };

    let required = {
        let binding = index
            .module_bindings
            .iter()
            .find(|b| b.ident == site.callee)
            .expect("resolve_endpoint checked");
        module_facts(&binding.spec, workspace_root, stdlib_dir)
            .required_ios
            .iter()
            .any(|i| i == &io)
    };

    let mut edits = Vec::new();
    let placeholder = if required {
        let name = format!("{instance}_{io}");
        edits.push(TextEdit {
            start: kwarg.value_span.0,
            end: kwarg.value_span.1,
            text: format!("Net({name:?})"),
        });
        Some(name)
    } else {
        if kwarg.prev_end == 0 {
            bail!("internal: pin kwarg with no preceding argument");
        }
        edits.push(TextEdit {
            start: kwarg.prev_end,
            end: kwarg.arg_span.1,
            text: String::new(),
        });
        None
    };

    let mut pruned_defs = Vec::new();
    if let PinState::Def { variable, name, .. } = &state {
        if ident_ref_spans(&ast, variable).len() == 1 {
            let def = index
                .net_defs
                .iter()
                .find(|d| &d.variable == variable)
                .expect("Def state has a def");
            let mut start = line_boundary_before(&content, def.stmt.0);
            if content[..start].ends_with("\n\n") {
                start -= 1;
            }
            edits.push(TextEdit {
                start,
                end: line_boundary_after(&content, def.stmt.1),
                text: String::new(),
            });
            pruned_defs.push(name.clone());
        }
    }

    let text = apply_edits(&content, edits)?;
    let reparsed = index_content(&rel, &text);
    if let Some(e) = reparsed.error {
        bail!("writer produced invalid source ({e}) — refusing to write");
    }
    std::fs::write(zen_file, &text)
        .with_context(|| format!("writing {}", zen_file.display()))?;
    Ok(DisconnectResult {
        io,
        placeholder,
        pruned_defs,
    })
}

// ---------------------------------------------------------------------------
// Attribute + removal writers (Phase 4)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct SetAttributeResult {
    /// The kwarg existed and its value was replaced (vs newly added).
    pub replaced: bool,
}

/// Replace (or add) one attribute kwarg's value as a string literal.
/// Pins are refused toward the wiring verbs; `name` toward rename.
pub fn set_attribute(
    zen_file: &Path,
    workspace_root: &Path,
    stdlib_dir: &Path,
    instance: &str,
    key: &str,
    value: &str,
) -> Result<SetAttributeResult> {
    let rel = zen_file
        .strip_prefix(workspace_root)
        .unwrap_or(zen_file)
        .display()
        .to_string();
    if key == "name" {
        bail!("`name` is the instance's identity — use rename_instance");
    }
    if !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') || key.is_empty() {
        bail!("invalid attribute name {key:?}");
    }
    let content = std::fs::read_to_string(zen_file)
        .with_context(|| format!("reading {}", zen_file.display()))?;
    let index = index_content(&rel, &content);
    if let Some(e) = &index.error {
        bail!("{e}");
    }
    let site = match index.named_calls.get(instance).map(Vec::as_slice) {
        Some([site]) => site,
        Some(sites) => bail!(
            "ambiguous: {} top-level calls in {rel} carry name={instance:?}",
            sites.len()
        ),
        None => bail!(
            "no top-level call with a literal name={instance:?} in {rel} — the instance may \
             be generated; edit the source or ask the agent"
        ),
    };
    if let Some(binding) = index
        .module_bindings
        .iter()
        .find(|b| b.ident == site.callee)
    {
        let facts = module_facts(&binding.spec, workspace_root, stdlib_dir);
        if facts.required_ios.iter().any(|i| i == key)
            || facts.pin_map.values().any(|i| i == key)
        {
            bail!("{key} is a pin — use connect_pins / disconnect_pin");
        }
    }

    let (edit, replaced) = match site.kwargs.iter().find(|k| k.name == key) {
        Some(kwarg) => (
            TextEdit {
                start: kwarg.value_span.0,
                end: kwarg.value_span.1,
                text: format!("{value:?}"),
            },
            true,
        ),
        None => (
            TextEdit {
                start: site.last_arg_end,
                end: site.last_arg_end,
                text: format!(", {key}={value:?}"),
            },
            false,
        ),
    };
    let text = apply_edits(&content, vec![edit])?;
    let reparsed = index_content(&rel, &text);
    if let Some(e) = reparsed.error {
        bail!("writer produced invalid source ({e}) — refusing to write");
    }
    std::fs::write(zen_file, &text)
        .with_context(|| format!("writing {}", zen_file.display()))?;
    Ok(SetAttributeResult { replaced })
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoveInstancesResult {
    pub removed: Vec<String>,
    /// Net definitions orphaned by the removal, pruned in the same write.
    pub pruned_nets: Vec<String>,
    /// Module bindings no longer used by anything, pruned too.
    pub pruned_bindings: Vec<String>,
    /// `# pcb:sch` keys dropped.
    pub removed_positions: Vec<String>,
}

/// Delete instances (one write, batch): the call statements go, net
/// definitions and Module bindings that nothing else references are pruned
/// (auto-prune, PRD §12.2), and the instances' position keys are dropped so
/// the block never carries orphans.
pub fn remove_instances(
    zen_file: &Path,
    workspace_root: &Path,
    names: &[String],
) -> Result<RemoveInstancesResult> {
    let rel = zen_file
        .strip_prefix(workspace_root)
        .unwrap_or(zen_file)
        .display()
        .to_string();
    if names.is_empty() {
        bail!("nothing to remove");
    }
    let content = std::fs::read_to_string(zen_file)
        .with_context(|| format!("reading {}", zen_file.display()))?;
    let index = index_content(&rel, &content);
    if let Some(e) = &index.error {
        bail!("{e}");
    }
    let ast = parse(&rel, &content)?;

    let mut spans = Vec::new();
    let mut callees = std::collections::BTreeSet::new();
    for name in names {
        let site = match index.named_calls.get(name).map(Vec::as_slice) {
            Some([site]) => site,
            Some(sites) => bail!(
                "ambiguous: {} top-level calls in {rel} carry name={name:?}",
                sites.len()
            ),
            None => bail!(
                "no top-level call with a literal name={name:?} in {rel} — the instance may \
                 be generated; edit the source or ask the agent"
            ),
        };
        spans.push(site.stmt);
        callees.insert(site.callee.clone());
    }
    let inside = |s: (usize, usize)| spans.iter().any(|d| s.0 >= d.0 && s.1 <= d.1);

    let mut edits: Vec<TextEdit> = Vec::new();
    let mut delete_stmt = |content: &str, stmt: (usize, usize)| {
        let mut start = line_boundary_before(content, stmt.0);
        if content[..start].ends_with("\n\n") {
            start -= 1;
        }
        TextEdit {
            start,
            end: line_boundary_after(content, stmt.1),
            text: String::new(),
        }
    };
    for span in &spans {
        edits.push(delete_stmt(&content, *span));
    }

    // Orphan pruning by tally: a definition dies when every reference to it
    // sat inside a deleted statement.
    let orphaned = |var: &str| {
        let refs = ident_ref_spans(&ast, var);
        !refs.is_empty() && refs.iter().all(|r| inside(*r))
    };
    let mut pruned_nets = Vec::new();
    for def in &index.net_defs {
        if orphaned(&def.variable) {
            edits.push(delete_stmt(&content, def.stmt));
            pruned_nets.push(def.name.clone());
        }
    }
    let mut pruned_bindings = Vec::new();
    for binding in &index.module_bindings {
        if callees.contains(&binding.ident) && orphaned(&binding.ident) {
            edits.push(delete_stmt(&content, binding.stmt));
            pruned_bindings.push(binding.ident.clone());
        }
    }

    let mut text = apply_edits(&content, edits)?;

    // Positions: the instances' keys (`NAME` or `NAME.…`) go with them.
    let removals: Vec<String> = crate::positions::parse_positions_in_text(&text)
        .into_keys()
        .filter(|k| {
            names.iter().any(|n| {
                k == n || k.strip_prefix(n.as_str()).is_some_and(|r| r.starts_with('.'))
            })
        })
        .collect();
    if !removals.is_empty() {
        text = crate::positions::edit_positions_in_text(&text, &BTreeMap::new(), &removals);
    }

    let reparsed = index_content(&rel, &text);
    if let Some(e) = reparsed.error {
        bail!("writer produced invalid source ({e}) — refusing to write");
    }
    std::fs::write(zen_file, &text)
        .with_context(|| format!("writing {}", zen_file.display()))?;
    Ok(RemoveInstancesResult {
        removed: names.to_vec(),
        pruned_nets,
        pruned_bindings,
        removed_positions: removals,
    })
}

/// Offset just past the newline ending the line that contains `offset`
/// (EOF-safe; appends a newline first when the file doesn't end with one).
fn line_boundary_after(text: &str, offset: usize) -> usize {
    match text[offset.min(text.len())..].find('\n') {
        Some(n) => offset + n + 1,
        None => text.len(),
    }
}

/// Offset of the start of the line containing `offset`.
fn line_boundary_before(text: &str, offset: usize) -> usize {
    match text[..offset.min(text.len())].rfind('\n') {
        Some(n) => n + 1,
        None => 0,
    }
}

/// A blank-line separator when the insertion point follows non-blank text —
/// matches the file's statement-group style without reformatting anything.
fn separator(text: &str, at: usize) -> &'static str {
    if at == 0 || text[..at].ends_with("\n\n") {
        ""
    } else {
        "\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{InstanceDoc, NetDoc, PinDoc};

    fn instance(path: &str, kind: InstanceKind, source_file: Option<&str>) -> InstanceDoc {
        InstanceDoc {
            path: path.into(),
            kind,
            type_name: path.rsplit('.').next().unwrap().into(),
            source_file: source_file.map(str::to_string),
            refdes: None,
            attributes: BTreeMap::new(),
            children: BTreeMap::new(),
            pins: vec![PinDoc {
                name: "P1".into(),
                net: None,
            }],
            position: None,
        }
    }

    fn net(name: &str, kind: &str) -> NetDoc {
        NetDoc {
            name: name.into(),
            kind: kind.into(),
            ports: vec![],
        }
    }

    fn workspace(files: &[(&str, &str)]) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "etch-edit-{}-{:p}",
            std::process::id(),
            &files[0].0
        ));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, content) in files {
            let p = dir.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, content).unwrap();
        }
        dir
    }

    /// The demo-board shape: literal calls and net assignments classify
    /// editable with the right file/line; stdlib internals anchor to their
    /// authored ancestor.
    #[test]
    fn demo_shape_classifies_editable() {
        let board = r#"load("@stdlib/interfaces.zen", "Analog")

Resistor = Module("@stdlib/generics/Resistor.zen")

VCC = Power("VCC_3V3")
LED_A = Net("LED_A")

Resistor(name="R_LIMIT", value="1kohm", package="0402", P1=VCC, P2=LED_A)
"#;
        let root = workspace(&[("board.zen", board)]);

        let mut instances = BTreeMap::new();
        instances.insert(
            "root".into(),
            instance("root", InstanceKind::Module, Some("board.zen")),
        );
        instances.insert(
            "root.R_LIMIT".into(),
            instance(
                "root.R_LIMIT",
                InstanceKind::Module,
                Some(".pcb/stdlib/generics/Resistor.zen"),
            ),
        );
        instances.insert(
            "root.R_LIMIT.R".into(),
            instance(
                "root.R_LIMIT.R",
                InstanceKind::Component,
                Some(".pcb/stdlib/generics/Resistor.zen"),
            ),
        );
        let mut nets = BTreeMap::new();
        nets.insert("VCC_3V3".into(), net("VCC_3V3", "Power"));
        nets.insert("LED_A".into(), net("LED_A", "Net"));
        nets.insert("V_MISSING".into(), net("V_MISSING", "Net"));
        let sch = SchematicDoc {
            root_module: "board".into(),
            instances,
            nets,
            by_refdes: BTreeMap::new(),
        };

        let doc = analyze_editability(&sch, &root);

        let r_limit = &doc.instances["root.R_LIMIT"];
        assert!(r_limit.editable, "{:?}", r_limit.reason);
        assert_eq!(r_limit.file.as_deref(), Some("board.zen"));
        assert_eq!(r_limit.line, Some(8));

        // The hoisted component inside the stdlib generic refuses toward
        // its authored ancestor.
        let inner = &doc.instances["root.R_LIMIT.R"];
        assert!(!inner.editable);
        assert!(inner.reason.as_deref().unwrap().contains("library source"));
        assert_eq!(inner.anchor.as_deref(), Some("root.R_LIMIT"));

        let vcc = &doc.nets["VCC_3V3"];
        assert!(vcc.editable, "{:?}", vcc.reason);
        assert_eq!(vcc.variable.as_deref(), Some("VCC"));
        assert_eq!(vcc.line, Some(5));
        assert!(doc.nets["LED_A"].editable);

        // A net with no literal definition refuses honestly.
        let missing = &doc.nets["V_MISSING"];
        assert!(!missing.editable);
        assert!(missing.reason.as_deref().unwrap().contains("io()"));

        // The root itself is not an edit target.
        assert!(!doc.instances.contains_key("root"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Loop-generated instances refuse with a "generated" reason; a
    /// duplicate literal name refuses as ambiguous.
    #[test]
    fn generated_and_ambiguous_instances_refuse() {
        let board = r#"Resistor = Module("@stdlib/generics/Resistor.zen")

for i in range(3):
    Resistor(name="R{}".format(i), value="1kohm", package="0402")

Resistor(name="R_DUP", value="1kohm", package="0402")
Resistor(name="R_DUP", value="2kohm", package="0402")
"#;
        let root = workspace(&[("board.zen", board)]);
        let mut instances = BTreeMap::new();
        instances.insert(
            "root".into(),
            instance("root", InstanceKind::Module, Some("board.zen")),
        );
        instances.insert(
            "root.R0".into(),
            instance(
                "root.R0",
                InstanceKind::Module,
                Some(".pcb/stdlib/generics/Resistor.zen"),
            ),
        );
        instances.insert(
            "root.R_DUP".into(),
            instance(
                "root.R_DUP",
                InstanceKind::Module,
                Some(".pcb/stdlib/generics/Resistor.zen"),
            ),
        );
        let sch = SchematicDoc {
            root_module: "board".into(),
            instances,
            nets: BTreeMap::new(),
            by_refdes: BTreeMap::new(),
        };

        let doc = analyze_editability(&sch, &root);
        let generated = &doc.instances["root.R0"];
        assert!(!generated.editable);
        assert!(generated.reason.as_deref().unwrap().contains("generated"));
        assert_eq!(generated.anchor, None);

        let dup = &doc.instances["root.R_DUP"];
        assert!(!dup.editable);
        assert!(dup.reason.as_deref().unwrap().contains("ambiguous"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A file that fails to parse refuses everything in it with the parse
    /// error — the red-board fix path stays honest instead of guessing.
    #[test]
    fn unparseable_file_refuses_with_the_error() {
        let root = workspace(&[("board.zen", "Resistor(name=\"R1\"\n")]);
        let mut instances = BTreeMap::new();
        instances.insert(
            "root".into(),
            instance("root", InstanceKind::Module, Some("board.zen")),
        );
        instances.insert(
            "root.R1".into(),
            instance(
                "root.R1",
                InstanceKind::Module,
                Some(".pcb/stdlib/generics/Resistor.zen"),
            ),
        );
        let sch = SchematicDoc {
            root_module: "board".into(),
            instances,
            nets: BTreeMap::new(),
            by_refdes: BTreeMap::new(),
        };
        let doc = analyze_editability(&sch, &root);
        let e = &doc.instances["root.R1"];
        assert!(!e.editable);
        assert!(e.reason.as_deref().unwrap().contains("does not parse"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Assigned instantiations (`x = Resistor(name=…)`) still classify, and
    /// submodule-owned instances resolve against the submodule's file.
    #[test]
    fn assigned_calls_and_submodule_files_classify() {
        let board = "VoltageDivider = Module(\"./components/vd.zen\")\nVoltageDivider(name=\"DIV\")\n";
        let vd = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\nr1 = Resistor(name=\"R1\", value=\"10kohms\", package=\"0603\")\n";
        let root = workspace(&[("board.zen", board), ("components/vd.zen", vd)]);
        let mut instances = BTreeMap::new();
        instances.insert(
            "root".into(),
            instance("root", InstanceKind::Module, Some("board.zen")),
        );
        instances.insert(
            "root.DIV".into(),
            instance("root.DIV", InstanceKind::Module, Some("components/vd.zen")),
        );
        instances.insert(
            "root.DIV.R1".into(),
            instance(
                "root.DIV.R1",
                InstanceKind::Module,
                Some(".pcb/stdlib/generics/Resistor.zen"),
            ),
        );
        let sch = SchematicDoc {
            root_module: "board".into(),
            instances,
            nets: BTreeMap::new(),
            by_refdes: BTreeMap::new(),
        };
        let doc = analyze_editability(&sch, &root);
        assert!(doc.instances["root.DIV"].editable);
        let r1 = &doc.instances["root.DIV.R1"];
        assert!(r1.editable, "{:?}", r1.reason);
        assert_eq!(r1.file.as_deref(), Some("components/vd.zen"));
        assert_eq!(r1.line, Some(2));
        let _ = std::fs::remove_dir_all(&root);
    }

    // -----------------------------------------------------------------------
    // Writer tests — diffs are asserted EXACTLY (docs/product.md: would this
    // diff make sense to a reviewer?).
    // -----------------------------------------------------------------------

    const GENERIC_RESISTOR: &str = r#"P1 = io(Net)
P2 = io(Net)

Component(
    name="R",
    prefix="R",
    pins={"1": P1, "2": P2},
)
"#;

    fn writer_workspace(board: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = workspace(&[
            ("board.zen", board),
            (".pcb/stdlib/generics/Resistor.zen", GENERIC_RESISTOR),
            (".pcb/stdlib/pcb.toml", ""),
        ]);
        let stdlib = root.join(".pcb/stdlib");
        (root, stdlib)
    }

    #[test]
    fn add_instance_with_existing_binding_is_one_clean_insertion() {
        let board = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
                     Resistor(name=\"R1\", value=\"1kohm\", package=\"0402\")\n\n\
                     Board(name=\"demo\", layers=2, layout_path=\"layout/demo\")\n";
        let (root, stdlib) = writer_workspace(board);
        let file = root.join("board.zen");

        let res = add_instance(
            &file,
            &root,
            &stdlib,
            None,
            &AddInstanceRequest {
                module: "@stdlib/generics/Resistor.zen".into(),
                name: "R2".into(),
                attrs: vec![
                    ("value".into(), "10kohm".into()),
                    ("package".into(), "0603".into()),
                ],
                position: Some(PlacedPosition {
                    x: 2.0,
                    y: -1.0,
                    rotation: 0.0,
                }),
            },
        )
        .unwrap();

        assert_eq!(res.binding, "Resistor");
        assert_eq!(res.position_key.as_deref(), Some("R2.R"));
        assert_eq!(res.placeholder_nets, vec!["R2_P1", "R2_P2"]);
        let after = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            after,
            "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             Resistor(name=\"R1\", value=\"1kohm\", package=\"0402\")\n\n\
             Resistor(name=\"R2\", value=\"10kohm\", package=\"0603\", P1=Net(\"R2_P1\"), P2=Net(\"R2_P2\"))\n\n\
             Board(name=\"demo\", layers=2, layout_path=\"layout/demo\")\n\n\
             # pcb:sch R2.R x=50.8000 y=25.4000 rot=0\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn add_instance_creates_the_module_binding_when_missing() {
        let board = "\"\"\"A blank board.\"\"\"\n\n\
                     Board(name=\"blank\", layers=2, layout_path=\"layout/blank\")\n";
        let (root, stdlib) = writer_workspace(board);
        let file = root.join("board.zen");

        let res = add_instance(
            &file,
            &root,
            &stdlib,
            None,
            &AddInstanceRequest {
                module: "@stdlib/generics/Resistor.zen".into(),
                name: "R1".into(),
                attrs: vec![("value".into(), "10kohm".into())],
                position: None,
            },
        )
        .unwrap();

        assert_eq!(res.binding, "Resistor");
        assert_eq!(res.position_key, None);
        let after = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            after,
            "\"\"\"A blank board.\"\"\"\n\n\
             Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             Resistor(name=\"R1\", value=\"10kohm\", P1=Net(\"R1_P1\"), P2=Net(\"R1_P2\"))\n\n\
             Board(name=\"blank\", layers=2, layout_path=\"layout/blank\")\n"
        );
        // Idempotence of the refusal: the same name now collides.
        let err = add_instance(
            &file,
            &root,
            &stdlib,
            None,
            &AddInstanceRequest {
                module: "@stdlib/generics/Resistor.zen".into(),
                name: "R1".into(),
                attrs: vec![],
                position: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn add_instance_refuses_bad_names_and_parse_errors() {
        let (root, stdlib) = writer_workspace("Resistor(name=\"R1\"\n");
        let file = root.join("board.zen");
        let err = add_instance(
            &file,
            &root,
            &stdlib,
            None,
            &AddInstanceRequest {
                module: "@stdlib/generics/Resistor.zen".into(),
                name: "R2".into(),
                attrs: vec![],
                position: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not parse"), "{err}");

        let err = add_instance(
            &file,
            &root,
            &stdlib,
            None,
            &AddInstanceRequest {
                module: "@stdlib/generics/Resistor.zen".into(),
                name: "1bad".into(),
                attrs: vec![],
                position: None,
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid instance name"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn module_facts_reads_child_and_prefix() {
        let (root, stdlib) = writer_workspace("# empty\n");
        let facts = module_facts("@stdlib/generics/Resistor.zen", &root, &stdlib);
        assert_eq!(facts.component_child.as_deref(), Some("R"));
        assert_eq!(facts.prefix.as_deref(), Some("R"));
        assert_eq!(facts.required_ios, vec!["P1", "P2"]);
        let none = module_facts("@stdlib/generics/Missing.zen", &root, &stdlib);
        assert_eq!(none.component_child, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn create_net_inserts_after_the_last_definition() {
        let board = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
                     GND = Ground(\"GND\")\n\n\
                     Resistor(name=\"R1\", value=\"1kohm\", package=\"0402\")\n";
        let (root, _) = writer_workspace(board);
        let file = root.join("board.zen");
        let res = create_net(&file, &root, "SENSE", "Net").unwrap();
        assert_eq!(res.variable, "SENSE");
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             GND = Ground(\"GND\")\n\n\
             SENSE = Net(\"SENSE\")\n\n\
             Resistor(name=\"R1\", value=\"1kohm\", package=\"0402\")\n"
        );
        // Duplicates and bound identifiers refuse.
        assert!(create_net(&file, &root, "SENSE", "Net")
            .unwrap_err()
            .to_string()
            .contains("already defined"));
        assert!(create_net(&file, &root, "Resistor", "Net")
            .unwrap_err()
            .to_string()
            .contains("already bound"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The flagship phase-2 diff: rename VCC_3V3 -> VCC_5V rewrites the
    /// variable, the string, and every reference — including one inside a
    /// for loop (references rename fine; only SHADOWING refuses).
    #[test]
    fn rename_net_rewrites_variable_string_and_references() {
        let board = "VCC = Power(\"VCC_3V3\")\nLED_A = Net(\"LED_A\")\n\n\
                     Resistor(name=\"R1\", P1=VCC, P2=LED_A)\n\n\
                     for i in range(2):\n    Led(name=\"D{}\".format(i), A=LED_A, K=VCC)\n\n\
                     # pcb:sch VCC_3V3.1 x=10.0000 y=5.0000 rot=0\n";
        let (root, _) = writer_workspace(board);
        let file = root.join("board.zen");

        let res = rename_net(&file, &root, "VCC_3V3", "VCC_5V").unwrap();
        assert_eq!(res.variable, "VCC_5V");
        assert_eq!(res.references, 2);
        assert_eq!(res.migrated_positions, vec!["VCC_5V.1".to_string()]);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "VCC_5V = Power(\"VCC_5V\")\nLED_A = Net(\"LED_A\")\n\n\
             Resistor(name=\"R1\", P1=VCC_5V, P2=LED_A)\n\n\
             for i in range(2):\n    Led(name=\"D{}\".format(i), A=LED_A, K=VCC_5V)\n\n\
             # pcb:sch VCC_5V.1 x=10.0000 y=5.0000 rot=0\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_net_refuses_shadowed_and_missing() {
        let board = "SIG = Net(\"SIG\")\n\ndef helper(SIG):\n    return SIG\n";
        let (root, _) = writer_workspace(board);
        let file = root.join("board.zen");
        let err = rename_net(&file, &root, "SIG", "SIG2").unwrap_err();
        assert!(err.to_string().contains("nested scope"), "{err}");
        let err = rename_net(&file, &root, "NOPE", "X").unwrap_err();
        assert!(err.to_string().contains("no top-level net definition"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Attach maps the component pin through the wrapper's pins dict,
    /// replaces the placeholder, and prunes the orphaned definition when a
    /// re-attachment leaves a net unreferenced.
    #[test]
    fn attach_pin_net_replaces_creates_and_prunes() {
        let board = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
                     GND = Ground(\"GND\")\n\n\
                     Resistor(name=\"R1\", value=\"1kohm\", P1=Net(\"R1_P1\"), P2=Net(\"R1_P2\"))\n";
        let (root, stdlib) = writer_workspace(board);
        let file = root.join("board.zen");

        // Component pin "1" -> io P1 via the pins dict; existing GND reused.
        let res = attach_pin_net(
            &file,
            &root,
            &stdlib,
            &AttachPinRequest {
                instance: "R1".into(),
                pin: "1".into(),
                net_name: "GND".into(),
                kind: "Ground".into(),
            },
        )
        .unwrap();
        assert_eq!(res.io, "P1");
        assert_eq!(res.variable, "GND");
        assert!(!res.created_def);
        assert!(res.pruned_defs.is_empty()); // placeholder was inline, no def

        // New net on P2: definition created before the call.
        let res = attach_pin_net(
            &file,
            &root,
            &stdlib,
            &AttachPinRequest {
                instance: "R1".into(),
                pin: "P2".into(),
                net_name: "SENSE".into(),
                kind: "Net".into(),
            },
        )
        .unwrap();
        assert!(res.created_def);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             GND = Ground(\"GND\")\n\n\
             SENSE = Net(\"SENSE\")\n\n\
             Resistor(name=\"R1\", value=\"1kohm\", P1=GND, P2=SENSE)\n"
        );

        // Re-attach P2 elsewhere: SENSE is orphaned and prunes in the same
        // write (auto-prune, PRD §12.2).
        let res = attach_pin_net(
            &file,
            &root,
            &stdlib,
            &AttachPinRequest {
                instance: "R1".into(),
                pin: "P2".into(),
                net_name: "LED_A".into(),
                kind: "Net".into(),
            },
        )
        .unwrap();
        assert_eq!(res.pruned_defs, vec!["SENSE".to_string()]);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             GND = Ground(\"GND\")\n\n\
             LED_A = Net(\"LED_A\")\n\n\
             Resistor(name=\"R1\", value=\"1kohm\", P1=GND, P2=LED_A)\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The PRD's endorsed example, as a literal test: draw wire R1.P2 →
    /// C1.P1 on two unwired pins creates the shared net and points both
    /// kwargs at it.
    #[test]
    fn connect_pins_endorsed_example() {
        let board = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
                     Resistor(name=\"R1\", value=\"1kohm\", P1=Net(\"R1_P1\"), P2=Net(\"R1_P2\"))\n\
                     Resistor(name=\"C1\", value=\"100nF\", P1=Net(\"C1_P1\"), P2=Net(\"C1_P2\"))\n";
        let (root, stdlib) = writer_workspace(board);
        let file = root.join("board.zen");
        let out = connect_pins(
            &file,
            &root,
            &stdlib,
            None,
            &ConnectPinsRequest {
                a: PinEndpoint { instance: "R1".into(), pin: "P2".into() },
                b: PinEndpoint { instance: "C1".into(), pin: "P1".into() },
                net: Some("SIG".into()),
                allow_merge: false,
            },
        )
        .unwrap();
        let ConnectOutcome::Applied { net, created_def, .. } = &out else {
            panic!("expected Applied, got {out:?}");
        };
        assert_eq!(net, "SIG");
        assert!(created_def);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             SIG = Net(\"SIG\")\n\n\
             Resistor(name=\"R1\", value=\"1kohm\", P1=Net(\"R1_P1\"), P2=SIG)\n\
             Resistor(name=\"C1\", value=\"100nF\", P1=SIG, P2=Net(\"C1_P2\"))\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// One end on a shared net: the other pin joins it; joining two shared
    /// nets reports NeedsMerge until allowed, then rewrites and prunes.
    #[test]
    fn connect_pins_join_and_merge() {
        let board = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
                     BUS_A = Net(\"BUS_A\")\nBUS_B = Net(\"BUS_B\")\n\n\
                     Resistor(name=\"R1\", value=\"1k\", P1=BUS_A, P2=BUS_A)\n\
                     Resistor(name=\"R2\", value=\"1k\", P1=BUS_B, P2=BUS_B)\n\
                     Resistor(name=\"R3\", value=\"1k\", P1=Net(\"R3_P1\"), P2=Net(\"R3_P2\"))\n";
        let (root, stdlib) = writer_workspace(board);
        let file = root.join("board.zen");

        // Placeholder joins the shared net — no merge involved.
        let out = connect_pins(
            &file,
            &root,
            &stdlib,
            None,
            &ConnectPinsRequest {
                a: PinEndpoint { instance: "R3".into(), pin: "P1".into() },
                b: PinEndpoint { instance: "R1".into(), pin: "P1".into() },
                net: None,
                allow_merge: false,
            },
        )
        .unwrap();
        assert!(
            matches!(&out, ConnectOutcome::Applied { net, created_def: false, .. } if net == "BUS_A"),
            "{out:?}"
        );

        // Two shared nets: refused into a confirmation, then merged.
        let req = ConnectPinsRequest {
            a: PinEndpoint { instance: "R1".into(), pin: "P2".into() },
            b: PinEndpoint { instance: "R2".into(), pin: "P1".into() },
            net: None,
            allow_merge: false,
        };
        let out = connect_pins(&file, &root, &stdlib, None, &req).unwrap();
        let ConnectOutcome::NeedsMerge { from, into, from_refs } = &out else {
            panic!("expected NeedsMerge, got {out:?}");
        };
        assert_eq!((from.as_str(), into.as_str(), *from_refs), ("BUS_B", "BUS_A", 2));

        let out = connect_pins(
            &file,
            &root,
            &stdlib,
            None,
            &ConnectPinsRequest { allow_merge: true, ..req },
        )
        .unwrap();
        let ConnectOutcome::Applied { merged_from, moved_refs, pruned_defs, .. } = &out else {
            panic!("expected Applied, got {out:?}");
        };
        assert_eq!(merged_from.as_deref(), Some("BUS_B"));
        assert_eq!(*moved_refs, 2);
        assert_eq!(pruned_defs, &vec!["BUS_B".to_string()]);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             BUS_A = Net(\"BUS_A\")\n\n\
             Resistor(name=\"R1\", value=\"1k\", P1=BUS_A, P2=BUS_A)\n\
             Resistor(name=\"R2\", value=\"1k\", P1=BUS_A, P2=BUS_A)\n\
             Resistor(name=\"R3\", value=\"1k\", P1=BUS_A, P2=Net(\"R3_P2\"))\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Disconnect: required pins revert to a placeholder; the def prunes
    /// when orphaned.
    #[test]
    fn disconnect_pin_reverts_and_prunes() {
        let board = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
                     SENSE = Net(\"SENSE\")\n\n\
                     Resistor(name=\"R1\", value=\"1k\", P1=SENSE, P2=Net(\"R1_P2\"))\n";
        let (root, stdlib) = writer_workspace(board);
        let file = root.join("board.zen");
        let res = disconnect_pin(&file, &root, &stdlib, "R1", "1").unwrap();
        assert_eq!(res.io, "P1");
        assert_eq!(res.placeholder.as_deref(), Some("R1_P1"));
        assert_eq!(res.pruned_defs, vec!["SENSE".to_string()]);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             Resistor(name=\"R1\", value=\"1k\", P1=Net(\"R1_P1\"), P2=Net(\"R1_P2\"))\n"
        );
        // Already-unconnected pins refuse.
        let err = disconnect_pin(&file, &root, &stdlib, "R1", "1").unwrap_err();
        assert!(err.to_string().contains("already unconnected"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The PRD's refusal criterion: every writer aimed at generated
    /// targets (loops, computed names, missing definitions) must ERR and
    /// leave the file byte-identical — refused, never misedited.
    #[test]
    fn refusals_never_misedit() {
        let board = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
                     BUS = Net(\"BUS\")\n\n\
                     for i in range(3):\n    Resistor(name=\"R{}\".format(i), value=\"1k\", P1=BUS, P2=BUS)\n";
        let (root, stdlib) = writer_workspace(board);
        let file = root.join("board.zen");
        let before = std::fs::read_to_string(&file).unwrap();

        // A schematic in which R1 exists (generated by the loop).
        let mut instances = BTreeMap::new();
        let mut root_inst = instance("root", InstanceKind::Module, Some("board.zen"));
        root_inst
            .children
            .insert("R1".into(), "root.R1".into());
        instances.insert("root".into(), root_inst);
        instances.insert(
            "root.R1".into(),
            instance(
                "root.R1",
                InstanceKind::Module,
                Some(".pcb/stdlib/generics/Resistor.zen"),
            ),
        );
        let sch = SchematicDoc {
            root_module: "board".into(),
            instances,
            nets: BTreeMap::new(),
            by_refdes: BTreeMap::new(),
        };

        let attempts: Vec<(&str, Box<dyn Fn() -> String>)> = vec![
            ("add colliding generated name", Box::new(|| {
                add_instance(&file, &root, &stdlib, Some(&sch), &AddInstanceRequest {
                    module: "@stdlib/generics/Resistor.zen".into(),
                    name: "R1".into(),
                    attrs: vec![],
                    position: None,
                }).unwrap_err().to_string()
            })),
            ("rename generated instance", Box::new(|| {
                rename_instance(&file, &root, "R1", "R_X").unwrap_err().to_string()
            })),
            ("attach pin of generated instance", Box::new(|| {
                attach_pin_net(&file, &root, &stdlib, &AttachPinRequest {
                    instance: "R1".into(), pin: "1".into(),
                    net_name: "GND".into(), kind: "Ground".into(),
                }).unwrap_err().to_string()
            })),
            ("wire generated instance", Box::new(|| {
                match connect_pins(&file, &root, &stdlib, None, &ConnectPinsRequest {
                    a: PinEndpoint { instance: "R1".into(), pin: "1".into() },
                    b: PinEndpoint { instance: "R2".into(), pin: "1".into() },
                    net: None, allow_merge: false,
                }) {
                    Err(e) => e.to_string(),
                    Ok(o) => panic!("connect must refuse, got {o:?}"),
                }
            })),
            ("disconnect generated pin", Box::new(|| {
                disconnect_pin(&file, &root, &stdlib, "R1", "1").unwrap_err().to_string()
            })),
            ("set attribute on generated instance", Box::new(|| {
                set_attribute(&file, &root, &stdlib, "R1", "value", "2k").unwrap_err().to_string()
            })),
            ("remove generated instance", Box::new(|| {
                remove_instances(&file, &root, &["R1".to_string()]).unwrap_err().to_string()
            })),
            ("rename missing net", Box::new(|| {
                rename_net(&file, &root, "GHOST", "X").unwrap_err().to_string()
            })),
            ("duplicate net", Box::new(|| {
                create_net(&file, &root, "BUS", "Net").unwrap_err().to_string()
            })),
        ];
        for (what, run) in attempts {
            let err = run();
            assert!(!err.is_empty(), "{what}: expected a reason");
            assert_eq!(
                std::fs::read_to_string(&file).unwrap(),
                before,
                "{what}: refusal must leave the file byte-identical"
            );
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn set_attribute_replaces_adds_and_refuses_pins() {
        let board = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
                     Resistor(name=\"R1\", value=\"1kohm\", P1=Net(\"R1_P1\"), P2=Net(\"R1_P2\"))\n";
        let (root, stdlib) = writer_workspace(board);
        let file = root.join("board.zen");

        let res = set_attribute(&file, &root, &stdlib, "R1", "value", "10kohm").unwrap();
        assert!(res.replaced);
        let res = set_attribute(&file, &root, &stdlib, "R1", "package", "0402").unwrap();
        assert!(!res.replaced);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             Resistor(name=\"R1\", value=\"10kohm\", P1=Net(\"R1_P1\"), P2=Net(\"R1_P2\"), package=\"0402\")\n"
        );
        let err = set_attribute(&file, &root, &stdlib, "R1", "P1", "GND").unwrap_err();
        assert!(err.to_string().contains("connect_pins"), "{err}");
        let err = set_attribute(&file, &root, &stdlib, "R1", "name", "R2").unwrap_err();
        assert!(err.to_string().contains("rename_instance"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Removal is a batch: statements go, orphaned nets and bindings prune,
    /// position keys drop — one write, exact diff.
    #[test]
    fn remove_instances_prunes_orphans_and_positions() {
        let board = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
                     SHARED = Net(\"SHARED\")\nONLY_R2 = Net(\"ONLY_R2\")\n\n\
                     Resistor(name=\"R1\", value=\"1k\", P1=SHARED, P2=Net(\"R1_P2\"))\n\
                     Resistor(name=\"R2\", value=\"2k\", P1=SHARED, P2=ONLY_R2)\n\n\
                     # pcb:sch R1.R x=10.0000 y=5.0000 rot=0\n\
                     # pcb:sch R2.R x=20.0000 y=5.0000 rot=0\n";
        let (root, _) = writer_workspace(board);
        let file = root.join("board.zen");

        let res = remove_instances(&file, &root, &["R2".to_string()]).unwrap();
        assert_eq!(res.removed, vec!["R2"]);
        assert_eq!(res.pruned_nets, vec!["ONLY_R2"]);
        assert!(res.pruned_bindings.is_empty(), "R1 still uses the binding");
        assert_eq!(res.removed_positions, vec!["R2.R"]);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             SHARED = Net(\"SHARED\")\n\n\
             Resistor(name=\"R1\", value=\"1k\", P1=SHARED, P2=Net(\"R1_P2\"))\n\n\
             # pcb:sch R1.R x=10.0000 y=5.0000 rot=0\n"
        );

        // Removing the last user prunes the net AND the module binding.
        let res = remove_instances(&file, &root, &["R1".to_string()]).unwrap();
        assert_eq!(res.pruned_nets, vec!["SHARED"]);
        assert_eq!(res.pruned_bindings, vec!["Resistor"]);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_instance_replaces_the_literal_and_migrates_positions() {
        let board = "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
                     Resistor(name=\"R1\", value=\"1kohm\", package=\"0402\")\n\n\
                     # pcb:sch R1.R x=50.8000 y=25.4000 rot=90\n\
                     # pcb:sch OTHER.C x=10.0000 y=10.0000 rot=0\n";
        let (root, _) = writer_workspace(board);
        let file = root.join("board.zen");

        let res = rename_instance(&file, &root, "R1", "R_SENSE").unwrap();
        assert_eq!(res.migrated_positions, vec!["R_SENSE.R".to_string()]);
        let after = std::fs::read_to_string(&file).unwrap();
        assert_eq!(
            after,
            "Resistor = Module(\"@stdlib/generics/Resistor.zen\")\n\n\
             Resistor(name=\"R_SENSE\", value=\"1kohm\", package=\"0402\")\n\n\
             # pcb:sch OTHER.C x=10.0000 y=10.0000 rot=0\n\
             # pcb:sch R_SENSE.R x=50.8000 y=25.4000 rot=90\n"
        );

        // Unknown and ambiguous targets refuse.
        let err = rename_instance(&file, &root, "R1", "R2").unwrap_err();
        assert!(err.to_string().contains("no top-level call"), "{err}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
