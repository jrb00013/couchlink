import { clog, cwarn } from "./log";

/**
 * Browser audio pipe — separate from WebCodecs/RTP video.
 * Attaches an Opus RTP track to a hidden <audio> element. Never touches
 * CLVD/SCTP, never delays video. One <audio> per peer; `ontrack` with
 * `kind === "audio"` is the only trigger.
 */

export function attachAudioTrack(track: MediaStreamTrack, el: HTMLAudioElement): void {
  try {
    // Pin jitter buffer if exposed (same as video path, but for audio).
    const receiver = (track as unknown as { _receiver?: unknown });
    void receiver;
    const anyTrack = track as unknown as { jitterBufferTarget?: number | null; playoutDelayHint?: number | null };
    // Hint is on the receiver, not the track — caller may pin via RTCRtpReceiver externally.
    // Here we just attach.

    el.srcObject = new MediaStream([track]);
    // Ensure autoplay works after user gesture; also handle promise.
    const p = el.play();
    if (p && typeof p.catch === "function") {
      p.catch((e) => {
        cwarn("audio autoplay blocked — need user gesture", String(e));
      });
    }
    clog("audio track attached", { id: track.id, muted: (track as MediaStreamTrack & { muted?: boolean }).muted });
  } catch (e) {
    cwarn("attachAudioTrack failed", String(e));
  }
}

export function detachAudio(el: HTMLAudioElement): void {
  try {
    const ms = el.srcObject as MediaStream | null;
    if (ms) {
      ms.getTracks().forEach((t) => {
        try {
          t.stop();
        } catch {
          /* ignore */
        }
      });
    }
    el.srcObject = null;
    el.pause();
  } catch {
    /* ignore */
  }
}

/** Pin audio receiver jitter buffer to 0 when available (Chromium). */
export function pinAudioJitterBuffer(receiver: RTCRtpReceiver & { jitterBufferTarget?: number | null; playoutDelayHint?: number | null }): void {
  try {
    if ("jitterBufferTarget" in receiver) receiver.jitterBufferTarget = 0;
    if ("playoutDelayHint" in receiver) receiver.playoutDelayHint = 0;
  } catch {
    /* older Chromium */
  }
}
