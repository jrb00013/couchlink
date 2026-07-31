/// <reference types="vite/client" />

/** Chromium MediaStreamTrackProcessor (main-thread). */
interface MediaStreamTrackProcessorInit {
  track: MediaStreamTrack;
  maxBufferSize?: number;
}

declare var MediaStreamTrackProcessor: {
  prototype: MediaStreamTrackProcessor;
  new (init: MediaStreamTrackProcessorInit): MediaStreamTrackProcessor;
};

interface MediaStreamTrackProcessor {
  readonly readable: ReadableStream<VideoFrame>;
}
