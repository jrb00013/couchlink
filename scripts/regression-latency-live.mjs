#!/usr/bin/env node
/**
 * Live Ricardo + self-beat probe against a running couchlink host.
 *
 * **Authority for S_p50 is real Chrome** — use HOST_ONLY=1 + paste a scrape from
 * `window.__couchlinkRicardo()` (see scripts/joel-live-gate.sh). Playwright is
 * optional smoke only (PLAYWRIGHT=1).
 *
 * Gates:
 *   Ricardo floor:  push ≥ 74 · shed ≤ 3% · encode ≥ 5000 · paint ≥ 74 · S_p50 ≤ 45
 *   Self-beat bars: push ≥ 90 · shed ≤ 1% · encode ≥ 5000 · paint ≥ 100 · S_p50 ≤ 5
 *
 * Usage:
 *   HOST_ONLY=1 HOST_LOG=/tmp/couchlink-stack.log node scripts/regression-latency-live.mjs
 *   CLIENT_SCRAPE=/tmp/ricardo.json HOST_LOG=… node scripts/regression-latency-live.mjs
 *   PLAYWRIGHT=1 JOIN_URL='…' HOST_LOG=… node scripts/regression-latency-live.mjs
 */
import { createRequire } from "node:module";
import path from "node:path";
import fs from "node:fs";
import { fileURLToPath } from "node:url";
import { execSync } from "node:child_process";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(__dirname, "..");
const webDir = path.join(root, "web");

const hostOnly =
  process.env.HOST_ONLY === "1" || process.env.HOST_ONLY === "true";
const usePlaywright =
  process.env.PLAYWRIGHT === "1" || process.env.PLAYWRIGHT === "true";
const joinUrl = process.argv[2] || process.env.JOIN_URL || "";
const hostLog = process.env.HOST_LOG || "";
const clientScrapePath = process.env.CLIENT_SCRAPE || "";

/** Hard Ricardo gate — mirrors crates/host/src/latency_live_sim/ricardo_gate.rs */
export const RICARDO = {
  minPushFps: 74,
  maxShedPct: 3,
  minEncodeKbps: 5000,
  minPaintFps: 74,
  maxSurplusP50Ms: 45,
};

/** Beat-self bars — clear margin over frozen self baseline (74.8/84/7.4). */
export const SELF = {
  minPushFps: 90,
  maxShedPct: 1,
  minEncodeKbps: 5000,
  minPaintFps: 100,
  maxSurplusP50Ms: 5,
};

const beatSelf =
  process.env.BEAT_SELF !== "0" && process.env.BEAT_SELF !== "false";
const GATE = beatSelf ? SELF : RICARDO;

export function parseHostStreamingWindows(logPath, n = 8, afterMarker = null) {
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
    if (pushFps < 10) continue;
    out.push({
      pushFps,
      shedPct: drop ? Number(drop[1]) : 99,
      raw: line.trim(),
    });
  }
  return out;
}

function sleep(ms) {
  return new Promise((r) => setTimeout(r, ms));
}

function loadClientScrape() {
  if (!clientScrapePath) return null;
  try {
    const raw = fs.readFileSync(clientScrapePath, "utf8");
    return JSON.parse(raw);
  } catch (e) {
    console.error("CLIENT_SCRAPE parse failed:", e.message || e);
    process.exit(2);
  }
}

/** Normalize __couchlinkRicardo() hook output into gate scrape shape. */
export function scrapeFromHook(snap) {
  if (!snap) return null;
  const paint = snap.present?.fps || 0;
  const encode = snap.hostStats?.target_bitrate_kbps ?? 0;
  const surplus =
    snap.inputPhoton?.surplusP50Ms ?? snap.present?.surplusP50Ms ?? null;
  const photon =
    snap.inputPhoton?.photonP50Ms ?? snap.present?.photonP50Ms ?? null;
  return {
    presentMode: snap.presentMode || "—",
    paintFps: paint || 0,
    encodeKbps: encode || 0,
    hostPushFps: snap.hostStats?.fps ?? 0,
    hostShedPct: snap.hostStats?.drop_pct ?? 100,
    surplusP50Ms: surplus,
    photonP50Ms: photon,
    rttMs: snap.rttMs || 0,
    sampleCount: snap.inputPhoton?.sampleCount ?? 0,
    watermarkActive: !!snap.inputPhoton?.watermarkActive,
  };
}

export function mergeBest(hostBest, clientBest) {
  if (!clientBest) return { ...hostBest };
  return {
    presentMode: clientBest.presentMode || hostBest.presentMode,
    paintFps: Math.max(hostBest.paintFps, clientBest.paintFps || 0),
    encodeKbps: Math.max(hostBest.encodeKbps, clientBest.encodeKbps || 0),
    hostPushFps: hostBest.hostPushFps || clientBest.hostPushFps || 0,
    hostShedPct: hostBest.hostShedPct ?? clientBest.hostShedPct ?? 100,
    surplusP50Ms:
      clientBest.surplusP50Ms != null
        ? clientBest.surplusP50Ms
        : hostBest.surplusP50Ms,
    photonP50Ms:
      clientBest.photonP50Ms != null
        ? clientBest.photonP50Ms
        : hostBest.photonP50Ms,
    rttMs: clientBest.rttMs || hostBest.rttMs,
    sampleCount: Math.max(hostBest.sampleCount, clientBest.sampleCount || 0),
    watermarkActive: hostBest.watermarkActive || clientBest.watermarkActive,
  };
}

export function scoreGate(best, gate, { hostLog, scoreWindows, requireClient }) {
  const failures = [];
  if (hostLog && !scoreWindows.length) {
    failures.push(
      "no host streaming windows after probe start (host log stale or dead)"
    );
  }
  if (requireClient) {
    if (best.presentMode !== "webcodecs") {
      failures.push(
        `presentMode=${best.presentMode} (need webcodecs for honest S_p50)`
      );
    }
    if (best.encodeKbps < gate.minEncodeKbps) {
      failures.push(`encode ${best.encodeKbps} < ${gate.minEncodeKbps} kbps`);
    }
    if (best.paintFps < gate.minPaintFps) {
      failures.push(`paint ${best.paintFps.toFixed(1)} < ${gate.minPaintFps}`);
    }
    if (!best.watermarkActive || best.sampleCount < 16) {
      failures.push(
        `input_wm samples=${best.sampleCount} active=${best.watermarkActive} (need ≥16)`
      );
    }
    if (best.surplusP50Ms == null) {
      failures.push("S_p50 missing (need WebCodecs + CLVD v4 input_wm in Chrome)");
    } else if (best.surplusP50Ms > gate.maxSurplusP50Ms) {
      failures.push(
        `S_p50 ${best.surplusP50Ms.toFixed(1)}ms > ${gate.maxSurplusP50Ms}ms`
      );
    }
  }
  if (best.hostPushFps < gate.minPushFps) {
    failures.push(`push ${best.hostPushFps.toFixed(1)} < ${gate.minPushFps}`);
  }
  if (best.hostShedPct > gate.maxShedPct) {
    failures.push(`shed ${best.hostShedPct}% > ${gate.maxShedPct}%`);
  }
  return failures;
}

function applyHostWindows(best, hostLog, probeMarker) {
  const windows = parseHostStreamingWindows(hostLog, 8, probeMarker);
  const steady = windows.filter((w) => w.pushFps >= 60);
  const scoreWindows =
    steady.length >= 3
      ? steady.slice(-3)
      : steady.length
        ? steady
        : windows.slice(-3);
  if (scoreWindows.length) {
    for (const w of scoreWindows) console.log("host log streaming:", w.raw);
    best.hostPushFps = Math.min(...scoreWindows.map((w) => w.pushFps));
    best.hostShedPct = Math.max(...scoreWindows.map((w) => w.shedPct));
  }
  return scoreWindows;
}

async function runHostOnly(probeMarker) {
  const waitSec = Number(process.env.HOST_WAIT_SEC || 25);
  console.log(`HOST_ONLY: waiting ${waitSec}s for post-marker streaming windows…`);
  for (let i = 0; i < waitSec; i++) {
    const wins = parseHostStreamingWindows(hostLog, 8, probeMarker);
    if (wins.filter((w) => w.pushFps >= 60).length >= 1) break;
    await sleep(1000);
  }
  const best = {
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
  const scoreWindows = applyHostWindows(best, hostLog, probeMarker);
  return { best, scoreWindows, consoleLines: [] };
}

async function runPlaywright(probeMarker) {
  if (!joinUrl) {
    console.error("PLAYWRIGHT=1 requires JOIN_URL");
    process.exit(2);
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
      execSync("npx playwright install chromium", {
        cwd: webDir,
        stdio: "inherit",
      });
      return require(path.join(webDir, "node_modules/playwright"));
    }
  }
  const { chromium } = loadPlaywright();
  const launchOpts = {
    headless: true,
    args: [
      "--autoplay-policy=no-user-gesture-required",
      "--use-fake-device-for-media-stream",
    ],
  };
  let browser;
  try {
    browser = await chromium.launch({ ...launchOpts, channel: "chrome" });
    console.log("browser: chrome channel (smoke only — not S_p50 authority)");
  } catch {
    browser = await chromium.launch(launchOpts);
    console.log("browser: bundled chromium (smoke only)");
  }
  const page = await browser.newPage();
  await page.addInitScript(() => {
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
      gp.timestamp = performance.now();
      return [gp, null, null, null];
    };
  });
  const consoleLines = [];
  page.on("console", (msg) => {
    const t = msg.text();
    if (/couchlink|present|webcodecs|photon|surplus/i.test(t)) {
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
  for (let i = 0; i < 50; i++) {
    await page.waitForTimeout(1000);
    const snap = await page.evaluate(() => {
      const hook = window.__couchlinkRicardo;
      return typeof hook === "function" ? hook() : null;
    });
    const scraped = scrapeFromHook(snap);
    if (scraped) {
      best = mergeBest(best, scraped);
    }
    const hostWindows = parseHostStreamingWindows(hostLog, 8, probeMarker);
    if (
      best.watermarkActive &&
      best.sampleCount >= 16 &&
      hostWindows.filter((w) => w.pushFps >= 60).length >= 1
    ) {
      break;
    }
  }
  await browser.close();
  const scoreWindows = applyHostWindows(best, hostLog, probeMarker);
  return { best, scoreWindows, consoleLines };
}

// --- main ---
if (!hostOnly && !usePlaywright && !clientScrapePath) {
  console.error(
    "usage: HOST_ONLY=1 HOST_LOG=… node scripts/regression-latency-live.mjs\n" +
      "   or: CLIENT_SCRAPE=/tmp/ricardo.json HOST_LOG=… node …\n" +
      "   or: PLAYWRIGHT=1 JOIN_URL=… HOST_LOG=… node … (smoke only)"
  );
  process.exit(2);
}

const probeMarker = `[ricardo-probe] start ${Date.now()}`;
if (hostLog) {
  try {
    fs.appendFileSync(hostLog, `\n${probeMarker}\n`);
  } catch (e) {
    console.warn("could not append probe marker to HOST_LOG:", e.message || e);
  }
}

let best;
let scoreWindows;
let consoleLines = [];

if (usePlaywright) {
  ({ best, scoreWindows, consoleLines } = await runPlaywright(probeMarker));
} else {
  ({ best, scoreWindows, consoleLines } = await runHostOnly(probeMarker));
}

const clientRaw = loadClientScrape();
if (clientRaw) {
  const fromFile =
    clientRaw.presentMode != null
      ? clientRaw
      : scrapeFromHook(clientRaw);
  if (fromFile) {
    console.log("CLIENT_SCRAPE merged");
    best = mergeBest(best, fromFile);
  }
}

console.log("Ricardo scrape:", JSON.stringify(best, null, 2));

const requireClient = !!clientScrapePath || usePlaywright;
const failures = scoreGate(best, GATE, {
  hostLog,
  scoreWindows,
  requireClient,
});

if (failures.length) {
  console.error(
    `FAIL ${beatSelf ? "beat-self" : "Ricardo"} hard gate:`,
    failures.join("; ")
  );
  if (!requireClient) {
    console.error(
      "\nClient axes skipped (HOST_ONLY). In real Chrome DevTools console:\n" +
        "  copy(JSON.stringify(window.__couchlinkRicardo()))\n" +
        "Save to /tmp/ricardo.json then:\n" +
        "  CLIENT_SCRAPE=/tmp/ricardo.json HOST_LOG=… node scripts/regression-latency-live.mjs"
    );
  }
  if (consoleLines.length) {
    console.error("console tail:\n", consoleLines.slice(-50).join("\n"));
  }
  process.exit(1);
}

const mode = hostOnly && !clientScrapePath ? "HOST" : "FULL";
console.log(
  `LIVE ${beatSelf ? "SELF" : "Ricardo"} ${mode} PASS — push≥${best.hostPushFps.toFixed(1)} shed≤${best.hostShedPct}% encode=${best.encodeKbps} paint=${best.paintFps.toFixed(1)} S_p50=${best.surplusP50Ms?.toFixed?.(1) ?? "—"}ms present=${best.presentMode} samples=${best.sampleCount}`
);
process.exit(0);
