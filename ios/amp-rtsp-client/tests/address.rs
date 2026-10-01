// Copyright (C) The Retina Authors
// SPDX-License-Identifier: MIT OR Apache-2.0

use amp_rtsp_client::address::{parse, AddressError};

#[test]
fn moves_basic_credentials_out_of_the_url() {
    let address = parse("rtsp://viewer:s3cr3t@192.0.2.10:8554/ir").unwrap();
    assert_eq!(address.url.as_str(), "rtsp://192.0.2.10:8554/ir");
    let credentials = address.credentials.unwrap();
    assert_eq!(credentials.username, "viewer");
    assert_eq!(credentials.password, "s3cr3t");
}

#[test]
fn percent_decodes_reserved_characters_in_userinfo() {
    let address = parse("rtsp://us%40er:p%3Ass%2Fw%25rd%20x@camera.local/depth").unwrap();
    let credentials = address.credentials.unwrap();
    assert_eq!(credentials.username, "us@er");
    assert_eq!(credentials.password, "p:ss/w%rd x");
    assert_eq!(address.url.as_str(), "rtsp://camera.local/depth");
}

#[test]
fn keeps_a_username_without_password() {
    let credentials = parse("rtsp://viewer@camera/color")
        .unwrap()
        .credentials
        .unwrap();
    assert_eq!(credentials.username, "viewer");
    assert_eq!(credentials.password, "");
}

#[test]
fn has_no_credentials_without_userinfo() {
    let address = parse("rtsp://127.0.0.1:18654/ir?token=abc#feed").unwrap();
    assert!(address.credentials.is_none());
    assert_eq!(address.url.as_str(), "rtsp://127.0.0.1:18654/ir?token=abc");
}

#[test]
fn rejects_other_schemes_and_malformed_input() {
    assert_eq!(
        parse("rtsps://camera/ir").unwrap_err(),
        AddressError::UnsupportedScheme("rtsps".into())
    );
    assert_eq!(
        parse("http://camera/ir.m3u8").unwrap_err(),
        AddressError::UnsupportedScheme("http".into())
    );
    assert_eq!(parse("not a url").unwrap_err(), AddressError::Invalid);
    assert_eq!(parse("").unwrap_err(), AddressError::Invalid);
    assert_eq!(parse("rtsp:///ir").unwrap_err(), AddressError::Invalid);
}

#[test]
fn rejects_userinfo_that_is_not_utf8() {
    assert_eq!(
        parse("rtsp://%FF:x@camera/ir").unwrap_err(),
        AddressError::Invalid
    );
}
