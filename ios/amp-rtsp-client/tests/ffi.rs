// Copyright (C) Mike Olson
// SPDX-License-Identifier: MIT OR Apache-2.0

//! Drives the C interface against a scripted RTSP server that sends real camera
//! H.264 parameter sets, so these tests cover URL credentials, parameter and
//! frame conversion, errors and stop without a camera.

use std::ffi::{c_char, c_void, CStr, CString};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{mpsc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use amp_rtsp_client::*;

const IR_SPS: [u8; 14] = [
    0x67, 0x42, 0xc0, 0x0d, 0x8c, 0x68, 0x18, 0x1d, 0x65, 0xb8, 0x07, 0x84, 0x42, 0x35,
];
const COLOR_SPS: [u8; 14] = [
    0x67, 0x42, 0xc0, 0x1f, 0x8c, 0x68, 0x05, 0x00, 0x5b, 0xa0, 0x1e, 0x11, 0x08, 0xd4,
];
const PPS: [u8; 4] = [0x68, 0xce, 0x3c, 0x80];
const IR_SPROP: &str = "Z0LADYxoGB1luAeEQjU=,aM48gA==";
const BASIC_AMP_S3CR3T: &str = "Basic YW1wOnMzY3IzdA==";
const IDR: [u8; 5] = [0x65, 0x88, 0x84, 0x00, 0x10];
const NON_IDR: [u8; 4] = [0x41, 0x9a, 0x02, 0x04];

#[derive(Debug, Clone, PartialEq)]
enum Event {
    Parameters {
        sps: Vec<u8>,
        pps: Vec<u8>,
        width: u32,
        height: u32,
    },
    Frame {
        data: Vec<u8>,
        timestamp: i64,
        clock_rate: u32,
        keyframe: bool,
        loss: u16,
    },
    Error {
        code: i32,
        message: String,
    },
}

#[derive(Default)]
struct Recorder {
    events: Mutex<Vec<Event>>,
    changed: Condvar,
}

impl Recorder {
    fn push(&self, event: Event) {
        self.events.lock().unwrap().push(event);
        self.changed.notify_all();
    }

    fn wait_for(&self, timeout: Duration, done: impl Fn(&[Event]) -> bool) -> Vec<Event> {
        let deadline = Instant::now() + timeout;
        let mut events = self.events.lock().unwrap();
        while !done(&events) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out; events so far: {events:?}");
            events = self.changed.wait_timeout(events, left).unwrap().0;
        }
        events.clone()
    }

    fn snapshot(&self) -> Vec<Event> {
        self.events.lock().unwrap().clone()
    }
}

unsafe extern "C" fn on_parameters(context: *mut c_void, value: *const AmpRtspParameters) {
    let recorder = unsafe { &*(context as *const Recorder) };
    let value = unsafe { &*value };
    recorder.push(Event::Parameters {
        sps: unsafe { std::slice::from_raw_parts(value.sps, value.sps_len) }.to_vec(),
        pps: unsafe { std::slice::from_raw_parts(value.pps, value.pps_len) }.to_vec(),
        width: value.width,
        height: value.height,
    });
}

unsafe extern "C" fn on_frame(context: *mut c_void, value: *const AmpRtspFrame) {
    let recorder = unsafe { &*(context as *const Recorder) };
    let value = unsafe { &*value };
    recorder.push(Event::Frame {
        data: unsafe { std::slice::from_raw_parts(value.data, value.len) }.to_vec(),
        timestamp: value.timestamp,
        clock_rate: value.clock_rate,
        keyframe: value.is_keyframe,
        loss: value.loss,
    });
}

unsafe extern "C" fn on_error(context: *mut c_void, code: i32, message: *const c_char) {
    let recorder = unsafe { &*(context as *const Recorder) };
    let message = unsafe { CStr::from_ptr(message) }
        .to_string_lossy()
        .into_owned();
    recorder.push(Event::Error { code, message });
}

fn callbacks(recorder: &Recorder) -> AmpRtspCallbacks {
    AmpRtspCallbacks {
        context: recorder as *const Recorder as *mut c_void,
        on_parameters: Some(on_parameters),
        on_frame: Some(on_frame),
        on_error: Some(on_error),
    }
}

fn start(url: &str, recorder: &Recorder) -> *mut AmpRtspClient {
    let url = CString::new(url).unwrap();
    let mut client = std::ptr::null_mut();
    let code = unsafe { amp_rtsp_client_start(url.as_ptr(), callbacks(recorder), &mut client) };
    assert_eq!(code, AMP_RTSP_OK);
    assert!(!client.is_null());
    client
}

fn avcc(nal: &[u8]) -> Vec<u8> {
    let mut out = (nal.len() as u32).to_be_bytes().to_vec();
    out.extend_from_slice(nal);
    out
}

struct Request {
    method: String,
    cseq: String,
    authorization: Option<String>,
}

fn read_request(reader: &mut BufReader<TcpStream>) -> Option<Request> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        if !line.trim().is_empty() {
            break;
        }
    }
    let method = line.split_whitespace().next()?.to_owned();
    let (mut cseq, mut authorization) = (String::new(), None);
    loop {
        line.clear();
        if reader.read_line(&mut line).ok()? == 0 {
            return None;
        }
        let header = line.trim_end();
        if header.is_empty() {
            break;
        }
        let (name, value) = header.split_once(':')?;
        match name.trim().to_ascii_lowercase().as_str() {
            "cseq" => cseq = value.trim().to_owned(),
            "authorization" => authorization = Some(value.trim().to_owned()),
            _ => {}
        }
    }
    Some(Request {
        method,
        cseq,
        authorization,
    })
}

fn respond(stream: &mut TcpStream, cseq: &str, status: &str, headers: &[String], body: &str) {
    let mut response = format!("RTSP/1.0 {status}\r\nCSeq: {cseq}\r\n");
    for header in headers {
        response.push_str(header);
        response.push_str("\r\n");
    }
    if !body.is_empty() {
        response.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    response.push_str("\r\n");
    response.push_str(body);
    stream.write_all(response.as_bytes()).unwrap();
}

fn rtp(seq: u16, timestamp: u32, marker: bool, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x80, if marker { 0xe0 } else { 0x60 }];
    packet.extend_from_slice(&seq.to_be_bytes());
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.extend_from_slice(&0x1234_5678u32.to_be_bytes());
    packet.extend_from_slice(payload);
    let mut interleaved = vec![b'$', 0];
    interleaved.extend_from_slice(&(packet.len() as u16).to_be_bytes());
    interleaved.extend_from_slice(&packet);
    interleaved
}

fn sdp(port: u16, video: bool) -> String {
    let media = if video {
        format!(
            "m=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\n\
             a=fmtp:96 packetization-mode=1;profile-level-id=42c00d;sprop-parameter-sets={IR_SPROP}\r\n\
             a=control:stream=0\r\n"
        )
    } else {
        "m=audio 0 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\na=control:stream=0\r\n".to_owned()
    };
    format!(
        "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=Session streamed with GStreamer\r\n\
         c=IN IP4 0.0.0.0\r\nt=0 0\r\na=control:rtsp://127.0.0.1:{port}/ir\r\n{media}"
    )
}

enum Script {
    /// Basic auth, then the IR stream, a parameter change to color, then close.
    Stream,
    AlwaysUnauthorized,
    AudioOnly,
}

/// Serves one connection with the given script and returns whether the peer
/// closed the connection before the server did.
fn serve(listener: TcpListener, script: Script) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let port = listener.local_addr().unwrap().port();
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let session = "Session: 7011;timeout=60".to_owned();
        while let Some(request) = read_request(&mut reader) {
            let cseq = request.cseq.as_str();
            match request.method.as_str() {
                "DESCRIBE" => {
                    let authorized = request.authorization.as_deref() == Some(BASIC_AMP_S3CR3T);
                    if matches!(script, Script::AlwaysUnauthorized) || !authorized {
                        respond(
                            &mut writer,
                            cseq,
                            "401 Unauthorized",
                            &["WWW-Authenticate: Basic realm=\"ocuvera\"".to_owned()],
                            "",
                        );
                        continue;
                    }
                    let body = sdp(port, !matches!(script, Script::AudioOnly));
                    respond(
                        &mut writer,
                        cseq,
                        "200 OK",
                        &[
                            "Content-Type: application/sdp".to_owned(),
                            format!("Content-Base: rtsp://127.0.0.1:{port}/ir/"),
                        ],
                        &body,
                    );
                }
                "SETUP" => {
                    assert_eq!(request.authorization.as_deref(), Some(BASIC_AMP_S3CR3T));
                    respond(
                        &mut writer,
                        cseq,
                        "200 OK",
                        &[
                            "Transport: RTP/AVP/TCP;unicast;interleaved=0-1;ssrc=12345678"
                                .to_owned(),
                            session.clone(),
                        ],
                        "",
                    );
                }
                "PLAY" => {
                    respond(
                        &mut writer,
                        cseq,
                        "200 OK",
                        std::slice::from_ref(&session),
                        "",
                    );
                    let mut packets = Vec::new();
                    packets.extend(rtp(1, 1000, true, &IDR));
                    packets.extend(rtp(2, 7000, true, &NON_IDR));
                    packets.extend(rtp(3, 13000, false, &COLOR_SPS));
                    packets.extend(rtp(4, 13000, false, &PPS));
                    packets.extend(rtp(5, 13000, true, &IDR));
                    writer.write_all(&packets).unwrap();
                    thread::sleep(Duration::from_millis(200));
                    return;
                }
                _ => respond(
                    &mut writer,
                    cseq,
                    "200 OK",
                    std::slice::from_ref(&session),
                    "",
                ),
            }
        }
    })
}

fn listener() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

fn is_error(event: &Event) -> bool {
    matches!(event, Event::Error { .. })
}

#[test]
fn delivers_ocuvera_parameters_and_avcc_frames_then_reports_the_end() {
    let (listener, port) = listener();
    let server = serve(listener, Script::Stream);
    let recorder = Recorder::default();
    let client = start(&format!("rtsp://amp:s3cr3t@127.0.0.1:{port}/ir"), &recorder);
    let events = recorder.wait_for(Duration::from_secs(10), |events| {
        events.iter().any(is_error)
    });
    unsafe { amp_rtsp_client_stop(client) };
    server.join().unwrap();

    let expected_prefix = [
        Event::Parameters {
            sps: IR_SPS.to_vec(),
            pps: PPS.to_vec(),
            width: 376,
            height: 220,
        },
        Event::Frame {
            data: avcc(&IDR),
            timestamp: 1000,
            clock_rate: 90_000,
            keyframe: true,
            loss: 0,
        },
        Event::Frame {
            data: avcc(&NON_IDR),
            timestamp: 7000,
            clock_rate: 90_000,
            keyframe: false,
            loss: 0,
        },
        Event::Parameters {
            sps: COLOR_SPS.to_vec(),
            pps: PPS.to_vec(),
            width: 1280,
            height: 720,
        },
        Event::Frame {
            data: avcc(&IDR),
            timestamp: 13000,
            clock_rate: 90_000,
            keyframe: true,
            loss: 0,
        },
    ];
    assert_eq!(&events[..expected_prefix.len()], &expected_prefix);
    assert_eq!(
        events.len(),
        expected_prefix.len() + 1,
        "one terminal error: {events:?}"
    );
    let Event::Error { code, .. } = &events[expected_prefix.len()] else {
        unreachable!()
    };
    assert_eq!(*code, AMP_RTSP_ERROR_ENDED);
}

#[test]
fn reports_rejected_credentials_as_an_auth_error() {
    let (listener, port) = listener();
    let server = serve(listener, Script::AlwaysUnauthorized);
    let recorder = Recorder::default();
    let client = start(
        &format!("rtsp://amp:wrong-pass@127.0.0.1:{port}/ir"),
        &recorder,
    );
    let events = recorder.wait_for(Duration::from_secs(10), |events| {
        events.iter().any(is_error)
    });
    unsafe { amp_rtsp_client_stop(client) };
    server.join().unwrap();
    let [Event::Error { code, message }] = events.as_slice() else {
        panic!("{events:?}")
    };
    assert_eq!(*code, AMP_RTSP_ERROR_AUTH);
    assert!(!message.contains("wrong-pass"), "{message}");
}

#[test]
fn reports_a_stream_without_h264_video() {
    let (listener, port) = listener();
    let server = serve(listener, Script::AudioOnly);
    let recorder = Recorder::default();
    let client = start(&format!("rtsp://amp:s3cr3t@127.0.0.1:{port}/ir"), &recorder);
    let events = recorder.wait_for(Duration::from_secs(10), |events| {
        events.iter().any(is_error)
    });
    unsafe { amp_rtsp_client_stop(client) };
    server.join().unwrap();
    let [Event::Error { code, .. }] = events.as_slice() else {
        panic!("{events:?}")
    };
    assert_eq!(*code, AMP_RTSP_ERROR_NO_H264);
}

#[test]
fn reports_a_refused_connection_once_without_the_password() {
    let (listener, port) = listener();
    drop(listener);
    let recorder = Recorder::default();
    let client = start(
        &format!("rtsp://amp:hunter2@127.0.0.1:{port}/ir"),
        &recorder,
    );
    recorder.wait_for(Duration::from_secs(10), |events| {
        events.iter().any(is_error)
    });
    thread::sleep(Duration::from_millis(200));
    unsafe { amp_rtsp_client_stop(client) };
    let events = recorder.snapshot();
    let [Event::Error { code, message }] = events.as_slice() else {
        panic!("{events:?}")
    };
    assert_eq!(*code, AMP_RTSP_ERROR_CONNECT);
    assert!(!message.contains("hunter2"), "{message}");
    assert!(!message.is_empty());
}

#[test]
fn stop_silences_callbacks_and_closes_the_connection() {
    let (listener, port) = listener();
    let recorder = Recorder::default();
    let client = start(&format!("rtsp://127.0.0.1:{port}/ir"), &recorder);
    let (mut stream, _) = listener.accept().unwrap();
    let mut request = [0u8; 512];
    assert!(stream.read(&mut request).unwrap() > 0, "DESCRIBE was sent");
    unsafe { amp_rtsp_client_stop(client) };
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut rest = Vec::new();
    let closed = stream.read_to_end(&mut rest);
    assert!(
        closed.is_ok(),
        "client closed its socket after stop: {closed:?}"
    );
    thread::sleep(Duration::from_millis(200));
    assert!(recorder.snapshot().is_empty(), "no callbacks after stop");
}

#[test]
fn start_rejects_bad_arguments_synchronously() {
    let recorder = Recorder::default();
    let cases = [
        ("", AMP_RTSP_ERROR_INVALID_URL),
        ("camera/ir", AMP_RTSP_ERROR_INVALID_URL),
        ("rtsps://camera/ir", AMP_RTSP_ERROR_UNSUPPORTED_SCHEME),
        (
            "https://camera/live.m3u8",
            AMP_RTSP_ERROR_UNSUPPORTED_SCHEME,
        ),
    ];
    for (url, expected) in cases {
        let url = CString::new(url).unwrap();
        let mut client = std::ptr::dangling_mut::<AmpRtspClient>();
        let code =
            unsafe { amp_rtsp_client_start(url.as_ptr(), callbacks(&recorder), &mut client) };
        assert_eq!(code, expected, "{url:?}");
        assert!(client.is_null());
    }
    let mut client = std::ptr::null_mut();
    let code =
        unsafe { amp_rtsp_client_start(std::ptr::null(), callbacks(&recorder), &mut client) };
    assert_eq!(code, AMP_RTSP_ERROR_INVALID_ARGUMENT);
    let not_utf8 = [0xffu8, 0xfe, 0];
    let code = unsafe {
        amp_rtsp_client_start(
            not_utf8.as_ptr() as *const c_char,
            callbacks(&recorder),
            &mut client,
        )
    };
    assert_eq!(code, AMP_RTSP_ERROR_INVALID_ARGUMENT);
    let url = CString::new("rtsp://camera/ir").unwrap();
    let code =
        unsafe { amp_rtsp_client_start(url.as_ptr(), callbacks(&recorder), std::ptr::null_mut()) };
    assert_eq!(code, AMP_RTSP_ERROR_INVALID_ARGUMENT);
    unsafe { amp_rtsp_client_stop(std::ptr::null_mut()) };
    thread::sleep(Duration::from_millis(100));
    assert!(recorder.snapshot().is_empty());
}

#[test]
fn callbacks_may_be_absent() {
    let (listener, port) = listener();
    let server = serve(listener, Script::Stream);
    let url = CString::new(format!("rtsp://amp:s3cr3t@127.0.0.1:{port}/ir")).unwrap();
    let empty = AmpRtspCallbacks {
        context: std::ptr::null_mut(),
        on_parameters: None,
        on_frame: None,
        on_error: None,
    };
    let mut client = std::ptr::null_mut();
    assert_eq!(
        unsafe { amp_rtsp_client_start(url.as_ptr(), empty, &mut client) },
        AMP_RTSP_OK
    );
    server.join().unwrap();
    unsafe { amp_rtsp_client_stop(client) };
}

/// Plays `packets` after PLAY, then keeps the session open and reports every
/// later request method and when the client closed the connection.
fn serve_and_hold(
    listener: TcpListener,
    packets: Vec<u8>,
) -> mpsc::Receiver<(Vec<String>, Instant)> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let port = listener.local_addr().unwrap().port();
        let (stream, _) = listener.accept().unwrap();
        let mut writer = stream.try_clone().unwrap();
        let mut reader = BufReader::new(stream);
        let session = "Session: 7012;timeout=60".to_owned();
        let mut after_play = Vec::new();
        let mut playing = false;
        while let Some(request) = read_request(&mut reader) {
            let cseq = request.cseq.as_str();
            if playing {
                after_play.push(request.method.clone());
            }
            match request.method.as_str() {
                "DESCRIBE" => respond(
                    &mut writer,
                    cseq,
                    "200 OK",
                    &[
                        "Content-Type: application/sdp".to_owned(),
                        format!("Content-Base: rtsp://127.0.0.1:{port}/ir/"),
                    ],
                    &sdp(port, true),
                ),
                "SETUP" => respond(
                    &mut writer,
                    cseq,
                    "200 OK",
                    &[
                        "Transport: RTP/AVP/TCP;unicast;interleaved=0-1;ssrc=12345678".to_owned(),
                        session.clone(),
                    ],
                    "",
                ),
                "PLAY" => {
                    respond(
                        &mut writer,
                        cseq,
                        "200 OK",
                        std::slice::from_ref(&session),
                        "",
                    );
                    writer.write_all(&packets).unwrap();
                    playing = true;
                }
                _ => respond(
                    &mut writer,
                    cseq,
                    "200 OK",
                    std::slice::from_ref(&session),
                    "",
                ),
            }
        }
        let _ = sender.send((after_play, Instant::now()));
    });
    receiver
}

#[test]
fn stop_returns_only_after_the_session_connection_closed() {
    let (listener, port) = listener();
    let closed = serve_and_hold(listener, rtp(1, 1000, true, &IDR));
    let recorder = Recorder::default();
    let client = start(&format!("rtsp://127.0.0.1:{port}/ir"), &recorder);
    recorder.wait_for(Duration::from_secs(10), |events| {
        events
            .iter()
            .any(|event| matches!(event, Event::Frame { .. }))
    });
    unsafe { amp_rtsp_client_stop(client) };
    let returned = Instant::now();
    let (after_play, closed_at) = closed
        .recv_timeout(Duration::from_secs(5))
        .expect("the server saw the connection close");
    assert!(
        closed_at <= returned,
        "stop returned {:?} before the connection closed",
        closed_at - returned
    );
    assert!(
        after_play.iter().all(|method| method == "TEARDOWN"),
        "{after_play:?}"
    );
    thread::sleep(Duration::from_millis(100));
    assert!(recorder.snapshot().iter().all(|event| !is_error(event)));
}

#[test]
fn frames_after_a_sequence_gap_report_the_loss() {
    let (listener, port) = listener();
    let mut packets = rtp(1, 1000, true, &IDR);
    packets.extend(rtp(2, 7000, true, &NON_IDR));
    packets.extend(rtp(5, 19000, true, &NON_IDR));
    packets.extend(rtp(6, 25000, true, &IDR));
    let closed = serve_and_hold(listener, packets);
    let recorder = Recorder::default();
    let client = start(&format!("rtsp://127.0.0.1:{port}/ir"), &recorder);
    let events = recorder.wait_for(Duration::from_secs(10), |events| {
        events
            .iter()
            .filter(|event| matches!(event, Event::Frame { .. }))
            .count()
            >= 4
    });
    unsafe { amp_rtsp_client_stop(client) };
    let _ = closed.recv_timeout(Duration::from_secs(5));
    let frames: Vec<(i64, bool, u16)> = events
        .iter()
        .filter_map(|event| match event {
            Event::Frame {
                timestamp,
                keyframe,
                loss,
                ..
            } => Some((*timestamp, *keyframe, *loss)),
            _ => None,
        })
        .collect();
    assert_eq!(
        frames,
        [
            (1000, true, 0),
            (7000, false, 0),
            (19000, false, 2),
            (25000, true, 0)
        ]
    );
}
