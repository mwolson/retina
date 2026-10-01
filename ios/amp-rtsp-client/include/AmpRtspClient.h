// C interface to the AMP RTSP client, a small Rust wrapper around the Retina
// RTSP library. It receives one H.264 video stream over RTSP (TCP
// interleaved) and hands the app H.264 access units in AVCC form (each NAL
// unit prefixed by a 4-byte big-endian length), plus the SPS and PPS needed to
// build a CMVideoFormatDescription.
//
// Threading: each client runs on its own background thread. Every callback
// arrives on that thread, never on the caller's thread. Callbacks for one
// client never overlap. Pointers passed to a callback are valid only until the
// callback returns; copy what you need. amp_rtsp_client_stop waits for a
// callback that is already running to return, so a callback must not block on
// the thread that calls stop, or call stop itself. Once stop returns, no
// further callback runs and the context pointer is no longer used.
//
// Callbacks also carry RTP loss: a frame with loss > 0 followed a gap and may
// reference data that never arrived.

#ifndef AMP_RTSP_CLIENT_H
#define AMP_RTSP_CLIENT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define AMP_RTSP_OK 0
// Synchronous errors from amp_rtsp_client_start.
#define AMP_RTSP_ERROR_INVALID_ARGUMENT 1
#define AMP_RTSP_ERROR_INVALID_URL 2
#define AMP_RTSP_ERROR_UNSUPPORTED_SCHEME 3
#define AMP_RTSP_ERROR_THREAD 4
// Errors delivered through on_error.
#define AMP_RTSP_ERROR_CONNECT 10
#define AMP_RTSP_ERROR_AUTH 11
#define AMP_RTSP_ERROR_NO_H264 12
#define AMP_RTSP_ERROR_ENDED 13
#define AMP_RTSP_ERROR_INTERNAL 20

typedef struct AmpRtspClient AmpRtspClient;

// Sequence and picture parameter sets without start codes or length prefixes.
// Delivered before the first frame and again whenever they change.
typedef struct AmpRtspParameters {
    const uint8_t *sps;
    size_t sps_len;
    const uint8_t *pps;
    size_t pps_len;
    uint32_t width;
    uint32_t height;
} AmpRtspParameters;

// One H.264 access unit in AVCC form. In-band SPS and PPS are removed and
// reported through on_parameters instead.
typedef struct AmpRtspFrame {
    const uint8_t *data;
    size_t len;
    // RTP timestamp extended to 64 bits, in clock_rate units (90 kHz for H.264).
    int64_t timestamp;
    uint32_t clock_rate;
    // RTP packets lost since the previous frame.
    uint16_t loss;
    // True for an IDR picture that decodes without earlier frames.
    bool is_keyframe;
} AmpRtspFrame;

typedef struct AmpRtspCallbacks {
    void *context;
    void (*on_parameters)(void *context, const AmpRtspParameters *parameters);
    void (*on_frame)(void *context, const AmpRtspFrame *frame);
    // Terminal: the session is over and no other callback follows. The message
    // is UTF-8 and never contains the URL's credentials.
    void (*on_error)(void *context, int32_t code, const char *message);
} AmpRtspCallbacks;

// Starts playing an rtsp:// URL. Credentials in the URL's userinfo are
// percent-decoded and used for Basic or Digest authentication; they are not
// sent in the request URL. On success returns AMP_RTSP_OK and stores a handle
// in out_client, which must be released with amp_rtsp_client_stop. On failure
// returns an error code, stores NULL and never calls a callback.
int32_t amp_rtsp_client_start(const char *url, AmpRtspCallbacks callbacks, AmpRtspClient **out_client);

// Stops the session, sends a best-effort TEARDOWN and frees the handle. Blocks
// until the session thread has closed its connection, for at most about 1.5
// seconds, so the next session on the same camera never overlaps this one.
// Call it off the main thread. Safe to call with NULL. Must be called exactly
// once per handle, including after on_error.
void amp_rtsp_client_stop(AmpRtspClient *client);

#ifdef __cplusplus
}
#endif

#endif
