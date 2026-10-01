// Copyright (C) The Retina Authors
// SPDX-License-Identifier: MIT OR Apache-2.0

use amp_rtsp_client::h264::{is_avcc, ParameterSets, ParameterTracker};

fn sets(sps: &[u8]) -> ParameterSets {
    ParameterSets {
        sps: sps.to_vec(),
        pps: vec![0x68, 0xce, 0x3c, 0x80],
        width: 376,
        height: 220,
    }
}

#[test]
fn tracker_reports_first_and_changed_parameters_only() {
    let mut tracker = ParameterTracker::default();
    assert!(!tracker.has_parameters());
    assert!(tracker.update(None).is_none());
    assert_eq!(
        tracker.update(Some(sets(&[0x67, 1]))),
        Some(&sets(&[0x67, 1]))
    );
    assert!(tracker.update(Some(sets(&[0x67, 1]))).is_none());
    assert!(tracker.update(None).is_none());
    assert_eq!(
        tracker.update(Some(sets(&[0x67, 2]))),
        Some(&sets(&[0x67, 2]))
    );
    assert!(tracker.has_parameters());
}

#[test]
fn accepts_well_formed_avcc_access_units() {
    assert!(is_avcc(&[0, 0, 0, 2, 0x65, 0x88]));
    assert!(is_avcc(&[0, 0, 0, 1, 0x06, 0, 0, 0, 2, 0x65, 0x88]));
}

#[test]
fn rejects_truncated_empty_or_annex_b_data() {
    assert!(!is_avcc(&[]));
    assert!(!is_avcc(&[0, 0, 0]));
    assert!(!is_avcc(&[0, 0, 0, 3, 0x65, 0x88]));
    assert!(!is_avcc(&[0, 0, 0, 0]));
    assert!(!is_avcc(&[0, 0, 0, 1, 0x65, 0x88, 0x84, 0x00, 0x10]));
}
