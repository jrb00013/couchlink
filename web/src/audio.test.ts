import { describe, it, expect } from "vitest";
import { attachAudioTrack, pinAudioJitterBuffer } from "./audio";

describe("audio separate pipe", () => {
  it("attachAudioTrack sets srcObject and calls play", () => {
    let played = false;
    const el = {
      srcObject: null as unknown,
      play: () => {
        played = true;
        return Promise.resolve();
      },
      pause: () => {},
    } as unknown as HTMLAudioElement;
    const track = { id: "audio-1", muted: false } as unknown as MediaStreamTrack;
    // jsdom MediaStream may not exist, but attach just wraps it
    global.MediaStream = class {
      tracks: MediaStreamTrack[];
      constructor(tracks: MediaStreamTrack[]) {
        this.tracks = tracks;
      }
    } as unknown as typeof MediaStream;
    attachAudioTrack(track, el);
    expect(el.srcObject).not.toBeNull();
    expect(played).toBe(true);
  });

  it("pinAudioJitterBuffer tolerates missing fields", () => {
    const receiver: Record<string, unknown> = {};
    expect(() => pinAudioJitterBuffer(receiver as unknown as RTCRtpReceiver & { jitterBufferTarget?: number | null })).not.toThrow();
    const withFields = { jitterBufferTarget: 100, playoutDelayHint: 100 } as RTCRtpReceiver & { jitterBufferTarget: number | null; playoutDelayHint: number | null };
    pinAudioJitterBuffer(withFields);
    expect(withFields.jitterBufferTarget).toBe(0);
    expect(withFields.playoutDelayHint).toBe(0);
  });

  it("audio must not be on video DataChannel (rg check)", async () => {
    const fs = await import("fs");
    const content = fs.readFileSync("src/audio.ts", "utf-8");
    expect(content).not.toMatch(/video_dc/);
    expect(content).not.toMatch(/VIDEO_CHANNEL/);
    const player = fs.readFileSync("src/player.ts", "utf-8");
    // audio track handling must not call promoteWebcodecs or preferRtpPresent
    const audioSection = player.slice(player.indexOf('kind === "audio"'), player.indexOf('kind === "audio"') + 2000);
    expect(audioSection).not.toMatch(/promoteWebcodecs/);
    expect(audioSection).not.toMatch(/preferRtpPresent/);
    expect(audioSection).not.toMatch(/notifyPresentPath.*rtp/);
  });
});
