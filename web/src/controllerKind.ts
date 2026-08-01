/** Classify browser Gamepad.id into a viz family. */

export type ControllerKind = "xbox" | "dualsense" | "generic";

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
