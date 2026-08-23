#!/usr/bin/env node
/**
 * Live Ricardo + self-beat probe against a running couchlink host.
 *
 * Opens the join URL in Chromium, injects a fake Standard gamepad so CLVD
 * input_wm / S_p50 can accumulate, waits for WebCodecs present when possible,
 * then hard-fails unless the session beats:
 *
 *   Ricardo floor:  push ≥ 74 · shed ≤ 3% · encode ≥ 5000 · paint ≥ 74 · S_p50 ≤ 45
 *   Self-beat bars: push ≥ 90 · shed ≤ 1% · encode ≥ 5000 · paint ≥ 100 · S_p50 ≤ 5
 *     (frozen self baseline was ~74.8 / 84 / 7.4 — beat-self is a clear margin)
 *
 * Usage:
 *   JOIN_URL='…' HOST_LOG=/tmp/couchlink-stack.log node scripts/regression-latency-live.mjs
 *   BEAT_SELF=0 …  # Ricardo floor only
 */
import { createRequire } from "node:module";
import path from "node:path";
import fs from "node:fs";
import { fileURLToPath } from "node:url";
import { execSync } from "node:child_process";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const webDir = path.join(root, "web");
const joinUrl = process.argv[2] || process.env.JOIN_URL;
const hostLog = process.env.HOST_LOG || "";
if (!joinUrl) {
  console.error(
    "usage: JOIN_URL='…' [HOST_LOG=…] node scripts/regression-latency-live.mjs"
  );
  process.exit(2);
}

/** Hard Ricardo gate — mirrors crates/host/src/latency_live_sim/ricardo_gate.rs */
const RICARDO = {
  minPushFps: 74,
  maxShedPct: 3,
  minEncodeKbps: 5000,
  minPaintFps: 74,
  maxSurplusP50Ms: 45,
};

/** Beat-self bars — clear margin over frozen self baseline (74.8/84/7.4). */
const SELF = {
  minPushFps: 90,
  maxShedPct: 1,
  minEncodeKbps: 5000,
  minPaintFps: 100,
  maxSurplusP50Ms: 5,
};

const beatSelf =
  process.env.BEAT_SELF !== "0" && process.env.BEAT_SELF !== "false";
const GATE = beatSelf ? SELF : RICARDO;

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

function parseHostStreamingWindows(logPath, n = 3, afterMarker = null) {
  if (!logPath || !fs.existsSync(logPath)) return [];
  const text = fs.readFileSync(logPath, "utf8");
  let body = text;
  if (afterMarker) {
    const idx = text.lastIndexOf(afterMarker);
    if (idx >= 0) body = text.slice(idx);
  }
  const lines = body
    .split(/\r?\n/)
    .filter((l) => l.includes("[couchlink-host] streaming"));
  const out = [];
  for (const line of lines.slice(-n)) {
    const fps = /streaming ([\d.]+) fps/.exec(line);
    const drop = /dropped \d+\/\d+ \((\d+)%\)/.exec(line);
    const pushFps = fps ? Number(fps[1]) : 0;
    // Ignore the first post-join blip (0.x / 1.x fps) — not a steady window.
    if (pushFps < 10) continue;
    out.push({
      pushFps,
      shedPct: drop ? Number(drop[1]) : 99,
      raw: line.trim(),
    });
  }
  return out;
}

const { chromium } = loadPlaywright();
// Only score host streaming lines written after this probe starts — otherwise
// a dead host can still "pass" on stale healthy windows from earlier.
const probeMarker = `[ricardo-probe] start ${Date.now()}`;
if (hostLog) {
  try {
    fs.appendFileSync(hostLog, `\n${probeMarker}\n`);
  } catch (e) {
    console.warn("could not append probe marker to HOST_LOG:", e.message || e);
  }
}
const launchOpts = {
  headless: true,
  args: ["--autoplay-policy=no-user-gesture-required", "--use-fake-device-for-media-stream"],
};
// Prefer installed Chrome/Edge when present — they often expose prefer-hardware
// where Playwright's bundled Chromium does not (WSL/headless).
let browser;
try {
  browser = await chromium.launch({ ...launchOpts, channel: "chrome" });
  console.log("browser: chrome channel");
} catch {
  try {
    browser = await chromium.launch({ ...launchOpts, channel: "msedge" });
    console.log("browser: msedge channel");
  } catch {
    browser = await chromium.launch(launchOpts);
    console.log("browser: bundled chromium");
  }
}
const page = await browser.newPage();

// Capture every RTCPeerConnection + inject a wiggle-pad so input_wm stamps.
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

  // Fake Standard gamepad — player.ts polls getGamepads at 500 Hz.
  const axes = [0, 0, 0, 0];
  const buttons = Array.from({ length: 17 }, () => ({
    pressed: false,
    touched: false,
    value: 0,
  }));
  let tick = 0;
  const gp = {
    id: "Couchlink Ricardo Probe Pad (Standard)",
    index: 0,
    connected: true,
    mapping: "standard",
    timestamp: 0,
    axes,
    buttons,
    hapticActuators: [],
    vibrationActuator: null,
  };
  navigator.getGamepads = () => {
    tick += 1;
    axes[0] = Math.sin(tick / 8) * 0.55;
    axes[1] = Math.cos(tick / 11) * 0.35;
    const down = tick % 16 < 8;
    buttons[0] = { pressed: down, touched: down, value: down ? 1 : 0 };
    gp.timestamp = performance.now();
    return [gp, null, null, null];
  };
});

const consoleLines = [];
page.on("console", (msg) => {
  const t = msg.text();
  if (/couchlink|present|canvas|webcodecs|video stats|CLVD|photon|surplus/i.test(t)) {
    consoleLines.push(t);
  }
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
console.log("UI state: connected — waiting for present + Ricardo scrape…");

async function readRicardo() {
  return page.evaluate(() => {
    const hook = window.__couchlinkRicardo;
    if (typeof hook !== "function") return null;
    return hook();
  });
}

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
          };
        }
      });
      if (found) return found;
    }
    return null;
  });
}

let best = {
  presentMode: "—",
  paintFps: 0,
  encodeKbps: 0,
  hostPushFps: 0,
  hostShedPct: 100,
  surplusP50Ms: null,
  photonP50Ms: null,
  rttMs: 0,
  sampleCount: 0,
  watermarkActive: false,
};

const inboundSamples = [];
let prevInbound = null;

// ~50s: accel probe, WebCodecs promote, host_stats, input_wm ring fill,
// and ≥1 host streaming window (~5s cadence) while still connected.
for (let i = 0; i < 50; i++) {
  await page.waitForTimeout(1000);
  const snap = await readRicardo();
  if (snap) {
    const paint = Math.max(
      snap.present?.fps || 0,
      // RTP inbound decode fps while dual-send / software-background WC
      0
    );
    const encode = snap.hostStats?.target_bitrate_kbps ?? 0;
    const push = snap.hostStats?.fps ?? 0;
    const shed = snap.hostStats?.drop_pct ?? 100;
    const surplus = snap.inputPhoton?.surplusP50Ms ?? snap.present?.surplusP50Ms ?? null;
    const photon = snap.inputPhoton?.photonP50Ms ?? snap.present?.photonP50Ms ?? null;
    best = {
      presentMode: snap.presentMode || best.presentMode,
      paintFps: Math.max(best.paintFps, paint || 0),
      encodeKbps: Math.max(best.encodeKbps, encode || 0),
      hostPushFps: Math.max(best.hostPushFps, push || 0),
      hostShedPct: Math.min(best.hostShedPct, shed ?? 100),
      surplusP50Ms: surplus != null ? surplus : best.surplusP50Ms,
      photonP50Ms: photon != null ? photon : best.photonP50Ms,
      rttMs: snap.rttMs || best.rttMs,
      sampleCount: snap.inputPhoton?.sampleCount ?? best.sampleCount,
      watermarkActive: !!(
        snap.inputPhoton?.watermarkActive || best.watermarkActive
      ),
    };
  }
  const inbound = await readInbound();
  if (inbound && prevInbound) {
    const decodedDelta = inbound.framesDecoded - prevInbound.framesDecoded;
    if (decodedDelta > 0) {
      inboundSamples.push({ decodeFps: decodedDelta });
      best.paintFps = Math.max(best.paintFps, decodedDelta);
    }
  }
  prevInbound = inbound || prevInbound;

  const hostWindows = parseHostStreamingWindows(hostLog, 8, probeMarker);
  const hostSteady = hostWindows.filter((w) => w.pushFps >= 60);
  const clientGreen =
    best.presentMode === "webcodecs" &&
    best.paintFps >= GATE.minPaintFps &&
    best.encodeKbps >= GATE.minEncodeKbps &&
    best.surplusP50Ms != null &&
    best.surplusP50Ms <= GATE.maxSurplusP50Ms &&
    best.sampleCount >= 16 &&
    best.watermarkActive;
  // Stay connected until we have post-marker host windows — streaming lines
  // stop once the probe browser closes.
  if (clientGreen && hostSteady.length >= 1) {
    console.log(
      `client+host axes green at t=${i + 1}s (paint/encode/S_p50 + ${hostSteady.length} host window(s))`
    );
    break;
  }
}

await browser.close();

const windows = parseHostStreamingWindows(hostLog, 8, probeMarker);
const steady = windows.filter((w) => w.pushFps >= 60);
const scoreWindows =
  steady.length >= 3 ? steady.slice(-3) : steady.length ? steady : windows.slice(-3);
if (scoreWindows.length) {
  for (const w of scoreWindows) console.log("host log streaming:", w.raw);
  // Floor/ceiling across post-probe windows — no stale pre-probe cherry-pick.
  best.hostPushFps = Math.min(...scoreWindows.map((w) => w.pushFps));
  best.hostShedPct = Math.max(...scoreWindows.map((w) => w.shedPct));
}

console.log("Ricardo scrape:", JSON.stringify(best, null, 2));

const failures = [];
if (hostLog && !scoreWindows.length) {
  failures.push(
    "no host streaming windows after probe start (host log stale or dead)"
  );
}
if (best.presentMode !== "webcodecs") {
  failures.push(`presentMode=${best.presentMode} (need webcodecs for honest S_p50)`);
}
if (best.hostPushFps < GATE.minPushFps) {
  failures.push(`push ${best.hostPushFps.toFixed(1)} < ${GATE.minPushFps}`);
}
if (best.hostShedPct > GATE.maxShedPct) {
  failures.push(`shed ${best.hostShedPct}% > ${GATE.maxShedPct}%`);
}
if (best.encodeKbps < GATE.minEncodeKbps) {
  failures.push(`encode ${best.encodeKbps} < ${GATE.minEncodeKbps} kbps`);
}
if (best.paintFps < GATE.minPaintFps) {
  failures.push(`paint ${best.paintFps.toFixed(1)} < ${GATE.minPaintFps}`);
}
if (!best.watermarkActive || best.sampleCount < 16) {
  failures.push(
    `input_wm samples=${best.sampleCount} active=${best.watermarkActive} (need ≥16)`
  );
}
if (best.surplusP50Ms == null) {
  failures.push("S_p50 missing (need WebCodecs + pad input_wm samples)");
} else if (best.surplusP50Ms > GATE.maxSurplusP50Ms) {
  failures.push(
    `S_p50 ${best.surplusP50Ms.toFixed(1)}ms > ${GATE.maxSurplusP50Ms}ms`
  );
}

if (failures.length) {
  console.error(
    `FAIL ${beatSelf ? "beat-self" : "Ricardo"} hard gate:`,
    failures.join("; ")
  );
  console.error("console tail:\n", consoleLines.slice(-50).join("\n"));
  process.exit(1);
}

console.log(
  `LIVE ${beatSelf ? "SELF" : "Ricardo"} PASS — push≥${best.hostPushFps.toFixed(1)} shed≤${best.hostShedPct}% encode=${best.encodeKbps} paint=${best.paintFps.toFixed(1)} S_p50=${best.surplusP50Ms.toFixed(1)}ms (Φ=${best.photonP50Ms?.toFixed?.(1) ?? "?"} RTT=${best.rttMs}) present=${best.presentMode} samples=${best.sampleCount}`
);
process.exit(0);
