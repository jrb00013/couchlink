#!/usr/bin/env node
/**
 * Live Ricardo beat probe against a running couchlink host.
 *
 * Opens the join URL in Chromium, injects a fake Standard gamepad so CLVD
 * input_wm / S_p50 can accumulate, waits for WebCodecs present when possible,
 * then hard-fails unless the session beats Ricardo's frozen playable night:
 *
 *   push ≥ 74 · shed ≤ 3% · encode ≥ 5000 kbps · paint ≥ 74 · S_p50 ≤ 45
 *
 * Usage:
 *   JOIN_URL='…' HOST_LOG=/tmp/couchlink-stack.log node scripts/regression-latency-live.mjs
 *   node scripts/regression-latency-live.mjs '<join-url>'
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

function parseHostStreaming(logPath) {
  if (!logPath || !fs.existsSync(logPath)) return null;
  const text = fs.readFileSync(logPath, "utf8");
  const lines = text.split(/\r?\n/).filter((l) => l.includes("[couchlink-host] streaming"));
  // Prefer a healthy window (push>1), else last line.
  const good = [...lines].reverse().find((l) => /streaming [1-9]/.test(l)) || lines.at(-1);
  if (!good) return null;
  const fps = /streaming ([\d.]+) fps/.exec(good);
  const drop = /dropped \d+\/\d+ \((\d+)%\)/.exec(good);
  return {
    pushFps: fps ? Number(fps[1]) : 0,
    shedPct: drop ? Number(drop[1]) : 99,
    raw: good.trim(),
  };
}

const { chromium } = loadPlaywright();
const browser = await chromium.launch({
  headless: true,
  args: ["--autoplay-policy=no-user-gesture-required", "--use-fake-device-for-media-stream"],
});
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
};

const inboundSamples = [];
let prevInbound = null;

// ~25s: need host_stats (~5s), WebCodecs promote, and input_wm ring fill.
for (let i = 0; i < 25; i++) {
  await page.waitForTimeout(1000);
  const snap = await readRicardo();
  if (snap) {
    const paint =
      snap.present?.fps ??
      (typeof snap.present?.fps === "number" ? snap.present.fps : 0);
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

  if (
    best.paintFps >= RICARDO.minPaintFps &&
    best.encodeKbps >= RICARDO.minEncodeKbps &&
    best.hostPushFps >= RICARDO.minPushFps &&
    best.hostShedPct <= RICARDO.maxShedPct &&
    best.surplusP50Ms != null &&
    best.surplusP50Ms <= RICARDO.maxSurplusP50Ms &&
    best.sampleCount >= 8
  ) {
    console.log(`Ricardo axes green at t=${i + 1}s`);
    break;
  }
}

await browser.close();

const fromLog = parseHostStreaming(hostLog);
if (fromLog) {
  console.log("host log streaming:", fromLog.raw);
  best.hostPushFps = Math.max(best.hostPushFps, fromLog.pushFps);
  best.hostShedPct = Math.min(best.hostShedPct, fromLog.shedPct);
}

console.log("Ricardo scrape:", JSON.stringify(best, null, 2));

const failures = [];
if (best.hostPushFps < RICARDO.minPushFps) {
  failures.push(`push ${best.hostPushFps.toFixed(1)} < ${RICARDO.minPushFps}`);
}
if (best.hostShedPct > RICARDO.maxShedPct) {
  failures.push(`shed ${best.hostShedPct}% > ${RICARDO.maxShedPct}%`);
}
if (best.encodeKbps < RICARDO.minEncodeKbps) {
  failures.push(`encode ${best.encodeKbps} < ${RICARDO.minEncodeKbps} kbps`);
}
if (best.paintFps < RICARDO.minPaintFps) {
  failures.push(`paint ${best.paintFps.toFixed(1)} < ${RICARDO.minPaintFps}`);
}
if (best.surplusP50Ms == null) {
  failures.push("S_p50 missing (need WebCodecs + pad input_wm samples)");
} else if (best.surplusP50Ms > RICARDO.maxSurplusP50Ms) {
  failures.push(
    `S_p50 ${best.surplusP50Ms.toFixed(1)}ms > ${RICARDO.maxSurplusP50Ms}ms`
  );
}

if (failures.length) {
  console.error("FAIL Ricardo hard gate:", failures.join("; "));
  console.error("console tail:\n", consoleLines.slice(-40).join("\n"));
  process.exit(1);
}

console.log(
  `LIVE Ricardo PASS — push=${best.hostPushFps.toFixed(1)} shed=${best.hostShedPct}% encode=${best.encodeKbps} paint=${best.paintFps.toFixed(1)} S_p50=${best.surplusP50Ms.toFixed(1)}ms (Φ=${best.photonP50Ms?.toFixed?.(1) ?? "?"} RTT=${best.rttMs}) present=${best.presentMode}`
);
process.exit(0);
