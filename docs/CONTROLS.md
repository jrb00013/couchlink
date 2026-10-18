# Controls: PC keyboard/mouse ↔ Xbox ↔ PlayStation

Couchlink's wire protocol and CLPD button bits are always PlayStation-shaped
(see `BTN` in `web/src/clpd.ts` and `couchlink_proto::pad_frame::buttons`),
regardless of which physical pad — or none at all — a friend is actually
using. This table is the one reference for how a button on any of the three
maps onto the others, and what a keyboard+mouse player has bound to it by
default (`web/src/kbmBinds.ts` → `DEFAULT_KBM_BINDS`, remappable in-page via
the keybinds modal).

## Face / shoulder / stick buttons

| PlayStation | Xbox | Default PC key/mouse | Action |
|---|---|---|---|
| ✕ Cross | A | `Space` | Jump / confirm |
| ○ Circle | B | `F` | Cancel |
| □ Square | X | `Q` | — |
| △ Triangle | Y | `E` | — |
| L1 | LB | `Shift` (either) | — |
| R1 | RB | `R` | — |
| L2 | LT | Right mouse button | Aim |
| R2 | RT | Left mouse button | Shoot |
| L3 (left stick click) | Left stick click | `C` / middle mouse button | — |
| R3 (right stick click) | Right stick click | `V` | — |
| Create | View (Back) | `G` | — |
| Options | Menu (Start) | `Tab` | — |
| PS button | Guide button | — | Not exposed over CLPD |
| Touchpad click | — (no Xbox equivalent) | — | Not exposed over CLPD |

## Movement / D-Pad

| Input | Default PC key |
|---|---|
| Left stick up/down/left/right | `W` `S` `A` `D` (or arrow keys) |
| D-Pad up/down/left/right | `I` `K` `J` `L` (or numpad `8` `2` `4` `6`) |

## Where this is enforced in code

- **Wire format:** `web/src/clpd.ts` (`BTN` constants), `crates/proto/src/pad_frame.rs` (`buttons` module) — both PlayStation-named bits.
- **Keyboard+mouse defaults:** `web/src/kbmBinds.ts` (`DEFAULT_KBM_BINDS`).
- **Player-facing labels:** `web/src/kbmBinds.ts` (`KBM_ACTIONS`) — shown in the in-page rebind UI (`web/src/KeybindsModal.tsx`) as `"{PS name} / {Xbox name}"`, so a friend who only knows one layout can still follow along.
- **Host-side virtual pad emission:** `crates/pad/src/virtual_pad.rs` (Linux uinput), `crates/ds-vhid` (Windows ViGEm/WinUHid) — both translate the same PlayStation-shaped `PadFrame` into whatever the emulator's bound backend (XInput/DualShock4/DualSense) expects.
