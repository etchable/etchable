//! M0 pipeline spike: eval a .zen file and dump schematic + diagnostics JSON.
//!
//!     zen-build path/to/board.zen [--pretty] [--summary] [--offline]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use zen_build::{BuildSummary, Severity, Workspace};

#[derive(Parser)]
#[command(name = "zen-build", about = "Eval a .zen file and print schematic JSON")]
struct Args {
    /// .zen file to build
    path: PathBuf,

    /// Pretty-print the JSON output
    #[arg(long)]
    pretty: bool,

    /// Print only a compact build summary instead of the full schematic
    #[arg(long)]
    summary: bool,

    /// Print the Circuit JSON view-model ({"elements": [...], "id_map": {...}})
    /// instead of the schematic document
    #[arg(long)]
    circuit_json: bool,

    /// Disable network access (use only cached/vendored dependencies)
    #[arg(long)]
    offline: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let workspace = match Workspace::open(&args.path, args.offline) {
        Ok(ws) => ws,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::FAILURE;
        }
    };
    eprintln!("workspace: {}", workspace.root().display());

    let output = match workspace.build_file(&args.path, &BTreeMap::new()) {
        Ok(out) => out,
        Err(e) => {
            eprintln!("error: {e:#}");
            return ExitCode::FAILURE;
        }
    };

    for diag in &output.diagnostics {
        if diag.suppressed {
            continue;
        }
        let sev = match diag.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Advice => "advice",
        };
        let loc = match (&diag.file, diag.line) {
            (Some(f), Some(l)) => format!("{f}:{l}"),
            (Some(f), None) => f.clone(),
            _ => String::from("<unknown>"),
        };
        eprintln!("{sev}: {loc}: {}", diag.message);
    }

    let json = if args.summary {
        let summary = BuildSummary::from_output(&output);
        if args.pretty {
            serde_json::to_string_pretty(&summary)
        } else {
            serde_json::to_string(&summary)
        }
    } else if args.circuit_json {
        let doc = zen_build::to_circuit_json(&output);
        if args.pretty {
            serde_json::to_string_pretty(&doc)
        } else {
            serde_json::to_string(&doc)
        }
    } else if args.pretty {
        serde_json::to_string_pretty(&output)
    } else {
        serde_json::to_string(&output)
    };

    match json {
        Ok(s) => println!("{s}"),
        Err(e) => {
            eprintln!("error: failed to serialize output: {e}");
            return ExitCode::FAILURE;
        }
    }

    if output.has_errors() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
