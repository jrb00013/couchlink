import { useRef } from "react";
import type { ConnectionState, PlayerCallbacks } from "./player";

/** Stable WebRTC player callbacks — safe across React re-renders. */
export function usePlayerCallbacks(handlers: {
  onState: (s: ConnectionState, detail?: string) => void;
  onVideo: (stream: MediaStream) => void;
  onStreamInfo?: PlayerCallbacks["onStreamInfo"];
  onPadStats?: PlayerCallbacks["onPadStats"];
}): PlayerCallbacks {
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;

  const stableRef = useRef<PlayerCallbacks | null>(null);
  if (!stableRef.current) {
    stableRef.current = {
      onState: (s, d) => handlersRef.current.onState(s, d),
      onVideo: (stream) => handlersRef.current.onVideo(stream),
      onStreamInfo: (info) => handlersRef.current.onStreamInfo?.(info),
      onPadStats: (hz, name) => handlersRef.current.onPadStats?.(hz, name),
    };
  }
  return stableRef.current;
}
