// socketcan/tests/serde.rs
//
// Integration tests for the optional `serde` feature.
//
// This file is part of the Rust 'socketcan-rs' library.
//
// Licensed under the MIT license:
//   <LICENSE or http://opensource.org/licenses/MIT>
// This file may not be copied, modified, or distributed except according
// to those terms.

//! Tests for serialization support.
//!
//! These deliberately test through the public API, as a downstream user sees
//! it, and they assert against **literal JSON** rather than only round-tripping.
//! The serialized form is a compatibility surface of its own: a round-trip test
//! passes happily while the wire format silently changes underneath it, and no
//! semver checker will notice.

#![cfg(feature = "serde")]

use socketcan::{
    CanAnyFrame, CanDataFrame, CanError, CanErrorFrame, CanErrors, CanFdFrame, CanFilter, CanFrame,
    CanId, CanTimestamps, EmbeddedFrame, Error, StandardId,
    errors::{
        CAN_ERR_CNT, CAN_ERR_CRTL, ControllerProblem, Location, TransceiverError, ViolationType,
    },
    id::FdFlags,
};

/// Round-trips a value through JSON and asserts the exact serialized text.
fn check_json<T>(value: &T, expect: &str) -> T
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let json = serde_json::to_string(value).unwrap();
    assert_eq!(json, expect, "serialized format changed");
    let back: T = serde_json::from_str(&json).unwrap();
    assert_eq!(&back, value, "JSON round-trip lost information");
    back
}

/// Round-trips a value through MessagePack, returning the encoded bytes.
fn round_trip_msgpack<T>(value: &T) -> Vec<u8>
where
    T: serde::Serialize + serde::de::DeserializeOwned + PartialEq + std::fmt::Debug,
{
    let buf = rmp_serde::to_vec(value).unwrap();
    let back: T = rmp_serde::from_slice(&buf).unwrap();
    assert_eq!(&back, value, "MessagePack round-trip lost information");
    buf
}

// ----- identifiers -----

#[test]
fn can_id_is_self_describing() {
    // A standard and an extended ID with the same numeric value must not
    // collapse to the same serialized form.
    let std_id = CanId::standard(0x123).unwrap();
    let ext_id = CanId::extended(0x123).unwrap();
    check_json(&std_id, r#"{"Standard":291}"#);
    check_json(&ext_id, r#"{"Extended":291}"#);
    assert_ne!(
        serde_json::to_string(&std_id).unwrap(),
        serde_json::to_string(&ext_id).unwrap()
    );
}

#[test]
fn can_id_range_is_validated_on_deserialize() {
    // 0x800 is out of range for an 11-bit identifier.
    assert!(serde_json::from_str::<CanId>(r#"{"Standard":2048}"#).is_err());
    // ... and 0x2000_0000 for a 29-bit one.
    assert!(serde_json::from_str::<CanId>(r#"{"Extended":536870912}"#).is_err());
}

// ----- frames -----

#[test]
fn data_frame_format() {
    let frame =
        CanDataFrame::new(StandardId::new(0x123).unwrap(), &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
    check_json(
        &frame,
        r#"{"id":{"Standard":291},"data":[222,173,190,239]}"#,
    );
}

#[test]
fn empty_payload_is_omitted() {
    let frame = CanDataFrame::new(StandardId::new(0x080).unwrap(), &[]).unwrap();
    let back = check_json(&frame, r#"{"id":{"Standard":128}}"#);
    assert_eq!(back.data(), &[] as &[u8]);
}

#[test]
fn remote_frame_carries_dlc_not_data() {
    let frame = CanFrame::new_remote(StandardId::new(0x104).unwrap(), 4).unwrap();
    check_json(&frame, r#"{"Remote":{"id":{"Standard":260},"dlc":4}}"#);
}

#[test]
fn fd_frame_flags_are_named() {
    let frame = CanFdFrame::with_flags(
        StandardId::new(0x701).unwrap(),
        &[0x7F],
        FdFlags::BRS | FdFlags::FDF,
    )
    .unwrap();
    // bitflags serializes as a flag-name string, not an opaque integer.
    check_json(
        &frame,
        r#"{"id":{"Standard":1793},"flags":"BRS | FDF","data":[127]}"#,
    );
}

/// `IdFlags` is serde-enabled but is not reachable through any frame repr
/// (the frame types carry a `CanId` instead), so it needs its own coverage.
#[test]
fn id_flags_serialize_by_name() {
    use socketcan::id::IdFlags;
    check_json(&(IdFlags::EFF | IdFlags::RTR), r#""EFF | RTR""#);
    check_json(&IdFlags::empty(), r#""""#);
    assert!(serde_json::from_str::<IdFlags>(r#""NOPE""#).is_err());
}

#[test]
fn unknown_flag_name_is_rejected() {
    let bad = r#"{"id":{"Standard":1},"flags":"NOPE","data":[]}"#;
    assert!(serde_json::from_str::<CanFdFrame>(bad).is_err());
}

#[test]
fn error_frame_format() {
    // CAN_ERR_CRTL with both warning bits in data[1].
    let frame = CanErrorFrame::new_error(CAN_ERR_CRTL, &[0, 0x0C, 0, 0, 0, 0, 0, 0]).unwrap();
    check_json(&frame, r#"{"error_bits":4,"data":[0,12,0,0,0,0,0,0]}"#);
}

#[test]
fn payload_length_is_validated_on_deserialize() {
    // Nine bytes will not fit a classical frame.
    let bad = r#"{"id":{"Standard":1},"data":[1,2,3,4,5,6,7,8,9]}"#;
    assert!(serde_json::from_str::<CanDataFrame>(bad).is_err());

    // ... and 65 will not fit an FD frame.
    let big: Vec<u8> = (0..65).collect();
    let bad = format!(
        r#"{{"id":{{"Standard":1}},"flags":"FDF","data":{}}}"#,
        serde_json::to_string(&big).unwrap()
    );
    assert!(serde_json::from_str::<CanFdFrame>(&bad).is_err());
}

#[test]
fn frame_enums_round_trip() {
    let data = CanFrame::new(StandardId::new(0x123).unwrap(), &[1, 2]).unwrap();
    check_json(&data, r#"{"Data":{"id":{"Standard":291},"data":[1,2]}}"#);

    let any: CanAnyFrame = data.into();
    check_json(&any, r#"{"Normal":{"id":{"Standard":291},"data":[1,2]}}"#);
}

/// The payload must reach a binary format's native byte-string type rather
/// than being encoded as a sequence of integers.
///
/// This is the test a JSON-only suite cannot do: JSON has no byte-string type,
/// so an implementation that only handles sequences passes every check above
/// while producing bloated and non-idiomatic output everywhere else.
#[test]
fn payload_uses_native_bytes_in_binary_formats() {
    let frame =
        CanDataFrame::new(StandardId::new(0x123).unwrap(), &[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
    let buf = round_trip_msgpack(&frame);

    // MessagePack encodes a 4-byte `bin 8` as 0xC4 0x04 followed by the bytes.
    // A sequence of four integers would instead appear as 0x94 followed by
    // four separately-tagged values, and each byte >= 0x80 would need two
    // bytes of its own.
    let needle = [0xC4u8, 0x04, 0xDE, 0xAD, 0xBE, 0xEF];
    assert!(
        buf.windows(needle.len()).any(|w| w == needle),
        "payload was not encoded as a MessagePack byte string: {buf:02X?}"
    );
}

// ----- errors -----

#[test]
fn can_errors_serializes_as_a_flat_array() {
    // The head+tail internal layout must not leak into the format.
    let frame =
        CanErrorFrame::new_error(CAN_ERR_CRTL | CAN_ERR_CNT, &[0, 0x0C, 0, 0, 0, 0, 112, 96])
            .unwrap();
    let errs = frame.into_errors();
    assert_eq!(errs.len(), 3);
    check_json(
        &errs,
        r#"[{"ControllerProblem":"ReceiveErrorWarning"},{"ControllerProblem":"TransmitErrorWarning"},{"Counters":{"tx":112,"rx":96}}]"#,
    );
}

/// `CanErrors` is non-empty by construction. Deserialization must not be a
/// way around that from outside the crate.
#[test]
fn can_errors_rejects_an_empty_sequence() {
    assert!(serde_json::from_str::<CanErrors>("[]").is_err());

    let one: CanErrors = serde_json::from_str(r#"["BusOff"]"#).unwrap();
    assert!(one.is_single());
    assert_eq!(*one.first(), CanError::BusOff);
}

#[test]
fn error_variants_round_trip() {
    let cases: &[(CanError, &str)] = &[
        (CanError::TransmitTimeout, r#""TransmitTimeout""#),
        (CanError::LostArbitration(7), r#"{"LostArbitration":7}"#),
        (
            CanError::ControllerProblem(ControllerProblem::BackToErrorActive),
            r#"{"ControllerProblem":"BackToErrorActive"}"#,
        ),
        (
            CanError::ProtocolViolation {
                vtype: ViolationType::BitStuffingError,
                location: Location::CrcSequence,
            },
            r#"{"ProtocolViolation":{"vtype":"BitStuffingError","location":"CrcSequence"}}"#,
        ),
        (
            CanError::TransceiverError(TransceiverError::CanLowNoWire),
            r#"{"TransceiverError":"CanLowNoWire"}"#,
        ),
        (
            CanError::Counters { tx: 1, rx: 2 },
            r#"{"Counters":{"tx":1,"rx":2}}"#,
        ),
        (CanError::Unknown(0x400), r#"{"Unknown":1024}"#),
    ];
    for (err, expect) in cases {
        check_json(err, expect);
    }
}

/// The `Reserved` location keeps its raw byte, so an unnamed `data[3]` code
/// survives serialization.
#[test]
fn reserved_location_keeps_its_value() {
    check_json(&Location::Reserved(0x1F), r#"{"Reserved":31}"#);
}

// ----- the composite Error -----

#[test]
fn error_can_variant_round_trips_exactly() {
    let err = Error::from(CanError::BusOff);
    let json = serde_json::to_string(&err).unwrap();
    assert_eq!(json, r#"{"Can":["BusOff"]}"#);

    let back: Error = serde_json::from_str(&json).unwrap();
    match back {
        Error::Can(errs) => {
            assert!(errs.is_single());
            assert_eq!(*errs.first(), CanError::BusOff);
        }
        other => panic!("expected a CAN error, got {other:?}"),
    }
}

/// The `Io` variant round-trips its *information* but not the error's
/// identity. These assertions pin the documented losses, not just what
/// survives — an untested documented loss is only a comment.
#[test]
fn error_io_variant_is_lossy_as_documented() {
    // ENOENT maps to a nameable kind, so the kind survives.
    let original = std::io::Error::from_raw_os_error(libc::ENOENT);
    assert_eq!(original.kind(), std::io::ErrorKind::NotFound);
    assert!(original.raw_os_error().is_some());

    let err = Error::Io(original);
    let json = serde_json::to_string(&err).unwrap();
    let back: Error = serde_json::from_str(&json).unwrap();

    match back {
        Error::Io(e) => {
            // Preserved: the kind and the message text.
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
            assert!(!e.to_string().is_empty());
            // Lost, by design: the OS errno and any source chain.
            assert!(
                e.raw_os_error().is_none(),
                "raw_os_error unexpectedly survived; update the docs if this changed"
            );
        }
        other => panic!("expected an I/O error, got {other:?}"),
    }
}

/// A CAN interface going down is `ENETDOWN`, which must keep its kind — this
/// is the most likely I/O error for this crate to be asked to serialize.
#[test]
fn network_down_kind_survives() {
    let err = Error::Io(std::io::Error::from_raw_os_error(libc::ENETDOWN));
    let json = serde_json::to_string(&err).unwrap();
    assert!(json.contains("NetworkDown"), "{json}");

    let back: Error = serde_json::from_str(&json).unwrap();
    match back {
        Error::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::NetworkDown),
        other => panic!("expected an I/O error, got {other:?}"),
    }
}

/// Some errnos map to kinds that cannot be named at all — `ENODEV` becomes the
/// unstable `ErrorKind::Uncategorized` — so they necessarily come back as
/// `Other`. Pinned so the behaviour is a recorded decision.
#[test]
fn unnameable_io_kind_becomes_other() {
    let original = std::io::Error::from_raw_os_error(libc::ENODEV);
    // Not `NotFound`, not `Other` — a kind with no stable name.
    assert_ne!(original.kind(), std::io::ErrorKind::Other);

    let err = Error::Io(original);
    let back: Error = serde_json::from_str(&serde_json::to_string(&err).unwrap()).unwrap();
    match back {
        Error::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::Other),
        other => panic!("expected an I/O error, got {other:?}"),
    }
}

/// A kind name written by a newer version must not fail to deserialize in an
/// older one; it degrades to `Other`.
#[test]
fn unknown_io_kind_degrades_to_other() {
    let json = r#"{"Io":{"kind":"SomeFutureKind","message":"whatever"}}"#;
    let back: Error = serde_json::from_str(json).unwrap();
    match back {
        Error::Io(e) => assert_eq!(e.kind(), std::io::ErrorKind::Other),
        other => panic!("expected an I/O error, got {other:?}"),
    }
}

// ----- other types -----

#[test]
fn filter_round_trips() {
    let filter = CanFilter::new(0x123, 0x7FF);
    let json = serde_json::to_string(&filter).unwrap();
    assert_eq!(json, r#"{"id":291,"mask":2047}"#);
    let back: CanFilter = serde_json::from_str(&json).unwrap();
    assert_eq!(back, filter);
}

#[test]
fn timestamps_round_trip() {
    let ts = CanTimestamps {
        socket: Some(
            std::time::UNIX_EPOCH + std::time::Duration::from_micros(1_785_099_856_242_430),
        ),
        sw: None,
        hw: Some(std::time::Duration::from_nanos(12345)),
    };
    let json = serde_json::to_string(&ts).unwrap();
    let back: CanTimestamps = serde_json::from_str(&json).unwrap();
    // CanTimestamps is not PartialEq, so compare field by field.
    assert_eq!(back.socket, ts.socket);
    assert_eq!(back.sw, ts.sw);
    assert_eq!(back.hw, ts.hw);
}

#[cfg(feature = "dump")]
#[test]
fn dump_record_round_trips() {
    use socketcan::dump::CanDumpRecord;

    let frame = CanDataFrame::new(StandardId::new(0x123).unwrap(), &[0xDE, 0xAD]).unwrap();
    let rec = CanDumpRecord {
        t_us: 1_785_099_856_242_430,
        device: "vcan0".into(),
        frame: CanFrame::Data(frame).into(),
    };
    let json = serde_json::to_string(&rec).unwrap();
    let back: CanDumpRecord = serde_json::from_str(&json).unwrap();
    // CanDumpRecord is not PartialEq, so compare the rendered candump line.
    assert_eq!(back.to_string(), rec.to_string());
    assert_eq!(back.t_us, rec.t_us);
    assert_eq!(back.device, rec.device);
}

/// `dump::ParseError` gets the same treatment as the crate-level `Error`, so
/// the two are consistent: unit variants round-trip exactly, and `Io` is
/// reduced to a kind name plus a message.
#[cfg(feature = "dump")]
#[test]
fn parse_error_round_trips() {
    use socketcan::dump::ParseError;

    let e = ParseError::InvalidCanFrame;
    let json = serde_json::to_string(&e).unwrap();
    assert_eq!(json, r#""InvalidCanFrame""#);
    assert!(matches!(
        serde_json::from_str::<ParseError>(&json).unwrap(),
        ParseError::InvalidCanFrame
    ));

    let e = ParseError::Io(std::io::Error::from_raw_os_error(libc::ENOENT));
    let json = serde_json::to_string(&e).unwrap();
    assert!(json.contains("NotFound"), "{json}");
    match serde_json::from_str::<ParseError>(&json).unwrap() {
        ParseError::Io(e) => {
            assert_eq!(e.kind(), std::io::ErrorKind::NotFound);
            assert!(e.raw_os_error().is_none(), "documented loss");
        }
        other => panic!("expected an I/O error, got {other:?}"),
    }
}

// ----- netlink configuration -----

/// The main practical use of the feature: keeping interface configuration in
/// a file.
#[cfg(feature = "netlink")]
#[test]
fn interface_params_round_trip() {
    use socketcan::nl::{CanBitTiming, CanCtrlModes, InterfaceCanParams};

    let params = InterfaceCanParams {
        bit_timing: Some(CanBitTiming {
            bitrate: 500_000,
            sample_point: 875,
            ..Default::default()
        }),
        restart_ms: Some(100),
        ctrl_mode: Some(CanCtrlModes::new(0x03, 0x01)),
        ..Default::default()
    };

    let json = serde_json::to_string(&params).unwrap();
    let back: InterfaceCanParams = serde_json::from_str(&json).unwrap();

    assert_eq!(back.bit_timing.unwrap().bitrate, 500_000);
    assert_eq!(back.bit_timing.unwrap().sample_point, 875);
    assert_eq!(back.restart_ms, Some(100));
    assert!(back.clock.is_none());

    // And it can be authored by hand, which is the point.
    let authored = r#"{"bit_timing":{"bitrate":250000,"sample_point":0,"tq":0,
        "prop_seg":0,"phase_seg1":0,"phase_seg2":0,"sjw":0,"brp":0},
        "bit_timing_const":null,"clock":null,"state":null,"restart_ms":50,
        "berr_counter":null,"ctrl_mode":null,"data_bit_timing":null,
        "data_bit_timing_const":null,"termination":null}"#;
    let parsed: InterfaceCanParams = serde_json::from_str(authored).unwrap();
    assert_eq!(parsed.bit_timing.unwrap().bitrate, 250_000);
    assert_eq!(parsed.restart_ms, Some(50));
}
