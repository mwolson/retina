// Copyright (C) The Retina Authors
// SPDX-License-Identifier: MIT OR Apache-2.0

//! H.264 parameter and frame helpers between Retina and the C interface.

use retina::codec::{ParametersRef, VideoParametersCodec};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParameterSets {
    pub sps: Vec<u8>,
    pub pps: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl ParameterSets {
    pub fn from_retina(parameters: Option<ParametersRef<'_>>) -> Option<Self> {
        let ParametersRef::Video(video) = parameters? else {
            return None;
        };
        let VideoParametersCodec::H264 { sps, pps } = video.codec_params() else {
            return None;
        };
        if sps.is_empty() || pps.is_empty() {
            return None;
        }
        let (width, height) = video.pixel_dimensions();
        Some(Self {
            sps: sps.to_vec(),
            pps: pps.to_vec(),
            width,
            height,
        })
    }
}

/// Remembers the last parameter sets sent so the app only hears about changes.
#[derive(Default)]
pub struct ParameterTracker {
    current: Option<ParameterSets>,
}

impl ParameterTracker {
    /// Returns the parameters when they differ from the last ones reported.
    pub fn update(&mut self, next: Option<ParameterSets>) -> Option<&ParameterSets> {
        let next = next?;
        if self.current.as_ref() == Some(&next) {
            return None;
        }
        self.current = Some(next);
        self.current.as_ref()
    }

    pub fn has_parameters(&self) -> bool {
        self.current.is_some()
    }
}

/// Checks that `data` is a non-empty run of 4-byte length-prefixed NAL units
/// that exactly fills the buffer, as CoreMedia expects for AVCC samples.
pub fn is_avcc(data: &[u8]) -> bool {
    let mut rest = data;
    if rest.is_empty() {
        return false;
    }
    while !rest.is_empty() {
        if rest.len() < 4 {
            return false;
        }
        let len = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        if len == 0 || rest.len() - 4 < len {
            return false;
        }
        rest = &rest[4 + len..];
    }
    true
}
