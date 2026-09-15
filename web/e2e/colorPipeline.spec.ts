import { test, expect } from "@playwright/test";

// PR #53 regression: BT.601 -> BT.709 full-range fix. This complements the
// existing vitest unit tests (webCodecsCanvas.test.ts) which check pure
// logic — this instead drives the ACTUAL browser VideoDecoder.configure()
// call path in webCodecsCanvas.ts, in a real Chromium page, to prove the
// color tagging that reaches the decoder is what the fix intended.
test("VideoDecoder.configure is called with bt709 full-range colorSpace on the real paint path", async ({
  page,
}) => {
  // Spy on VideoDecoder.prototype.configure before any app code runs, so the
  // real configure() call made deep inside WebCodecsCanvasView.push() is
  // captured without needing a live H.264 decode to actually succeed.
  await page.addInitScript(() => {
    (window as unknown as { __configureCalls: VideoDecoderConfig[] }).__configureCalls = [];
    const proto = VideoDecoder.prototype as unknown as {
      configure(this: VideoDecoder, config: VideoDecoderConfig): void;
    };
    const original = proto.configure;
    proto.configure = function (config: VideoDecoderConfig) {
      (window as unknown as { __configureCalls: VideoDecoderConfig[] }).__configureCalls.push(
        config
      );
      return original.call(this, config);
    };
  });

  await page.goto("/");

  const result = await page.evaluate(async () => {
    const mod = await import("/src/webCodecsCanvas.ts");

    const canvas = document.createElement("canvas");
    document.body.appendChild(canvas);
    const view = new mod.WebCodecsCanvasView(canvas);
    const started = view.start();

    // Minimal (not bit-valid, framing only — same fixture shape as
    // src/h264Avc.test.ts) SPS/PPS/IDR access unit, annex-B start-coded.
    const sps = new Uint8Array([0x67, 0x42, 0xe0, 0x1f, 1, 2, 3]);
    const pps = new Uint8Array([0x68, 0xce, 0x3c, 0x80]);
    const idr = new Uint8Array([0x65, 0x88, 0x84, 0x00]);
    const parts: number[] = [];
    for (const nal of [sps, pps, idr]) parts.push(0, 0, 0, 1, ...nal);
    const annexB = new Uint8Array(parts);

    view.push({
      seq: 1,
      width: 16,
      height: 16,
      keyframe: true,
      annexB,
      stampUs: 0,
      inputWm: 0,
    });

    return {
      started,
      configureCalls: (window as unknown as { __configureCalls: VideoDecoderConfig[] })
        .__configureCalls,
    };
  });

  expect(result.started, "WebCodecsCanvasView.start() must succeed in Chromium").toBe(true);
  expect(result.configureCalls.length).toBeGreaterThanOrEqual(1);
  expect(result.configureCalls[0].colorSpace).toEqual({
    primaries: "bt709",
    transfer: "bt709",
    matrix: "bt709",
    fullRange: true,
  });
});
