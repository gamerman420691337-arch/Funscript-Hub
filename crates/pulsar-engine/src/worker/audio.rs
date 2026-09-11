//! Audio effects with explicit source-clock admission. Raw PCM alone is not a timeline.
use super::*;
use serde::{Deserialize, Serialize};
use std::process::Command;

pub(super) const SAMPLE_RATE: u32 = 16_000;

#[derive(Serialize)]
pub(super) struct AudioTimeline {
    first_pts: i64,
    time_base_numerator: u32,
    time_base_denominator: u32,
    source_sample_rate: u32,
    source_samples: u64,
    decoded_frames: usize,
    continuity_tolerance_ticks: u32,
    origin_mapping: &'static str,
    resampled_rate: u32,
}

pub(super) struct DecodedAudio {
    pub samples: Vec<f32>,
    pub timeline: AudioTimeline,
}

#[derive(Deserialize)]
struct Probe {
    streams: Vec<AudioStream>,
    #[serde(default)]
    frames: Vec<AudioFrame>,
}
#[derive(Deserialize)]
struct AudioStream {
    time_base: String,
    sample_rate: String,
}
#[derive(Deserialize)]
struct AudioFrame {
    best_effort_timestamp: serde_json::Value,
    nb_samples: u32,
    #[serde(default)]
    sample_rate: Option<serde_json::Value>,
}

fn unsupported(message: &str) -> anyhow::Error {
    ProtocolError::unsupported(message).into()
}

fn admit_timeline(bytes: &[u8]) -> Result<AudioTimeline> {
    let probe: Probe =
        serde_json::from_slice(bytes).context("invalid decoded audio timeline metadata")?;
    if probe.streams.len() != 1 || probe.frames.is_empty() || probe.frames.len() > 1_000_000 {
        return Err(unsupported(
            "audio requires one bounded decoded stream timeline",
        ));
    }
    let stream = &probe.streams[0];
    let rate: u32 = stream
        .sample_rate
        .parse()
        .context("invalid audio sample rate")?;
    if rate == 0 || rate > MAX_AUDIO_SAMPLE_RATE {
        return Err(unsupported("audio sample rate outside admitted range"));
    }
    let (num, den) = stream
        .time_base
        .split_once('/')
        .context("audio lacks rational time base")?;
    let num: u32 = num.parse()?;
    let den: u32 = den.parse()?;
    if num == 0 || den == 0 || u64::from(num) * 1000 > u64::from(den) {
        return Err(unsupported("audio source clock precision coarser than one millisecond requires an explicit repair policy"));
    }
    let parse_integer = |value: &serde_json::Value| {
        value
            .as_i64()
            .or_else(|| value.as_str().and_then(|text| text.parse().ok()))
            .context("audio frame timestamp is not an integer")
    };
    let first = parse_integer(&probe.frames[0].best_effort_timestamp)?;
    // Preserve exact origin with checked domain arithmetic, including negative
    // source origins. Project synthesis intentionally starts at this origin.
    SourceTimestamp::new(
        first
            .checked_mul(i64::from(num))
            .context("audio origin overflow")?,
        den,
    )?;
    let mut total = 0u64;
    let mut previous: Option<(i64, u32)> = None;
    let scale = i128::from(num) * i128::from(rate);
    for frame in &probe.frames {
        let pts = parse_integer(&frame.best_effort_timestamp)?;
        if frame.nb_samples == 0 || u64::from(frame.nb_samples) > u64::from(rate) * 10 {
            return Err(unsupported(
                "audio frame sample count outside admitted range",
            ));
        }
        if let Some(frame_rate) = &frame.sample_rate {
            if parse_integer(frame_rate)? != i64::from(rate) {
                return Err(unsupported("audio changes sample rate within a stream"));
            }
        }
        let cumulative_error =
            (i128::from(pts) - i128::from(first)) * scale - i128::from(total) * i128::from(den);
        if cumulative_error.abs() > scale {
            return Err(unsupported(
                "audio timestamp gaps, overlaps or clock drift require explicit timeline repair",
            ));
        }
        if let Some((last_pts, last_samples)) = previous {
            let adjacent_error = (i128::from(pts) - i128::from(last_pts)) * scale
                - i128::from(last_samples) * i128::from(den);
            if pts <= last_pts || adjacent_error.abs() > scale {
                return Err(unsupported("audio timestamp gaps, overlaps or non-increasing frames require explicit timeline repair"));
            }
        }
        total = total
            .checked_add(u64::from(frame.nb_samples))
            .context("audio sample count overflow")?;
        previous = Some((pts, frame.nb_samples));
    }
    Ok(AudioTimeline {
        first_pts: first, time_base_numerator: num, time_base_denominator: den,
        source_sample_rate: rate, source_samples: total, decoded_frames: probe.frames.len(),
        continuity_tolerance_ticks: 1, origin_mapping: "subtract first decoded frame PTS; preserve rational origin; reject discontinuities beyond one tick capped at 1ms",
        resampled_rate: SAMPLE_RATE,
    })
}

pub(super) fn decode(request: &WorkerRequest, deadline: Instant) -> Result<DecodedAudio> {
    let mut probe = Command::new(&request.tools.ffprobe.path);
    probe
        .args([
            "-v",
            "error",
            "-threads",
            "1",
            "-protocol_whitelist",
            "file,pipe",
            "-select_streams",
            "a:0",
            "-show_entries",
            "stream=sample_rate,time_base:frame=best_effort_timestamp,nb_samples,sample_rate",
            "-show_frames",
            "-of",
            "json",
        ])
        .arg(&request.source.path);
    let metadata_limit = usize::try_from((request.budget.memory_bytes / 16).min(32 * 1024 * 1024))?;
    let metadata = media::bounded_output(probe, metadata_limit, deadline)?;
    let timeline = admit_timeline(&metadata)?;
    let expected = (u128::from(timeline.source_samples) * u128::from(SAMPLE_RATE))
        .div_ceil(u128::from(timeline.source_sample_rate));
    let limit = (request.budget.memory_bytes / 8).min(MAX_AUDIO_PCM_SAMPLES as u64 * 4);
    if expected == 0
        || expected
            .checked_mul(4)
            .is_none_or(|bytes| bytes > u128::from(limit))
    {
        return Err(ProtocolError::new(
            ErrorCode::ResourceExhausted,
            "decoded audio exceeds immutable memory admission",
        )
        .into());
    }
    let mut command = Command::new(&request.tools.ffmpeg.path);
    command
        .args([
            "-v",
            "error",
            "-nostdin",
            "-threads",
            &request.budget.cpu_threads.to_string(),
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ])
        .arg(&request.source.path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-sn",
            "-dn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-threads",
            "1",
            "-f",
            "f32le",
            "-c:a",
            "pcm_f32le",
            "pipe:1",
        ]);
    let bytes = media::bounded_output(command, usize::try_from(limit)?, deadline)?;
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        bail!("audio decoder returned empty or incomplete PCM samples");
    }
    let actual = bytes.len() as u128 / 4;
    if actual.abs_diff(expected) > 1 {
        return Err(unsupported(
            "decoded sample count differs from admitted source timeline",
        ));
    }
    let samples = bytes
        .chunks_exact(4)
        .map(|v| f32::from_le_bytes([v[0], v[1], v[2], v[3]]))
        .collect();
    Ok(DecodedAudio { samples, timeline })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(pts: &[i64], base: &str, samples: u32) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "streams": [{"sample_rate":"16000", "time_base":base}],
            "frames": pts.iter().map(|pts| serde_json::json!({"best_effort_timestamp":pts,"nb_samples":samples})).collect::<Vec<_>>()
        })).unwrap()
    }
    #[test]
    fn audio_origin_and_integer_sample_clock_remain_explicit() {
        let admitted = admit_timeline(&fixture(&[-160, 0, 160], "1/16000", 160)).unwrap();
        assert_eq!(admitted.first_pts, -160);
        assert_eq!(admitted.source_samples, 480);
    }
    #[test]
    fn audio_internal_gap_overlap_and_coarse_clock_reject_before_decode() {
        for bytes in [
            fixture(&[0, 160, 16320], "1/16000", 160),
            fixture(&[0, 160, 200], "1/16000", 160),
            fixture(&[0, 0, 160], "1/16000", 160),
            fixture(&[0, 1, 2], "1/10", 160),
        ] {
            let error = admit_timeline(&bytes).err().unwrap();
            assert_eq!(
                error.downcast_ref::<ProtocolError>().unwrap().code,
                ErrorCode::Unsupported
            );
        }
    }
    #[test]
    fn audio_tick_rounding_is_bounded_cumulatively_not_per_packet() {
        assert!(admit_timeline(&fixture(&[0, 10, 20], "1/1000", 160)).is_ok());
        assert!(admit_timeline(&fixture(&[0, 11, 22], "1/1000", 160)).is_err());
    }
}
