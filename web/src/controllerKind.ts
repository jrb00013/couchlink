/** Classify browser Gamepad.id into a viz family. */

export type ControllerKind = "xbox" | "dualsense" | "generic";

/** ViGEm / companion virtual pads that leak into the host browser's Gamepad API. */
export function isVirtualGamepadId(id: string): boolean {
  const s = id.toLowerCase();
  return (
    s.includes("vigem") ||
    s.includes("nefarius") ||
    s.includes("vhid") ||
    s.includes("couchlink") ||
    s.includes("virtual gamepad")
  );
}

/**
 * Two or more identical "Xbox 360 Controller" entries are the host's
 * ViGEm bus (one pad per remote seat), not four friends each holding a 360.
 */
export function isViGEmXbox360Cluster(ids: readonly string[]): boolean {
  const x360 = ids.filter((id) => /xbox 360 controller/i.test(id));
  if (x360.length < 2) return false;
  return x360.every((id) => id === x360[0]);
}

/** Gamepads this browser should treat as a real controller in the player's hands. */
export function selectPhysicalGamepads<T extends { id: string }>(pads: readonly T[]): T[] {
  const listed = pads.filter((p) => !!p && p.id);
  const ids = listed.map((p) => p.id);
  const dropCluster = isViGEmXbox360Cluster(ids);
  return listed.filter((p) => {
    if (isVirtualGamepadId(p.id)) return false;
    if (dropCluster && /xbox 360 controller/i.test(p.id)) return false;
    return true;
  });
}

export function controllerKind(id: string): ControllerKind {
  const s = id.toLowerCase();
  if (
    s.includes("xbox") ||
    s.includes("xinput") ||
    s.includes("045e") || // Microsoft VID
    s.includes("microsoft")
  ) {
    return "xbox";
  }
  if (
    s.includes("dualsense") ||
    s.includes("dualshock") ||
    s.includes("wireless controller") || // common PS4/PS5 browser id
    s.includes("054c") || // Sony VID
    s.includes("playstation") ||
    s.includes("sony")
  ) {
    return "dualsense";
  }
  return "generic";
}
