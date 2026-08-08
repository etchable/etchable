#!/usr/bin/env node
// Validates a CircuitJsonDoc ({elements, id_map}) — as emitted by
// `cargo run -p zen-build -- board.zen --circuit-json` — against the
// published circuit-json zod schemas plus etchable's own invariants:
// every referenced id resolves through id_map, and every symbol_name is a
// real schematic-symbols key. Reads a file path argv[1] or stdin.
//
//   cargo run -q -p zen-build -- examples/demo/top.zen --circuit-json \
//     | node ui/tools/validate-circuit-json.mjs

import { readFileSync } from "node:fs";
import { any_circuit_element } from "circuit-json";
import { symbols } from "schematic-symbols";

const input = process.argv[2]
  ? readFileSync(process.argv[2], "utf8")
  : readFileSync(0, "utf8");
// The zen-build CLI prints diagnostics to stderr but `workspace:` may land
// before the JSON when piped through shells that merge streams; be tolerant.
const doc = JSON.parse(input.slice(input.indexOf("{")));

if (!Array.isArray(doc.elements) || typeof doc.id_map !== "object") {
  console.error("input is not a CircuitJsonDoc ({elements, id_map})");
  process.exit(1);
}

const errors = [];

// 1. Every element parses against the published schema.
for (const [i, el] of doc.elements.entries()) {
  const parsed = any_circuit_element.safeParse(el);
  if (!parsed.success) {
    const issues = parsed.error.issues
      .slice(0, 3)
      .map((iss) => `${iss.path.join(".")}: ${iss.message}`)
      .join("; ");
    errors.push(`element[${i}] (${el.type}): ${issues}`);
  }
}

// 2. Every id referenced anywhere resolves through id_map.
for (const el of doc.elements) {
  for (const [key, value] of Object.entries(el)) {
    if (!key.endsWith("_id")) continue;
    const ids = Array.isArray(value) ? value : [value];
    for (const id of ids) {
      if (typeof id === "string" && !(id in doc.id_map)) {
        errors.push(`unmapped id ${id} referenced by ${el.type}.${key}`);
      }
    }
  }
}

// 3. Every symbol_name is a real schematic-symbols key.
for (const el of doc.elements) {
  if (el.symbol_name && !(el.symbol_name in symbols)) {
    errors.push(`unknown symbol_name "${el.symbol_name}" on ${el.type}`);
  }
}

if (errors.length > 0) {
  console.error(`circuit-json validation FAILED (${errors.length} problems):`);
  for (const e of errors.slice(0, 20)) console.error("  - " + e);
  if (errors.length > 20) console.error(`  ... and ${errors.length - 20} more`);
  process.exit(1);
}

const counts = {};
for (const el of doc.elements) counts[el.type] = (counts[el.type] ?? 0) + 1;
console.log(
  `circuit-json OK: ${doc.elements.length} elements, ${Object.keys(doc.id_map).length} mapped ids`,
);
console.log(
  "  " +
    Object.entries(counts)
      .map(([t, n]) => `${t}=${n}`)
      .join(" "),
);
