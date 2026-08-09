#!/usr/bin/env node
// Regenerates src/generated/ from the ts-rs derives in crates/store.
// `--check` regenerates into a temp dir and diffs against the committed
// files (CI runs this; a mismatch means someone changed a store DTO
// without re-running `pnpm gen:store-types`).

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import os from "node:os";
import url from "node:url";

const here = path.dirname(url.fileURLToPath(import.meta.url));
const appDir = path.join(here, "..");
const repoRoot = path.join(appDir, "../..");
const outDir = path.join(appDir, "src/generated");
const check = process.argv.includes("--check");

function generate(dir) {
  fs.rmSync(dir, { recursive: true, force: true });
  fs.mkdirSync(dir, { recursive: true });
  execFileSync("cargo", ["test", "-p", "store", "export_bindings", "--quiet"], {
    cwd: repoRoot,
    env: { ...process.env, TS_RS_EXPORT_DIR: dir },
    stdio: ["ignore", "inherit", "inherit"],
  });
}

function snapshot(dir) {
  if (!fs.existsSync(dir)) return new Map();
  return new Map(
    fs
      .readdirSync(dir)
      .filter((f) => f.endsWith(".ts"))
      .sort()
      .map((f) => [f, fs.readFileSync(path.join(dir, f), "utf8")]),
  );
}

if (check) {
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "store-types-"));
  try {
    generate(tmp);
    const want = snapshot(tmp);
    const have = snapshot(outDir);
    const keys = new Set([...want.keys(), ...have.keys()]);
    const stale = [...keys].filter((k) => want.get(k) !== have.get(k));
    if (stale.length > 0) {
      console.error(
        `src/generated/ is stale (${stale.join(", ")}) — run: pnpm gen:store-types`,
      );
      process.exit(1);
    }
    console.log("src/generated/ is up to date");
  } finally {
    fs.rmSync(tmp, { recursive: true, force: true });
  }
} else {
  generate(outDir);
  console.log(
    `wrote ${fs.readdirSync(outDir).length} files to src/generated/`,
  );
}
