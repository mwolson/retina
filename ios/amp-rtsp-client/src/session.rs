// Copyright (C) Mike Olson
// SPDX-License-Identifier: MIT OR Apache-2.0

//! One RTSP session: DESCRIBE, SETUP of the H.264 video stream over TCP, PLAY,
//! then frames until the server ends the session or the app stops it.

use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use retina::client::{
    PlayOptions, Session, SessionGroup, SessionOptions, SetupOptions, TcpTransportOptions,
    Transport,
};
use retina::codec::{CodecItem, FrameFormat};

use crate::address::StreamAddress;
use crate::h264::{self, ParameterSets, ParameterTracker};

pub const USER_AGENT: &str = "Alairo AMP Native";
const TEARDOWN_GRACE: Duration = Duration::from_secs(1);

#[derive(Debug, PartialEq, Eq)]
pub enum SessionError {
    Connect(String),
    Auth(String),
    NoH264,
    Ended,
}

pub struct Frame<'a> {
    pub data: &'a [u8],
    pub timestamp: i64,
    pub clock_rate: u32,
    pub loss: u16,
    pub is_keyframe: bool,
}

pub trait Sink {
    fn parameters(&self, parameters: &ParameterSets);
    fn frame(&self, frame: &Frame<'_>);
}

fn classify(error: retina::Error) -> SessionError {
    let message = error.to_string();
    if error.status_code() == Some(401) || message.contains("no credentials supplied") {
        SessionError::Auth(message)
    } else {
        SessionError::Connect(message)
    }
}

/// Runs until the stream fails or ends. Dropping the future stops the stream.
pub async fn play(
    address: StreamAddress,
    group: Arc<SessionGroup>,
    sink: &dyn Sink,
) -> Result<(), SessionError> {
    let options = SessionOptions::default()
        .creds(address.credentials)
        .user_agent(USER_AGENT.to_owned())
        .session_group(group);
    let mut session = Session::describe(address.url, options)
        .await
        .map_err(classify)?;
    let video = session
        .streams()
        .iter()
        .position(|stream| {
            stream.media() == "video" && stream.encoding_name().eq_ignore_ascii_case("h264")
        })
        .ok_or(SessionError::NoH264)?;
    session
        .setup(
            video,
            SetupOptions::default()
                .transport(Transport::Tcp(TcpTransportOptions::default()))
                .frame_format(FrameFormat::MP4),
        )
        .await
        .map_err(classify)?;
    let mut tracker = ParameterTracker::default();
    if let Some(parameters) = tracker.update(ParameterSets::from_retina(
        session.streams()[video].parameters(),
    )) {
        sink.parameters(parameters);
    }
    let mut demuxed = session
        .play(PlayOptions::default())
        .await
        .map_err(classify)?
        .demuxed()
        .map_err(classify)?;
    loop {
        let item = match demuxed.next().await {
            None => return Err(SessionError::Ended),
            Some(Err(error)) => return Err(classify(error)),
            Some(Ok(item)) => item,
        };
        let CodecItem::VideoFrame(frame) = item else {
            continue;
        };
        if frame.stream_id() != video {
            continue;
        }
        if frame.has_new_parameters() || !tracker.has_parameters() {
            if let Some(parameters) = tracker.update(ParameterSets::from_retina(
                demuxed.streams()[video].parameters(),
            )) {
                sink.parameters(parameters);
            }
        }
        if !tracker.has_parameters() || !h264::is_avcc(frame.data()) {
            continue;
        }
        let timestamp = frame.timestamp();
        sink.frame(&Frame {
            data: frame.data(),
            timestamp: timestamp.timestamp(),
            clock_rate: timestamp.clock_rate().get(),
            loss: frame.loss(),
            is_keyframe: frame.is_random_access_point(),
        });
    }
}

/// Gives Retina's background TEARDOWN a short chance to finish after the
/// session is dropped, without holding up the app.
pub async fn await_teardown(group: &SessionGroup) {
    let _ = tokio::time::timeout(TEARDOWN_GRACE, group.await_teardown()).await;
}
