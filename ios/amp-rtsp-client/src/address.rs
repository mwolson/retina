// Copyright (C) The Retina Authors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Parses the camera URL and moves its userinfo into Retina credentials.

use percent_encoding::percent_decode_str;
use retina::client::Credentials;
use url::Url;

#[derive(Debug)]
pub struct StreamAddress {
    /// The URL without userinfo, safe to show in errors and logs.
    pub url: Url,
    pub credentials: Option<Credentials>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AddressError {
    Invalid,
    UnsupportedScheme(String),
}

pub fn parse(input: &str) -> Result<StreamAddress, AddressError> {
    let mut url = Url::parse(input.trim()).map_err(|_| AddressError::Invalid)?;
    match url.scheme() {
        "rtsp" => {}
        other => return Err(AddressError::UnsupportedScheme(other.to_owned())),
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(AddressError::Invalid);
    }
    let username = decode(url.username())?;
    let password = decode(url.password().unwrap_or(""))?;
    let credentials = if username.is_empty() && password.is_empty() {
        None
    } else {
        Some(Credentials { username, password })
    };
    url.set_username("").map_err(|_| AddressError::Invalid)?;
    url.set_password(None).map_err(|_| AddressError::Invalid)?;
    url.set_fragment(None);
    Ok(StreamAddress { url, credentials })
}

fn decode(value: &str) -> Result<String, AddressError> {
    percent_decode_str(value)
        .decode_utf8()
        .map(|decoded| decoded.into_owned())
        .map_err(|_| AddressError::Invalid)
}
