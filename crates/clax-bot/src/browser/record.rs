//! `page.record({seconds})` (spec
//! `docs/superpowers/specs/2026-11-19-claxbot-browser-design.md` §6.6):
//! the page's screencast for 1 to 10 seconds, kept as a frame sequence.
//!
//! [`record`] sends `Page.startScreencast {format: "jpeg", quality: 70,
//! maxWidth, maxHeight}` (both the run's image side), acknowledges every
//! `Page.screencastFrame` with `Page.screencastFrameAck` (Chrome sends the
//! next only once the last is acknowledged), keeps a frame at most every
//! [`MIN_FRAME_GAP_MS`] (8 a second) by the session's clock, and sends
//! `Page.stopScreencast` once the time is up, by the browser source's
//! deadline (no sleep in Clax). Every other event is taken in as between
//! calls, so the page's requests are served while it records.
//!
//! The frames are the JPEG Chrome sent, neither decoded nor encoded. The
//! performer keeps each as an item of the run's content and the clip as
//! one more, of type [`MANIFEST_MEDIA_TYPE`], [`manifest`]'s JSON; the
//! script is answered [`SHOWN`] evenly spaced frames ([`shown`]) to look
//! at.

use super::cdp::CdpFail;
use super::session::Driver;
use base64::Engine as _;
use chrono::Duration;
use serde_json::{Value, json};

/// The fewest seconds a recording takes.
pub const MIN_SECONDS: i64 = 1;
/// The most seconds a recording takes.
pub const MAX_SECONDS: i64 = 10;
/// The least time between two frames kept: at most 8 a second.
pub const MIN_FRAME_GAP_MS: i64 = 125;
/// How many frames the script is answered.
pub const SHOWN: usize = 4;
/// The media type of a clip's manifest.
pub const MANIFEST_MEDIA_TYPE: &str = "application/vnd.clax.frames+json";

/// A recording: its frames (JPEG) with their times, in milliseconds since
/// it began, and how long it took.
#[derive(Debug, Default)]
pub struct Recorded {
    pub frames: Vec<(i64, Vec<u8>)>,
    pub ms: i64,
}

/// Whether a frame at `at_ms` is kept after one kept at `last` (none
/// yet: kept).
#[must_use]
pub fn keeps(last: Option<i64>, at_ms: i64) -> bool {
    last.is_none_or(|l| at_ms - l >= MIN_FRAME_GAP_MS)
}

/// The indices of at most [`SHOWN`] of `n` frames, evenly spaced, the
/// first and the last among them.
#[must_use]
pub fn shown(n: usize) -> Vec<usize> {
    if n <= SHOWN {
        return (0..n).collect();
    }
    (0..SHOWN).map(|i| i * (n - 1) / (SHOWN - 1)).collect()
}

/// A clip's manifest: its size and each frame's item and time.
#[must_use]
pub fn manifest(width: u32, height: u32, frames: &[(String, i64)]) -> Value {
    let frames: Vec<Value> = frames
        .iter()
        .map(|(item, at_ms)| json!({"item": item, "at_ms": at_ms}))
        .collect();
    json!({"width": width, "height": height, "frames": frames})
}

/// Records the page `d` shows for `seconds` (clamped to 1 to 10), its
/// frames at most `side` pixels a side, ending by `timing`'s deadline
/// for the recording's end or the call's, whichever comes first.
///
/// # Errors
/// What ended the call (its deadline before the recording's end, the
/// run's cancellation, the page lost).
pub(super) async fn record(
    d: &mut Driver<'_>,
    timing: &super::act::Timing<'_>,
    seconds: i64,
) -> Result<Recorded, CdpFail> {
    let seconds = seconds.clamp(MIN_SECONDS, MAX_SECONDS);
    let side = timing.limits.image_side;
    let began = timing.clock.now();
    let end = (began + Duration::seconds(seconds)).min(timing.call_until);
    d.page_call(
        "Page.startScreencast",
        json!({"format": "jpeg", "quality": 70, "maxWidth": side, "maxHeight": side}),
    )
    .await?;
    let mut out = Recorded::default();
    let mut last = None;
    loop {
        let event = {
            let timer = timing.source.deadline(end);
            let next = d.cdp.next_event();
            futures::pin_mut!(timer, next);
            match futures::future::select(timer, next).await {
                futures::future::Either::Left(((), _)) => None,
                futures::future::Either::Right((e, _)) => Some(e?),
            }
        };
        let Some(ev) = event else { break };
        if ev.method != "Page.screencastFrame" {
            d.handle(ev).await?;
            continue;
        }
        let now = timing.clock.now();
        let at_ms = (now - began).num_milliseconds().max(0);
        let ack = ev.params["sessionId"].clone();
        let session = d.session.clone();
        d.cdp
            .send_only(
                session.as_deref(),
                "Page.screencastFrameAck",
                json!({"sessionId": ack}),
            )
            .await?;
        if now >= end {
            break;
        }
        if keeps(last, at_ms)
            && let Some(bytes) = ev.params["data"]
                .as_str()
                .and_then(|b| base64::engine::general_purpose::STANDARD.decode(b).ok())
        {
            out.frames.push((at_ms, bytes));
            last = Some(at_ms);
        }
    }
    d.page_call("Page.stopScreencast", json!({})).await?;
    out.ms = (timing.clock.now().min(end) - began)
        .num_milliseconds()
        .max(0);
    Ok(out)
}

#[cfg(test)]
#[warn(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_kept_at_most_eight_a_second() {
        let mut last = None;
        let mut kept = Vec::new();
        for at in (0..1_000).step_by(40) {
            if keeps(last, at) {
                kept.push(at);
                last = Some(at);
            }
        }
        assert_eq!(kept, [0, 160, 320, 480, 640, 800, 960]);
        assert!(keeps(Some(0), 125));
        assert!(!keeps(Some(0), 124));
    }

    #[test]
    fn four_evenly_spaced_frames_are_returned() {
        assert_eq!(shown(0), Vec::<usize>::new());
        assert_eq!(shown(3), [0, 1, 2]);
        assert_eq!(shown(4), [0, 1, 2, 3]);
        assert_eq!(shown(10), [0, 3, 6, 9]);
        assert_eq!(shown(80), [0, 26, 52, 79]);
    }

    #[test]
    fn the_manifest_names_each_frame_and_its_time() {
        let m = manifest(390, 844, &[("c1".into(), 0), ("c2".into(), 130)]);
        assert_eq!(
            m,
            json!({"width": 390, "height": 844, "frames": [
                {"item": "c1", "at_ms": 0}, {"item": "c2", "at_ms": 130}
            ]})
        );
    }
}
