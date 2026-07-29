#!/usr/bin/env node
/**
 * Live LAN latency probe against a running couchlink host.
 *
 * Opens the join URL in Chromium, waits for WebRTC connected, samples
 * inbound-rtp getStats, and fails if jitter buffer / fps regress past the
 * gates locked in web/src/latencyStats.ts (JB ≤ 20ms, fps ≥ 50, drops = 0).
 *
 * Usage: node scripts/regression-latency-live.mjs '<join-url>'
 */
import { createRequire } from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { execSync } from "node:child_process";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const webDir = path.join(root, "web");
const joinUrl = process.argv[2];
if (!joinUrl) {
  console.error("usage: node scripts/regression-latency-live.mjs <join-url>");
  process.exit(2);
}

const GATES = { maxJitterBufferMs: 20, minDecodeFps: 50, maxFramesDropped: 0 };

function evaluate(sample) {
  const failures = [];
  if (sample.jitterBufferMs > GATES.maxJitterBufferMs) {
    failures.push(
      `jitterBufferMs ${sample.jitterBufferMs.toFixed(1)} > ${GATES.maxJitterBufferMs}`
    );
  }
  if (sample.decodeFps < GATES.minDecodeFps) {
    failures.push(`decodeFps ${sample.decodeFps.toFixed(1)} < ${GATES.minDecodeFps}`);
  }
  if (sample.framesDropped > GATES.maxFramesDropped) {
    failures.push(`framesDropped ${sample.framesDropped} > ${GATES.maxFramesDropped}`);
  }
  return { ok: failures.length === 0, failures };
}

const require = createRequire(import.meta.url);
function loadPlaywright() {
  try {
    return require(path.join(webDir, "node_modules/playwright"));
  } catch {
    console.error("Installing playwright (one-time)…");
    execSync("npm install --no-fund --no-audit playwright@1", {
      cwd: webDir,
      stdio: "inherit",
    });
    execSync("npx playwright install chromium", { cwd: webDir, stdio: "inherit" });
    return require(path.join(webDir, "node_modules/playwright"));
  }
}

const { chromium } = loadPlaywright();
const browser = await chromium.launch({
  headless: true,
  args: ["--autoplay-policy=no-user-gesture-required"],
});
const page = await browser.newPage();

// Capture every RTCPeerConnection the page creates so we can read getStats.
await page.addInitScript(() => {
  const Orig = window.RTCPeerConnection;
  const pcs = [];
  function Patched(...args) {
    const pc = new Orig(...args);
    pcs.push(pc);
    return pc;
  }
  Patched.prototype = Orig.prototype;
  Object.keys(Orig).forEach((k) => {
    try {
      Patched[k] = Orig[k];
    } catch {
      /* ignore */
    }
  });
  window.RTCPeerConnection = Patched;
  window.__couchlinkPcs = pcs;
});

const consoleLines = [];
page.on("console", (msg) => {
  const t = msg.text();
  if (/couchlink|present|canvas|video stats/i.test(t)) consoleLines.push(t);
});

console.log("opening", joinUrl);
await page.goto(joinUrl, { waitUntil: "domcontentloaded", timeout: 30_000 });

await page.waitForFunction(
  () => {
    const pill = document.querySelector(".pill");
    return pill && /connected/i.test(pill.textContent || "");
  },
  { timeout: 60_000 }
);
console.log("UI state: connected — settling 3s…");
await page.waitForTimeout(3000);

async function readInbound() {
  return page.evaluate(async () => {
    const pcs = window.__couchlinkPcs || [];
    for (const pc of pcs) {
      if (!pc || pc.connectionState === "closed") continue;
      const report = await pc.getStats();
      let found = null;
      report.forEach((r) => {
        if (r.type === "inbound-rtp" && r.kind === "video") {
          found = {
            jitterBufferDelay: r.jitterBufferDelay ?? 0,
            jitterBufferEmittedCount: r.jitterBufferEmittedCount ?? 0,
            framesDecoded: r.framesDecoded ?? 0,
            framesDropped: r.framesDropped ?? 0,
            frameHeight: r.frameHeight ?? 0,
            jitterBufferTarget: r.jitterBufferTarget ?? null,
          };
        }
      });
      if (found) return found;
    }
    return null;
  });
}

const samples = [];
let prev = await readInbound();
if (!prev) {
  console.error("FAIL: no inbound-rtp stats — host may not be streaming video");
  console.error("console:\n", consoleLines.slice(-30).join("\n"));
  await browser.close();
  process.exit(1);
}

for (let i = 0; i < 4; i++) {
  await page.waitForTimeout(2000);
  const next = await readInbound();
  if (!next) continue;
  const countDelta = next.jitterBufferEmittedCount - prev.jitterBufferEmittedCount;
  if (countDelta > 0) {
    const delayDelta = next.jitterBufferDelay - prev.jitterBufferDelay;
    const decodedDelta = next.framesDecoded - prev.framesDecoded;
    samples.push({
      jitterBufferMs: (delayDelta / countDelta) * 1000,
      decodeFps: decodedDelta / 2,
      framesDropped: next.framesDropped,
      frameHeight: next.frameHeight,
    });
  }
  prev = next;
}

const present = await page.evaluate(() => {
  const spans = Array.from(document.querySelectorAll("footer.meta span"));
  const text = spans.map((s) => s.textContent || "").join(" ");
  if (/present:\s*canvas/i.test(text) || /canvas:/i.test(text)) return "canvas";
  if (/present:\s*video/i.test(text) || /video:/i.test(text)) return "video";
  return "unknown";
});

await browser.close();

console.log("present path:", present);
console.log("samples:");
for (const s of samples) {
  console.log(
    `  JB=${s.jitterBufferMs.toFixed(1)}ms  fps=${s.decodeFps.toFixed(1)}  drops=${s.framesDropped}  h=${s.frameHeight}`
  );
}

if (samples.length === 0) {
  console.error("FAIL: no usable getStats windows");
  process.exit(1);
}

const last = samples[samples.length - 1];
const avg = {
  jitterBufferMs: samples.reduce((a, s) => a + s.jitterBufferMs, 0) / samples.length,
  decodeFps: samples.reduce((a, s) => a + s.decodeFps, 0) / samples.length,
  framesDropped: last.framesDropped,
};
console.log(
  `average: JB=${avg.jitterBufferMs.toFixed(1)}ms  fps=${avg.decodeFps.toFixed(1)}  drops=${avg.framesDropped}`
);

const gate = evaluate(avg);
if (present !== "canvas") {
  console.warn("WARN: expected present: canvas on Chromium");
}
if (!gate.ok) {
  console.error("FAIL latency gates:", gate.failures.join("; "));
  process.exit(1);
}
console.log("LIVE latency regression PASS (vs gates JB≤20ms fps≥50 drops=0)");
console.log(
  `baseline reference from prior session: JB≈6–9ms fps≈59 — now JB=${avg.jitterBufferMs.toFixed(1)}ms fps=${avg.decodeFps.toFixed(1)}`
);
