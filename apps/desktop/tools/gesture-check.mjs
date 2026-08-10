#!/usr/bin/env node
// Gesture regression check for the canvas.
//
// WHY THIS EXISTS AS A SCRIPT: the bugs that keep recurring live in the
// interaction between our overlay and the vendored @tscircuit/schematic-viewer
// — a dragged symbol drifting away from the cursor, a group that only moves
// after the drop, pin targets swallowing clicks meant for wires or symbol
// bodies, a drag that clears the selection when you let go. None of that is
// reachable from a unit test (see src/circuit/moves.test.ts for the arithmetic
// those gestures share); it only shows up when real events hit the real
// renderer. So this drives the repro harness in a browser and asserts on what
// the DOM and the recorded callbacks actually did.
//
// Requires: a repro server and the agent-browser CLI (a developer tool, not a
// repo dependency) — which is why this is NOT wired into CI.
//
//   pnpm --filter @etchable/desktop exec vite --port 5199 --host 127.0.0.1 &
//   node apps/desktop/tools/gesture-check.mjs
//
// Exits non-zero on the first failed assertion.

import { execFile } from "node:child_process";
import { promisify } from "node:util";

const run = promisify(execFile);
const URL_BASE = process.env.REPRO_URL ?? "http://127.0.0.1:5199/repro.html";
const CANVAS = `${URL_BASE}?state=canvas`;

let passed = 0;
const failures = [];

function check(name, condition, detail) {
  if (condition) {
    passed += 1;
    console.log(`  ok   ${name}`);
  } else {
    failures.push({ name, detail });
    console.log(`  FAIL ${name}${detail ? ` — ${detail}` : ""}`);
  }
}

async function ab(...args) {
  const { stdout } = await run("agent-browser", args, { maxBuffer: 8 * 1024 * 1024 });
  return stdout.trim();
}

/** Evaluate JS in the page and JSON.parse the result. */
async function evalJson(expr) {
  const out = await ab("eval", `(()=>{const __r=(${expr}); return JSON.stringify(__r);})()`);
  // agent-browser echoes the value as a JSON string literal.
  const firstQuote = out.indexOf('"');
  const raw = firstQuote === -1 ? out : out.slice(firstQuote);
  return JSON.parse(JSON.parse(raw));
}

const mouse = (...steps) => ab("batch", ...steps.map((s) => `mouse ${s}`));

async function open() {
  await ab("open", CANVAS);
  await ab("wait", "--load", "networkidle");
}

async function openState(state) {
  await ab("open", `${URL_BASE}?state=${state}`);
  await ab("wait", "--load", "networkidle");
}

/** Screen-space center of a component's rendered symbol. */
const centerOf = (path) =>
  evalJson(
    `(()=>{const el=document.querySelector('g[data-schematic-component-id="sch:${path}"]');` +
      `const r=el.getBoundingClientRect();return {x:Math.round(r.x+r.width/2),y:Math.round(r.y+r.height/2)};})()`,
  );

const selection = () => evalJson("window.__repro.selection()");
const saves = () => evalJson("window.__repro.record.saves");
const clearSaves = () => ab("eval", "(()=>{window.__repro.record.saves.length=0; return 1;})()");

// A marquee across the divider, which selects its two resistors.
async function marqueeDivider() {
  await mouse("move 500 420", "down left", "move 720 560", "move 720 560", "up left");
}

async function main() {
  console.log("drag tracks the cursor");
  await open();
  {
    const before = await centerOf("root.R_LIMIT.R");
    await mouse("move 640 103", "down left", "move 660 123", "move 690 153", "move 720 183", "move 720 183");
    const during = await centerOf("root.R_LIMIT.R");
    await mouse("up left");
    // The viewer snaps to its own 0.1 unit while dragging, so allow a few px.
    const dx = during.x - before.x;
    const dy = during.y - before.y;
    check("symbol follows 1:1", Math.abs(dx - 80) <= 4 && Math.abs(dy - 80) <= 4, `moved (${dx},${dy}), expected ~(80,80)`);
  }

  console.log("no drift across many mousemoves (regression: in-progress events fed back into the viewer)");
  await open();
  {
    const before = await centerOf("root.R_LIMIT.R");
    const steps = ["move 640 103", "down left"];
    for (let i = 1; i <= 20; i += 1) steps.push(`move ${640 + i * 3} ${103 + i * 2}`);
    await mouse(...steps);
    const during = await centerOf("root.R_LIMIT.R");
    await mouse("up left");
    const dx = during.x - before.x;
    const dy = during.y - before.y;
    check("20 moves accumulate no extra offset", Math.abs(dx - 60) <= 4 && Math.abs(dy - 40) <= 4, `moved (${dx},${dy}), expected ~(60,40)`);
  }

  console.log("marquee + group drag");
  await open();
  {
    await marqueeDivider();
    const sel = await selection();
    check("plain drag rubber-bands", sel.length === 2, JSON.stringify(sel));
    check("frame renders for a multi-selection", await evalJson("!!document.querySelector('.sel-frame')"));

    await clearSaves();
    await mouse("move 640 459", "down left", "move 660 475", "move 685 500");
    const ghosts = await evalJson(
      "[...document.querySelectorAll('div')].filter(d=>d.style&&d.style.border&&d.style.border.includes('dashed')).length",
    );
    check("other members preview live during the drag", ghosts >= 1, `${ghosts} ghosts`);
    await mouse("up left");

    const written = await saves();
    check("one save carries the whole group", written.length === 1 && Object.keys(written[0]).length === 2, JSON.stringify(written));
    const [a, b] = Object.values(written[0] ?? {});
    check("group keeps its shape", a && b && Math.abs(a.x - b.x) < 1e-9, JSON.stringify(written[0]));
    check("selection survives the drag", (await selection()).length === 2, JSON.stringify(await selection()));
  }

  console.log("click targeting (regression: pin discs swallowed wires and bodies)");
  await open();
  {
    await mouse("move 640 135", "down left", "up left");
    check("clicking a wire selects its net", JSON.stringify(await selection()) === '["LED_A"]', JSON.stringify(await selection()));
    const glow = await evalJson(
      "getComputedStyle(document.querySelector('g[data-schematic-trace-id=\"schtrace:LED_A\"]')).filter",
    );
    check("selected wire is highlighted", glow.includes("drop-shadow"), glow);

    await mouse("move 640 103", "down left", "up left");
    check("clicking a body selects the component", JSON.stringify(await selection()) === '["root.R_LIMIT.R"]', JSON.stringify(await selection()));
  }

  console.log("modes");
  await open();
  {
    const modes = () => evalJson("[...document.querySelectorAll('button[aria-pressed]')].map(b=>b.textContent+':'+b.getAttribute('aria-pressed'))");
    await ab("eval", "(()=>{window.__repro.key('w'); return 1;})()");
    check("W arms the wire tool", (await modes()).some((m) => m.startsWith("WireW:true")));
    check("wire mode shows a crosshair", (await evalJson("getComputedStyle(document.querySelector('.dotgrid')).cursor")) === "crosshair");
    await ab("eval", "(()=>{window.__repro.key('Escape'); return 1;})()");
    check("Esc returns to select", (await modes()).some((m) => m.startsWith("SelectEsc:true")));
    check("select mode does not look like pan", (await evalJson("getComputedStyle(document.querySelector('.dotgrid')).cursor")) === "default");
    await ab("eval", "(()=>{window.__repro.key('h'); return 1;})()");
    check("H arms the hand tool", (await modes()).some((m) => m.startsWith("PanH:true")));
    check("pan mode shows grab", (await evalJson("getComputedStyle(document.querySelector('.dotgrid')).cursor")) === "grab");
  }

  console.log("wire tool connects adjacent pins (regression: both clicks resolved to one pin)");
  await open();
  {
    await ab("console", "--clear");
    await ab("eval", "(()=>{window.__repro.key('w'); return 1;})()");
    await mouse("move 640 128", "down left", "up left");
    await mouse("move 640 143", "down left", "up left");
    const log = await ab("console");
    check(
      "two adjacent pins resolve to two different pins",
      log.includes("connectPins") && log.includes("D_STATUS.LED"),
      log.split("\n").filter((l) => l.includes("connectPins")).join(" | ") || "no connectPins logged",
    );
  }

  console.log("pan does not disturb the selection");
  await open();
  {
    await marqueeDivider();
    await ab("eval", "(()=>{window.dispatchEvent(new KeyboardEvent('keydown',{key:' ',code:'Space',bubbles:true})); return 1;})()");
    await mouse("move 400 300", "down left", "move 450 340", "move 450 340", "up left");
    await ab("eval", "(()=>{window.dispatchEvent(new KeyboardEvent('keyup',{key:' ',code:'Space',bubbles:true})); return 1;})()");
    check("selection survives a pan", (await selection()).length === 2, JSON.stringify(await selection()));
  }

  console.log("wheel still zooms (regression: the pan predicate also gates wheel events)");
  await open();
  {
    await mouse("move 640 300");
    await ab("mouse", "wheel", "-300");
    const t = await evalJson(
      "(()=>{const d=document.querySelector('.dotgrid div[style*=matrix]'); return d?d.style.transform:'none';})()",
    );
    check("wheel zooms", t.startsWith("matrix(") && !t.startsWith("matrix(1,"), t);
  }

  console.log("chat: resuming a session renders its notice without looping");
  {
    // ?state=resume renders the system notice on load, so simply opening it
    // exercises the component that used to re-render forever.
    await ab("console", "--clear");
    await openState("resume");
    const log = await ab("console");
    // A selector returning a fresh object made every snapshot compare unequal,
    // so any system notice (session error, interrupt, resume) re-rendered
    // forever. React reports it as an uncached getSnapshot, then as exceeded
    // update depth.
    check(
      "no render loop on a system message",
      !/maximum update depth|infinite loop/i.test(log),
      log.split("\n").find((l) => /maximum update depth|infinite loop/i.test(l)) ?? "",
    );
    check(
      "the notice actually renders",
      await evalJson('document.body.innerText.includes("Resuming previous session")'),
    );
  }

  console.log("a render error shows a recovery card instead of a blank window");
  {
    await openState("crash");
    check("the boundary catches", await evalJson('!!document.querySelector("[data-testid=error-boundary]")'));
    check("the window is not blank", await evalJson("document.body.innerText.trim().length > 0"));
    check(
      "recovery is offered",
      (await evalJson('[...document.querySelectorAll("button")].map(b=>b.textContent.trim())')).join(
        ",",
      ) === "Try again,Reload,Copy details",
    );
    // Retrying re-mounts the subtree; this one always throws, so the card comes
    // back rather than the window going blank.
    await ab(
      "eval",
      '(()=>{const b=[...document.querySelectorAll("button")].find(x=>x.textContent.trim()==="Try again"); if(b) b.click(); return 1;})()',
    );
    check("retry cannot blank the window", await evalJson("document.body.innerText.trim().length > 0"));
  }

  console.log("async failures surface instead of vanishing (boundaries cannot see them)");
  {
    await openState("async");
    const clickText = (t) =>
      ab(
        "eval",
        `(()=>{const b=[...document.querySelectorAll("button")].find(x=>/${t}/.test(x.textContent)); if(b) b.click(); return !!b;})()`,
      );
    await clickText("reject a promise");
    check("an unhandled rejection is reported", await evalJson('!!document.querySelector("[data-testid=global-error]")'));
    await clickText("throw in a timeout");
    check(
      "a throw outside render is reported",
      (await evalJson('(document.querySelector("[data-testid=global-error] pre")||{}).innerText || ""')).includes(
        "thrown in a timeout",
      ),
    );
    await ab(
      "eval",
      '(()=>{const x=[...document.querySelectorAll("[data-testid=global-error] button")].find(b=>b.title==="Dismiss"); if(x) x.click(); return 1;})()',
    );
    check("it can be dismissed", !(await evalJson('!!document.querySelector("[data-testid=global-error]")')));
    await clickText("ignored noise");
    check(
      "known noise raises nothing",
      !(await evalJson('!!document.querySelector("[data-testid=global-error]")')),
    );
  }

  await ab("close").catch(() => {});

  console.log(`\n${passed} passed, ${failures.length} failed`);
  if (failures.length > 0) process.exit(1);
}

main().catch((err) => {
  console.error("\ngesture-check could not run:", err.message);
  console.error("Is the repro server up on 5199, and is `agent-browser` installed?");
  process.exit(2);
});
