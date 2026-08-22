import { useRef } from "react";
import type {
  ConnectionState,
  PlayerCallbacks,
  PlayerTelemetry,
  PresentPath,
} from "./player";
import type { VideoAccessUnit } from "./clvd";

/** Stable WebRTC player callbacks — safe across React re-renders. */
export function usePlayerCallbacks(handlers: {
  onState: (s: ConnectionState, detail?: string) => void;
  onVideo: (stream: MediaStream) => void;
  onVideoAccessUnit?: (au: VideoAccessUnit) => void;
  onPresentPath?: (path: PresentPath, detail?: string) => void;
  onStreamInfo?: PlayerCallbacks["onStreamInfo"];
  onHostStats?: PlayerCallbacks["onHostStats"];
  onPadStats?: PlayerCallbacks["onPadStats"];
  onRegistered?: PlayerCallbacks["onRegistered"];
  onPlayersStatus?: PlayerCallbacks["onPlayersStatus"];
  onPlayerPadInfo?: PlayerCallbacks["onPlayerPadInfo"];
  onPlayerLeft?: PlayerCallbacks["onPlayerLeft"];
  onTelemetry?: (t: PlayerTelemetry) => void;
}): PlayerCallbacks {
  const handlersRef = useRef(handlers);
  handlersRef.current = handlers;

  const stableRef = useRef<PlayerCallbacks | null>(null);
  if (!stableRef.current) {
    stableRef.current = {
      onState: (s, d) => handlersRef.current.onState(s, d),
      onVideo: (stream) => handlersRef.current.onVideo(stream),
      onVideoAccessUnit: (au) => handlersRef.current.onVideoAccessUnit?.(au),
      onPresentPath: (path, detail) =>
        handlersRef.current.onPresentPath?.(path, detail),
      onStreamInfo: (info) => handlersRef.current.onStreamInfo?.(info),
      onHostStats: (stats) => handlersRef.current.onHostStats?.(stats),
      onPadStats: (hz, name) => handlersRef.current.onPadStats?.(hz, name),
      onRegistered: (slot) => handlersRef.current.onRegistered?.(slot),
      onPlayersStatus: (occupied, max) =>
        handlersRef.current.onPlayersStatus?.(occupied, max),
      onPlayerPadInfo: (slot, kind, id) =>
        handlersRef.current.onPlayerPadInfo?.(slot, kind, id),
      onPlayerLeft: (slot) => handlersRef.current.onPlayerLeft?.(slot),
      onTelemetry: (t) => handlersRef.current.onTelemetry?.(t),
    };
  }
  return stableRef.current;
}
