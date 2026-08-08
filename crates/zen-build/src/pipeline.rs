//! Re-hosted build pipeline from `pcbc/src/build.rs` (diodeinc/pcb, MIT).
//! resolve -> eval -> electrical checks -> schematic -> ERC.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use pcb_zen_core::resolution::ResolutionResult;
use pcb_zen_core::{DefaultFileProvider, Diagnostics, EvalContext, EvalContextConfig, FileProvider};
use serde_json::Value as JsonValue;
use starlark::collections::SmallMap;

pub(crate) struct EvalState {
    file_provider: Arc<DefaultFileProvider>,
    resolution: Arc<ResolutionResult>,
}

pub(crate) struct RawBuildResult {
    pub(crate) schematic: Option<pcb_sch::Schematic>,
    pub(crate) diagnostics: Diagnostics,
}

impl EvalState {
    pub(crate) fn new(mut resolution: ResolutionResult) -> Self {
        let file_provider = Arc::new(DefaultFileProvider::new());
        resolution.canonicalize_keys(file_provider.as_ref());
        Self {
            file_provider,
            resolution: Arc::new(resolution),
        }
    }

    fn eval(
        &self,
        zen_path: &Path,
        inputs: SmallMap<String, JsonValue>,
    ) -> pcb_zen_core::WithDiagnostics<pcb_zen_core::EvalOutput> {
        // Fresh session per build: the session caches parsed sources by path,
        // and the watch loop rebuilds precisely because files changed on disk.
        let session = pcb_zen_core::lang::eval::EvalSession::default();
        session.prepare_for_root_eval();
        let source_path = self
            .file_provider
            .canonicalize(zen_path)
            .expect("failed to canonicalise input path");

        let mut ctx = EvalContext::from_session_and_config(
            session,
            EvalContextConfig::new(self.file_provider.clone(), self.resolution.clone()),
        )
        .set_source_path(source_path);

        ctx.set_json_inputs(inputs);
        ctx.eval()
    }

    pub(crate) fn build(
        &self,
        zen_path: &Path,
        inputs: SmallMap<String, JsonValue>,
    ) -> RawBuildResult {
        let eval_result = self.eval(zen_path, inputs);
        let mut diagnostics = eval_result.diagnostics;

        let output = if let Some(eval_output) = eval_result.output {
            for (check, defining_module) in eval_output.collect_electrical_checks() {
                diagnostics
                    .diagnostics
                    .push(execute_electrical_check(&check, &defining_module));
            }
            Some(eval_output)
        } else {
            None
        };

        let schematic = output.as_ref().and_then(|eval_output| {
            let schematic_result = eval_output.to_schematic_with_diagnostics();
            diagnostics
                .diagnostics
                .extend(schematic_result.diagnostics.diagnostics);
            if let Some(ref schematic) = schematic_result.output {
                let erc_diagnostics = pcb_zen_core::run_schematic_erc(eval_output, schematic);
                for diag in erc_diagnostics.diagnostics {
                    diagnostics.push_unique(diag);
                }
            }
            schematic_result.output
        });

        diagnostics.apply_passes(&diagnostics_passes());

        RawBuildResult {
            schematic,
            diagnostics,
        }
    }
}

fn diagnostics_passes() -> Vec<Box<dyn pcb_zen_core::DiagnosticsPass>> {
    // Same set as `pcbc`, minus RenderPass: a library must not print to stderr.
    vec![
        Box::new(pcb_zen_core::FilterHiddenPass),
        Box::new(pcb_zen_core::SuppressPass::new(Vec::new())),
        Box::new(pcb_zen_core::CommentSuppressPass::new()),
        Box::new(pcb_zen_core::AggregatePass),
    ]
}

fn execute_electrical_check(
    check: &pcb_zen_core::lang::electrical_check::FrozenElectricalCheck,
    defining_module: &pcb_zen_core::lang::module::FrozenModuleValue,
) -> pcb_zen_core::Diagnostic {
    use starlark::environment::Module;
    use starlark::eval::Evaluator;

    Module::with_temp_heap(|module| {
        let mut eval = Evaluator::new(&module);
        let module_value = module.heap().alloc_simple(defining_module.clone());

        pcb_zen_core::lang::electrical_check::execute_electrical_check(
            &mut eval,
            check,
            module_value,
        )
    })
}

/// Discover the workspace containing `path` and resolve its dependencies.
pub(crate) fn resolve(path: &Path, offline: bool) -> Result<ResolutionResult> {
    let file_provider = DefaultFileProvider::new();
    let workspace_info = pcb_zen::get_workspace_info(&file_provider, path)
        .with_context(|| format!("failed to discover workspace for {}", path.display()))?;

    if !workspace_info.errors.is_empty() {
        let mut msg = String::new();
        for err in &workspace_info.errors {
            msg.push_str(&format!("{}: {}\n", err.path.display(), err.error));
        }
        bail!("invalid pcb.toml file(s):\n{msg}");
    }

    pcb_zen::resolve_workspace_dependencies(workspace_info, path, offline)
}

pub(crate) fn workspace_root(resolution: &ResolutionResult) -> PathBuf {
    resolution.workspace_info.root.clone()
}

impl EvalState {
    /// The materialized stdlib dir (`<root>/.pcb/stdlib`).
    pub(crate) fn stdlib_dir(&self) -> PathBuf {
        self.resolution.workspace_info.workspace_stdlib_dir()
    }
}
