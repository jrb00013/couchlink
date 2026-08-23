//! End-to-end controller testers: recognition → simulated HID → PadFrame → CLPD
//! → host decode. Covers every supported Xbox SKU and DualSense USB/BT without
//! needing real hardware or `/dev/uinput`.

#[cfg(test)]
mod tests {
    use bytes::BytesMut;
    use couchlink_proto::pad_frame::{buttons, PAD_FRAME_LEN_V2};
    use couchlink_proto::PadFrame;

    use crate::dualsense::{PID_DUALSENSE, PID_DUALSENSE_EDGE, PRODUCT_NAME, SONY_VID};
    use crate::recognize::{
        classify, is_native_supported, is_supported_dualsense, is_supported_steam_controller,
        is_supported_switch, is_supported_xbox, parse_hid_id_line, product_label,
        ControllerFamily, XboxVariant, DUALSHOCK4_PIDS, PID_DUALSHOCK4_V2,
    };
    use crate::sim::{
        decode_clpd, dualsense_bt_neutral_report, dualsense_usb_neutral_report,
        dualsense_usb_press, dualsense_usb_with_sticks, encode_clpd, simulate_dualsense_frame,
        simulate_steam_frame, simulate_switch_frame, simulate_xbox_frame, steam_neutral_report,
        steam_press, switch_neutral_report, switch_press, xbox_neutral_report, xbox_press,
        xbox_with_sticks, SimButton, DUALSENSE_ONLY_BUTTONS, SHARED_BUTTONS,
    };
    use crate::steam_controller::{KNOWN_PIDS as STEAM_PIDS, VALVE_VID};
    use crate::switch::{KNOWN_PIDS as SWITCH_PIDS, NINTENDO_VID, PRODUCT_NAME as SWITCH_NAME};
    use crate::virtual_pad::{VirtualPad, VirtualPadConfig};
    use crate::xbox::MICROSOFT_VID;

    // ── Recognition (client would accept these hidraw nodes) ───────────────

    #[test]
    fn xbox_tester_recognizes_every_supported_sku() {
        for v in XboxVariant::ALL {
            let hid = format!("HID_ID=0003:0000{:04X}:0000{:04X}", MICROSOFT_VID, v.pid());
            let (bus, vid, pid) = parse_hid_id_line(&hid).unwrap();
            assert_eq!(bus, 0x0003);
            assert!(
                is_supported_xbox(vid, pid),
                "client must recognize {}",
                v.label()
            );
            assert_eq!(classify(vid, pid), ControllerFamily::Xbox);
            assert_eq!(product_label(vid, pid), Some(v.label()));
        }
    }

    #[test]
    fn dualsense_tester_recognizes_usb_and_edge() {
        for pid in [PID_DUALSENSE, PID_DUALSENSE_EDGE] {
            let hid = format!("HID_ID=0003:0000{:04X}:0000{:04X}", SONY_VID, pid);
            let (_, vid, p) = parse_hid_id_line(&hid).unwrap();
            assert!(is_supported_dualsense(vid, p));
            assert!(is_native_supported(vid, p));
            assert_eq!(classify(vid, p), ControllerFamily::DualSense);
        }
        // Bluetooth bus still recognized by VID/PID
        let hid = format!("HID_ID=0005:0000{:04X}:0000{:04X}", SONY_VID, PID_DUALSENSE);
        let (bus, vid, pid) = parse_hid_id_line(&hid).unwrap();
        assert_eq!(bus, 0x0005);
        assert!(is_supported_dualsense(vid, pid));
    }

    #[test]
    fn ps4_dualshock4_native_hidraw_and_labeled() {
        for &pid in DUALSHOCK4_PIDS {
            assert!(is_native_supported(SONY_VID, pid));
            assert_eq!(classify(SONY_VID, pid), ControllerFamily::DualShock4);
            assert_eq!(product_label(SONY_VID, pid), Some("DualShock 4"));
        }
        assert_eq!(
            classify(SONY_VID, PID_DUALSHOCK4_V2),
            ControllerFamily::DualShock4
        );
        let mut raw = [0u8; 32];
        raw[0] = 0x01;
        raw[5] = 0x20; // Cross
        let f = crate::parse_ds4_input_report(&raw).unwrap();
        assert!(f.buttons & buttons::CROSS != 0);
        let encoded = encode_clpd(&f);
        let back = decode_clpd(&encoded).unwrap();
        assert_eq!(back.buttons & buttons::CROSS, buttons::CROSS);
    }

    // ── Client: simulated Xbox input ───────────────────────────────────────

    #[test]
    fn xbox_tester_neutral_and_full_button_matrix() {
        let n = simulate_xbox_frame(&xbox_neutral_report()).unwrap();
        assert_eq!(n.lx, 128);
        assert_eq!(n.ly, 128);
        assert_eq!(n.rx, 128);
        assert_eq!(n.ry, 128);
        assert_eq!(n.buttons, 0);

        for &btn in SHARED_BUTTONS {
            let f = simulate_xbox_frame(&xbox_press(btn)).unwrap();
            assert!(
                f.buttons & btn.pad_bit() != 0,
                "Xbox sim missing {:?}",
                btn
            );
        }
        // Xbox has no Touch/Mute in our HID layout
        for &btn in DUALSENSE_ONLY_BUTTONS {
            let f = simulate_xbox_frame(&xbox_press(btn)).unwrap();
            assert_eq!(f.buttons & btn.pad_bit(), 0);
        }
    }

    #[test]
    fn xbox_tester_sticks_and_face_remap_a_to_cross() {
        let f = simulate_xbox_frame(&xbox_with_sticks(i16::MAX, 0, i16::MIN, 0)).unwrap();
        assert!(f.lx > 200, "max LX → high u8, got {}", f.lx);
        assert_eq!(f.ly, 128);
        assert!(f.rx < 60, "min RX → low u8, got {}", f.rx);

        // A (bottom) must land on CROSS so DualSense-shaped host injection is correct
        let a = simulate_xbox_frame(&xbox_press(SimButton::Cross)).unwrap();
        assert_eq!(a.buttons & buttons::CROSS, buttons::CROSS);
        assert_eq!(a.buttons & buttons::CIRCLE, 0);
    }

    #[test]
    fn each_xbox_sku_produces_identical_parsed_input() {
        // Recognition differs by PID; parse path is shared — pressing A must
        // yield the same PadFrame for every supported Xbox SKU.
        let raw = xbox_press(SimButton::Cross);
        let expected = simulate_xbox_frame(&raw).unwrap();
        for v in XboxVariant::ALL {
            assert!(is_supported_xbox(MICROSOFT_VID, v.pid()));
            let got = simulate_xbox_frame(&raw).unwrap();
            assert_eq!(got.buttons, expected.buttons, "{}", v.label());
            assert_eq!(got.lx, expected.lx);
        }
    }

    // ── Client: simulated DualSense / PlayStation input ────────────────────

    #[test]
    fn switch_tester_recognizes_pro_and_joycons() {
        for &pid in SWITCH_PIDS {
            let hid = format!("HID_ID=0003:0000{:04X}:0000{:04X}", NINTENDO_VID, pid);
            let (_, vid, p) = parse_hid_id_line(&hid).unwrap();
            assert!(is_supported_switch(vid, p), "PID {pid:04X}");
            assert!(is_native_supported(vid, p));
            assert_eq!(classify(vid, p), ControllerFamily::Switch);
            assert!(product_label(vid, p).is_some());
        }
        assert_eq!(classify(NINTENDO_VID, SWITCH_PIDS[0]), ControllerFamily::Switch);
        assert_eq!(product_label(NINTENDO_VID, SWITCH_PIDS[0]), Some(SWITCH_NAME));
    }

    #[test]
    fn switch_tester_button_matrix_and_sticks() {
        let n = simulate_switch_frame(&switch_neutral_report()).unwrap();
        assert_eq!(n.lx, 128);
        assert_eq!(n.ly, 128);
        assert_eq!(n.rx, 128);
        assert_eq!(n.ry, 128);
        assert_eq!(n.buttons, 0);

        for &btn in SHARED_BUTTONS {
            let f = simulate_switch_frame(&switch_press(btn)).unwrap();
            assert!(
                f.buttons & btn.pad_bit() != 0,
                "Switch sim missing {:?}",
                btn
            );
        }
        // Capture has no DualSense equivalent.
        for &btn in DUALSENSE_ONLY_BUTTONS {
            let f = simulate_switch_frame(&switch_press(btn)).unwrap();
            assert_eq!(f.buttons & btn.pad_bit(), 0);
        }
    }

    #[test]
    fn switch_tester_face_remap_by_position() {
        // A (bottom) → CROSS; Y (top) → TRIANGLE
        let a = simulate_switch_frame(&switch_press(SimButton::Cross)).unwrap();
        assert_eq!(a.buttons & buttons::CROSS, buttons::CROSS);
        assert_eq!(a.buttons & buttons::CIRCLE, 0);
        let y = simulate_switch_frame(&switch_press(SimButton::Triangle)).unwrap();
        assert_eq!(y.buttons & buttons::TRIANGLE, buttons::TRIANGLE);
    }

    #[test]
    fn switch_input_survives_client_to_host_clpd() {
        let client = simulate_switch_frame(&switch_press(SimButton::Ps)).unwrap();
        let host = roundtrip_client_to_host(client);
        assert!(host.buttons & buttons::PS != 0);
    }

    // ── Client: simulated Steam Controller input ──────────────────────────

    #[test]
    fn steam_tester_recognizes_wired_wireless_and_bt() {
        for &pid in STEAM_PIDS {
            let hid = format!("HID_ID=0003:0000{:04X}:0000{:04X}", VALVE_VID, pid);
            let (_, vid, p) = parse_hid_id_line(&hid).unwrap();
            assert!(is_supported_steam_controller(vid, p), "PID {pid:04X}");
            assert!(is_native_supported(vid, p));
            assert_eq!(classify(vid, p), ControllerFamily::SteamController);
            assert!(product_label(vid, p).is_some());
        }
    }

    #[test]
    fn steam_tester_button_matrix() {
        let n = simulate_steam_frame(&steam_neutral_report()).unwrap();
        assert_eq!(n.lx, 128);
        assert_eq!(n.ly, 128);
        assert_eq!(n.rx, 128);
        assert_eq!(n.ry, 128);
        assert_eq!(n.buttons, 0);

        for &btn in SHARED_BUTTONS {
            let f = simulate_steam_frame(&steam_press(btn)).unwrap();
            assert!(
                f.buttons & btn.pad_bit() != 0,
                "Steam sim missing {:?}",
                btn
            );
        }
        // Grip buttons have no DualSense equivalent.
        for &btn in DUALSENSE_ONLY_BUTTONS {
            let f = simulate_steam_frame(&steam_press(btn)).unwrap();
            assert_eq!(f.buttons & btn.pad_bit(), 0);
        }
    }

    #[test]
    fn steam_tester_face_remap_by_position() {
        let a = simulate_steam_frame(&steam_press(SimButton::Cross)).unwrap();
        assert_eq!(a.buttons & buttons::CROSS, buttons::CROSS);
        let b = simulate_steam_frame(&steam_press(SimButton::Circle)).unwrap();
        assert_eq!(b.buttons & buttons::CIRCLE, buttons::CIRCLE);
    }

    #[test]
    fn steam_input_survives_client_to_host_clpd() {
        let client = simulate_steam_frame(&steam_press(SimButton::R3)).unwrap();
        let host = roundtrip_client_to_host(client);
        assert!(host.buttons & buttons::R3 != 0);
    }

    // ── Client: simulated DualSense / PlayStation input ────────────────────

    #[test]
    fn dualsense_tester_usb_button_matrix() {
        let n = simulate_dualsense_frame(&dualsense_usb_neutral_report()).unwrap();
        assert_eq!(n.lx, 128);
        assert_eq!(n.buttons, 0);

        for &btn in SHARED_BUTTONS.iter().chain(DUALSENSE_ONLY_BUTTONS.iter()) {
            let f = simulate_dualsense_frame(&dualsense_usb_press(btn)).unwrap();
            assert!(
                f.buttons & btn.pad_bit() != 0,
                "DualSense USB sim missing {:?}",
                btn
            );
        }
    }

    #[test]
    fn dualsense_tester_bluetooth_neutral_and_cross() {
        let n = simulate_dualsense_frame(&dualsense_bt_neutral_report()).unwrap();
        assert_eq!(n.lx, 128);
        assert_eq!(n.ly, 128);

        let mut raw = dualsense_bt_neutral_report();
        // body after 0x31, 0x01 → same offsets as USB body, so CROSS at body[7]
        raw[9] = (raw[9] & 0xF0) | 0x08; // keep released dpad
        raw[9] |= 0x20; // CROSS
        let f = simulate_dualsense_frame(&raw).unwrap();
        assert!(f.buttons & buttons::CROSS != 0);
    }

    #[test]
    fn dualsense_tester_sticks() {
        let f = simulate_dualsense_frame(&dualsense_usb_with_sticks(0, 255, 64, 192)).unwrap();
        assert_eq!(f.lx, 0);
        assert_eq!(f.ly, 255);
        assert_eq!(f.rx, 64);
        assert_eq!(f.ry, 192);
    }

    // ── Client → host wire path ────────────────────────────────────────────

    fn roundtrip_client_to_host(frame: PadFrame) -> PadFrame {
        let mut framed = frame;
        framed.seq = 42;
        let bytes = encode_clpd(&framed);
        decode_clpd(&bytes).expect("host must decode client CLPD")
    }

    #[test]
    fn xbox_input_survives_client_to_host_clpd() {
        let client = simulate_xbox_frame(&xbox_press(SimButton::Triangle)).unwrap();
        let host = roundtrip_client_to_host(client);
        assert_eq!(host.seq, 42);
        assert!(host.buttons & buttons::TRIANGLE != 0);
        assert_eq!(host.lx, 128);
    }

    #[test]
    fn dualsense_input_survives_client_to_host_clpd() {
        let client = simulate_dualsense_frame(&dualsense_usb_press(SimButton::Square)).unwrap();
        let host = roundtrip_client_to_host(client);
        assert!(host.buttons & buttons::SQUARE != 0);
    }

    #[test]
    fn host_virtual_pad_identity_is_bluetooth_dualsense() {
        let cfg = VirtualPadConfig::default();
        assert_eq!(cfg.vendor, SONY_VID);
        assert_eq!(cfg.product, PID_DUALSENSE);
        assert_eq!(cfg.name, PRODUCT_NAME);
        assert!(cfg.as_bluetooth);
        // Host always injects DualSense — Xbox client input is already remapped.
        assert!(!is_supported_xbox(cfg.vendor, cfg.product));
        assert!(is_supported_dualsense(cfg.vendor, cfg.product));
    }

    #[test]
    fn host_noop_applies_simulated_xbox_and_dualsense_frames() {
        let mut pad = VirtualPad::create_noop(VirtualPadConfig::default());
        for raw in [
            xbox_press(SimButton::Cross),
            xbox_press(SimButton::L2),
            dualsense_usb_press(SimButton::Circle),
            dualsense_usb_press(SimButton::Ps),
        ] {
            let frame = if raw[0] == crate::parse_xbox::XBOX_REPORT_ID {
                simulate_xbox_frame(&raw).unwrap()
            } else {
                simulate_dualsense_frame(&raw).unwrap()
            };
            let bytes = encode_clpd(&frame);
            let decoded = PadFrame::decode(&bytes).unwrap();
            pad.apply(&decoded).expect("noop apply");
        }
    }

    #[test]
    fn host_noop_applies_switch_and_steam_frames() {
        let mut pad = VirtualPad::create_noop(VirtualPadConfig::default());
        for raw in [
            switch_press(SimButton::Cross),
            switch_press(SimButton::L2),
            steam_press(SimButton::Circle),
            steam_press(SimButton::Options),
        ] {
            let frame = match raw[0] {
                crate::parse_switch::SWITCH_REPORT_ID => simulate_switch_frame(&raw).unwrap(),
                crate::parse_steam::STEAM_REPORT_ID => simulate_steam_frame(&raw).unwrap(),
                _ => unreachable!(),
            };
            let bytes = encode_clpd(&frame);
            let decoded = PadFrame::decode(&bytes).unwrap();
            pad.apply(&decoded).expect("noop apply");
        }
    }

    #[test]
    fn clpd_encode_matches_proto_length() {
        // Wire encode is always CLPD v2 (client_ts_ms for input_wm / S_p50).
        let f = PadFrame::neutral();
        let mut buf = BytesMut::new();
        f.encode(&mut buf);
        assert_eq!(buf.len(), PAD_FRAME_LEN_V2);
        assert_eq!(encode_clpd(&f).len(), PAD_FRAME_LEN_V2);
    }
}
