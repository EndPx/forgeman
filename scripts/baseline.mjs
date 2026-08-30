#!/usr/bin/env node
// ForgeMan evaluation — FAIR BASELINE (hackathon brief: "one direct prompt
// with basic instructions").
//
// What it does, deliberately naive:
//   1. reads the repository files,
//   2. sends ONE direct prompt to the same LLM a human would use,
//   3. applies whatever edits come back,
//   4. runs the repository's own test suite,
//   5. records the outcome.
// No plan, no diagnosis, no verification loop, no retries beyond re-prompting
// a fresh attempt. This is "the manual process people use today" encoded.
//
// Usage:  node scripts/baseline.mjs [attempts-per-case]
// Requires ZAI_API_KEY in the environment or ../../.env (same key ForgeMan uses).

import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";

const ROOT = path.resolve(import.meta.dirname, "..");
const MODEL = "glm-4.7-flash";
const BASE_URL = "https://api.z.ai/api/paas/v4";

const CASES = [
  { repo: "examples/flawed-api", kind: "cargo", attempts: 0 },
  { repo: "examples/flawed-js", kind: "npm", attempts: 2 },
  { repo: "examples/flawed-py", kind: "unittest", attempts: 2 },
];

const TASK = "This repository has failing tests and a performance problem. Fix the code so all tests pass and the performance problem is gone.";

function loadApiKey() {
  if (process.env.ZAI_API_KEY) return process.env.ZAI_API_KEY;
  const envPath = path.join(ROOT, ".env");
  if (fs.existsSync(envPath)) {
    const line = fs.readFileSync(envPath, "utf8").split("\n")
      .find((l) => l.startsWith("ZAI_API_KEY="));
    if (line) return line.slice("ZAI_API_KEY=".length).trim();
  }
  throw new Error("ZAI_API_KEY not set (add it to .env)");
}

function listFiles(dir, base = dir, out = []) {
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    const full = path.join(dir, entry.name);
    const rel = path.relative(base, full).replace(/\\/g, "/");
    if (entry.isDirectory()) {
      if (![".git", "target", ".forgeman", "node_modules", "__pycache__"].includes(entry.name)
        && !entry.name.startsWith(".")) {
        listFiles(full, base, out);
      }
    } else if (rel !== "Cargo.lock" && !rel.endsWith(".lock")) {
      out.push({ path: rel, content: fs.readFileSync(full, "utf8") });
    }
  }
  return out;
}

function extractJson(text) {
  const trimmed = text.trim();
  try { return JSON.parse(trimmed); } catch { /* fall through */ }
  const start = trimmed.indexOf("{");
  const end = trimmed.lastIndexOf("}");
  if (start >= 0 && end > start) {
    try { return JSON.parse(trimmed.slice(start, end + 1)); } catch { /* fall through */ }
  }
  return null;
}

async function callLlmOnce(apiKey, prompt) {
  const response = await fetch(`${BASE_URL}/chat/completions`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${apiKey}`,
      "content-type": "application/json",
      accept: "application/json",
    },
    body: JSON.stringify({
      model: MODEL,
      messages: [{ role: "user", content: prompt }],
      max_tokens: 8192,
      stream: false,
    }),
    signal: AbortSignal.timeout(300_000),
  });
  const raw = await response.text();
  if (!response.ok) {
    const err = new Error(`LLM ${response.status}: ${raw.slice(0, 200)}`);
    err.status = response.status;
    throw err;
  }
  const parsed = JSON.parse(raw);
  return parsed.choices?.[0]?.message?.content ?? "";
}

// The naive baseline does not retry — but the eval must not be confounded by
// transient free-tier 429s, so transient failures get a bounded backoff.
async function callLlm(apiKey, prompt) {
  for (let attempt = 1; ; attempt++) {
    try {
      return await callLlmOnce(apiKey, prompt);
    } catch (err) {
      if (err.status !== 429 || attempt >= 4) throw err;
      const waitMs = 30_000 * attempt;
      console.error(`429 — waiting ${waitMs / 1000}s (retry ${attempt}/3)`);
      await new Promise((resolve) => setTimeout(resolve, waitMs));
    }
  }
}

// Shell-free process runner: background environments sometimes lack a usable
// cmd.exe for execSync, so we spawn the real executables directly.
function run(program, args, cwd) {
  const result = spawnSync(program, args, {
    cwd,
    encoding: "utf8",
    timeout: 600_000,
    windowsHide: true,
  });
  return {
    ok: result.status === 0,
    output: (result.stdout ?? "") + (result.stderr ?? ""),
    error: result.error?.message,
  };
}

const NPM = process.platform === "win32" ? "npm.cmd" : "npm";

const TEST_RUNNERS = {
  cargo: (repo) => run("cargo", ["test", "--quiet"], repo),
  npm: (repo) => {
    // npm is a .cmd shim — node >= 20.12 refuses to spawn it without a shell.
    const result = spawnSync(NPM, ["test"], {
      cwd: repo,
      encoding: "utf8",
      timeout: 600_000,
      windowsHide: true,
      shell: process.platform === "win32",
    });
    return {
      ok: result.status === 0,
      output: (result.stdout ?? "") + (result.stderr ?? ""),
      error: result.error?.message,
    };
  },
  unittest: (repo) =>
    run("python", ["-m", "unittest", "discover", "-s", "tests", "-t", "."], repo),
};

function runTests(repo, kind) {
  const result = TEST_RUNNERS[kind](repo);
  return { ok: result.ok, output: result.output, error: result.error };
}

function summarizeTests(output) {
  const tap = output.match(/^#\s+tests\s+(\d+)/m)?.[1];
  const tapPass = output.match(/^#\s+pass\s+(\d+)/m)?.[1];
  const tapFail = output.match(/^#\s+fail\s+(\d+)/m)?.[1];
  if (tap) return { total: +tap, passed: +(tapPass ?? 0), failed: +(tapFail ?? 0) };
  const unittest = output.match(/^Ran (\d+) tests/m)?.[1];
  if (unittest) {
    const failed = (output.match(/failures=(\d+)/)?.[1] ?? 0) + (output.match(/errors=(\d+)/)?.[1] ?? 0);
    return { total: +unittest, passed: Math.max(0, +unittest - failed), failed: +failed };
  }
  const cargo = output.match(/test result: \w+\. (\d+) passed; (\d+) failed/);
  if (cargo) return { total: +cargo[1] + +cargo[2], passed: +cargo[1], failed: +cargo[2] };
  return null;
}

async function attempt(repoAbs, kind, apiKey, attemptNumber) {
  // Reset to the baseline commit before every attempt.
  run("git", ["checkout", "--", "."], repoAbs);
  run("git", ["clean", "-fdq"], repoAbs);

  const files = listFiles(repoAbs);
  const filesBlock = files
    .map((f) => `--- FILE: ${f.path} ---\n${f.content}\n--- END FILE ---`)
    .join("\n");
  const prompt = `${TASK}\n\nREPOSITORY FILES:\n${filesBlock}\n\nReturn ONLY a JSON object: {"edits":[{"path":"relative/path","action":"write","content":"COMPLETE new file content"}]}`;

  const t0 = Date.now();
  const text = await callLlm(apiKey, prompt);
  const llmMs = Date.now() - t0;

  const parsed = extractJson(text);
  let applied = 0;
  if (parsed?.edits) {
    for (const edit of parsed.edits) {
      const target = path.join(repoAbs, edit.path ?? "");
      if (!path.resolve(target).startsWith(path.resolve(repoAbs))) continue;
      if (edit.action === "write" && typeof edit.content === "string") {
        fs.mkdirSync(path.dirname(target), { recursive: true });
        fs.writeFileSync(target, edit.content);
        applied++;
      }
    }
  }

  const test = runTests(repoAbs, kind);
  const tests = summarizeTests(test.output);
  return {
    attempt: attemptNumber,
    llm_ms: llmMs,
    edits_applied: applied,
    tests_ok: test.ok,
    tests,
    note: parsed ? undefined : "LLM output contained no parsable JSON",
    output_tail: test.output.split("\n").filter(Boolean).slice(-6).join(" | ").slice(0, 400),
  };
}

const apiKey = loadApiKey();
const attemptsPerCase = Number(process.argv[2] ?? 0) || null; // 0 => per-case defaults
const report = [];

for (const testCase of CASES) {
  const repoAbs = path.resolve(ROOT, testCase.repo);
  const count = attemptsPerCase ?? testCase.attempts;
  const results = [];
  for (let n = 1; n <= count; n++) {
    process.stderr.write(`baseline ${path.basename(repoAbs)} attempt ${n} …\n`);
    try {
      results.push(await attempt(repoAbs, testCase.kind, apiKey, n));
    } catch (err) {
      results.push({ attempt: n, error: String(err.message ?? err) });
    }
  }
  report.push({
    case: path.basename(repoAbs),
    test_kind: testCase.kind,
    attempts: results,
    any_pass: results.some((r) => r.tests_ok),
  });
}

const outPath = path.join(ROOT, "docs", "baseline-results.json");
fs.writeFileSync(outPath, JSON.stringify(report, null, 2));
console.log(JSON.stringify(report.map(({ case: c, any_pass, attempts }) => ({
  case: c,
  attempts: attempts.map((a) => ({ attempt: a.attempt, tests: a.tests, tests_ok: a.tests_ok, note: a.note, error: a.error })),
  any_pass,
})), null, 2));
console.log(`\nfull results written to ${outPath}`);
