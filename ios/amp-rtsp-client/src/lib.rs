// Copyright (C) Mike Olson
// SPDX-License-Identifier: MIT OR Apache-2.0

//! C interface for the native AMP iOS app. See `include/AmpRtspClient.h` for
//! the contract; this file keeps the two in step.

pub mod address;
pub mod h264;
pub mod session;

use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{self, AssertUnwindSafe};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::Duration;

use retina::client::SessionGroup;
use tokio::sync::oneshot;

use crate::address::AddressError;
use crate::h264::ParameterSets;
use crate::session::{Frame, SessionError, Sink};

pub const AMP_RTSP_OK: i32 = 0;
pub const AMP_RTSP_ERROR_INVALID_ARGUMENT: i32 = 1;
pub const AMP_RTSP_ERROR_INVALID_URL: i32 = 2;
pub const AMP_RTSP_ERROR_UNSUPPORTED_SCHEME: i32 = 3;
pub const AMP_RTSP_ERROR_THREAD: i32 = 4;
pub const AMP_RTSP_ERROR_CONNECT: i32 = 10;
pub const AMP_RTSP_ERROR_AUTH: i32 = 11;
pub const AMP_RTSP_ERROR_NO_H264: i32 = 12;
pub const AMP_RTSP_ERROR_ENDED: i32 = 13;
pub const AMP_RTSP_ERROR_INTERNAL: i32 = 20;

#[repr(C)]
pub struct AmpRtspParameters {
    pub sps: *const u8,
    pub sps_len: usize,
    pub pps: *const u8,
    pub pps_len: usize,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
pub struct AmpRtspFrame {
    pub data: *const u8,
    pub len: usize,
    pub timestamp: i64,
    pub clock_rate: u32,
    pub loss: u16,
    pub is_keyframe: bool,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct AmpRtspCallbacks {
    pub context: *mut c_void,
    pub on_parameters: Option<unsafe extern "C" fn(*mut c_void, *const AmpRtspParameters)>,
    pub on_frame: Option<unsafe extern "C" fn(*mut c_void, *const AmpRtspFrame)>,
    pub on_error: Option<unsafe extern "C" fn(*mut c_void, i32, *const c_char)>,
}

/// Opaque handle returned to C.
pub struct AmpRtspClient {
    shared: Arc<Shared>,
    stop: Option<oneshot::Sender<()>>,
    exited: mpsc::Receiver<()>,
}

/// State shared between the handle and the client thread. The mutex both
/// serializes callbacks and lets stop wait out one that is in flight.
struct Shared {
    callbacks: AmpRtspCallbacks,
    active: Mutex<bool>,
}

// The context pointer is owned by the caller, who promises it can be used from
// the client thread until stop returns.
unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, bool> {
        self.active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn deactivate(&self) {
        *self.lock() = false;
    }

    /// Delivers the terminal error, at most once and never after stop.
    fn fail(&self, code: i32, message: &str) {
        let mut active = self.lock();
        if !*active {
            return;
        }
        *active = false;
        if let Some(on_error) = self.callbacks.on_error {
            let message = c_message(message);
            unsafe { on_error(self.callbacks.context, code, message.as_ptr()) };
        }
    }
}

impl Sink for Shared {
    fn parameters(&self, parameters: &ParameterSets) {
        let active = self.lock();
        let Some(on_parameters) = self.callbacks.on_parameters.filter(|_| *active) else {
            return;
        };
        let value = AmpRtspParameters {
            sps: parameters.sps.as_ptr(),
            sps_len: parameters.sps.len(),
            pps: parameters.pps.as_ptr(),
            pps_len: parameters.pps.len(),
            width: parameters.width,
            height: parameters.height,
        };
        unsafe { on_parameters(self.callbacks.context, &value) };
    }

    fn frame(&self, frame: &Frame<'_>) {
        let active = self.lock();
        let Some(on_frame) = self.callbacks.on_frame.filter(|_| *active) else {
            return;
        };
        let value = AmpRtspFrame {
            data: frame.data.as_ptr(),
            len: frame.data.len(),
            timestamp: frame.timestamp,
            clock_rate: frame.clock_rate,
            loss: frame.loss,
            is_keyframe: frame.is_keyframe,
        };
        unsafe { on_frame(self.callbacks.context, &value) };
    }
}

fn c_message(message: &str) -> CString {
    CString::new(message.replace('\0', " ")).unwrap_or_default()
}

fn error_code_and_message(error: &SessionError) -> (i32, String) {
    match error {
        SessionError::Connect(message) => (AMP_RTSP_ERROR_CONNECT, message.clone()),
        SessionError::Auth(message) => (AMP_RTSP_ERROR_AUTH, message.clone()),
        SessionError::NoH264 => (
            AMP_RTSP_ERROR_NO_H264,
            "The stream has no H.264 video".to_owned(),
        ),
        SessionError::Ended => (
            AMP_RTSP_ERROR_ENDED,
            "The server ended the stream".to_owned(),
        ),
    }
}

/// Starts a session. See the header for the contract.
///
/// # Safety
/// `url` must be null or a valid NUL-terminated string, and `out_client` must
/// be null or valid for writes. The callback context must stay valid until
/// `amp_rtsp_client_stop` returns.
#[no_mangle]
pub unsafe extern "C" fn amp_rtsp_client_start(
    url: *const c_char,
    callbacks: AmpRtspCallbacks,
    out_client: *mut *mut AmpRtspClient,
) -> i32 {
    if out_client.is_null() {
        return AMP_RTSP_ERROR_INVALID_ARGUMENT;
    }
    unsafe { *out_client = std::ptr::null_mut() };
    let result = catch(|| unsafe { start(url, callbacks) });
    match result {
        Ok(Ok(client)) => {
            unsafe { *out_client = Box::into_raw(client) };
            AMP_RTSP_OK
        }
        Ok(Err(code)) => code,
        Err(()) => AMP_RTSP_ERROR_INTERNAL,
    }
}

unsafe fn start(
    url: *const c_char,
    callbacks: AmpRtspCallbacks,
) -> Result<Box<AmpRtspClient>, i32> {
    if url.is_null() {
        return Err(AMP_RTSP_ERROR_INVALID_ARGUMENT);
    }
    let url = unsafe { CStr::from_ptr(url) }
        .to_str()
        .map_err(|_| AMP_RTSP_ERROR_INVALID_ARGUMENT)?;
    let address = address::parse(url).map_err(|error| match error {
        AddressError::Invalid => AMP_RTSP_ERROR_INVALID_URL,
        AddressError::UnsupportedScheme(_) => AMP_RTSP_ERROR_UNSUPPORTED_SCHEME,
    })?;
    let shared = Arc::new(Shared {
        callbacks,
        active: Mutex::new(true),
    });
    let (stop_sender, stop_receiver) = oneshot::channel();
    let (exit_sender, exited) = mpsc::channel();
    let thread_shared = Arc::clone(&shared);
    std::thread::Builder::new()
        .name("amp-rtsp-client".to_owned())
        .spawn(move || {
            // Dropped when the thread ends, which wakes a waiting stop.
            let _exit = exit_sender;
            let outcome = catch(|| run(address, &thread_shared, stop_receiver));
            if outcome.is_err() {
                thread_shared.fail(
                    AMP_RTSP_ERROR_INTERNAL,
                    "The RTSP client stopped unexpectedly",
                );
            }
        })
        .map_err(|_| AMP_RTSP_ERROR_THREAD)?;
    Ok(Box::new(AmpRtspClient {
        shared,
        stop: Some(stop_sender),
        exited,
    }))
}

/// Body of the client thread: a single-threaded Tokio runtime that lives only
/// as long as this session.
fn run(address: address::StreamAddress, shared: &Arc<Shared>, stop: oneshot::Receiver<()>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            shared.fail(
                AMP_RTSP_ERROR_INTERNAL,
                &format!("Could not start the RTSP runtime: {error}"),
            );
            return;
        }
    };
    runtime.block_on(async {
        let group = Arc::new(SessionGroup::default());
        let result = tokio::select! {
            result = session::play(address, Arc::clone(&group), shared.as_ref()) => Some(result),
            _ = stop => None,
        };
        if let Some(Err(error)) = result {
            let (code, message) = error_code_and_message(&error);
            shared.fail(code, &message);
        }
        session::await_teardown(&group).await;
    });
}

/// Stops the session and frees the handle. See the header for the contract.
///
/// # Safety
/// `client` must be null or a handle from `amp_rtsp_client_start` that has not
/// been stopped yet.
#[no_mangle]
pub unsafe extern "C" fn amp_rtsp_client_stop(client: *mut AmpRtspClient) {
    if client.is_null() {
        return;
    }
    let mut client = unsafe { Box::from_raw(client) };
    let _ = catch(move || {
        client.shared.deactivate();
        if let Some(stop) = client.stop.take() {
            let _ = stop.send(());
        }
        // Wait for the session thread to close its connection, so a following
        // start never overlaps this session on a camera that allows one client.
        let _ = client.exited.recv_timeout(STOP_WAIT);
    });
}

/// Longest wait in stop: Retina's TEARDOWN grace plus time to wind down.
pub const STOP_WAIT: Duration = Duration::from_millis(1500);

fn catch<T>(body: impl FnOnce() -> T) -> Result<T, ()> {
    panic::catch_unwind(AssertUnwindSafe(body)).map_err(|_| ())
}
