//! Real WASAPI loopback recording (IMMDeviceEnumerator → IAudioClient LOOPBACK).
//! Gated by SKY_ENABLE_AUDIO=1; treated as high-risk / session-only approval by policy.
//! Output: 24 kHz stereo PCM WAV (resampled from the mix format when needed).

use crate::engine::AudioResult;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct AudioState {
    active: Option<Active>,
}

struct Active {
    path: PathBuf,
    samples: Arc<Mutex<Vec<i16>>>,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl AudioState {
    pub fn start(&mut self, max_ms: u64) -> Result<(), String> {
        if self.active.is_some() {
            return Err("computer audio recording is already active".into());
        }
        let path = std::env::temp_dir().join(format!(
            "ocu-replica-audio-{}.wav",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        ));
        let samples = Arc::new(Mutex::new(Vec::<i16>::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let samples_t = samples.clone();
        let stop_t = stop.clone();
        let max = Duration::from_millis(max_ms);
        let join = std::thread::spawn(move || capture_loopback(samples_t, stop_t, max));
        self.active = Some(Active {
            path,
            samples,
            stop,
            join: Some(join),
        });
        Ok(())
    }

    pub fn stop(&mut self) -> Result<AudioResult, String> {
        let mut active = self.active.take().ok_or("computer audio recording is not active")?;
        active.stop.store(true, Ordering::SeqCst);
        let join_err = active
            .join
            .take()
            .and_then(|h| h.join().ok())
            .and_then(|r| r.err());
        let data = active
            .samples
            .lock()
            .map_err(|_| "audio buffer lock poisoned")?
            .clone();
        if data.is_empty() {
            if let Some(e) = join_err {
                return Err(e);
            }
            return Err("computer audio packet has no data".into());
        }
        write_wav_24k_stereo(&active.path, &data)?;
        Ok(AudioResult {
            filepath: active.path.to_string_lossy().into(),
        })
    }
}

fn capture_loopback(
    samples: Arc<Mutex<Vec<i16>>>,
    stop: Arc<AtomicBool>,
    max: Duration,
) -> Result<(), String> {
    use windows::Win32::Media::Audio::{
        eConsole, eRender, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, WAVEFORMATEX, WAVE_FORMAT_PCM,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
                .map_err(|e| format!("create audio device enumerator: {e}"))?;
        let device = enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .map_err(|e| format!("get default audio output: {e}"))?;
        let client: IAudioClient = device
            .Activate(CLSCTX_ALL, None)
            .map_err(|e| format!("activate default audio output: {e}"))?;

        let mix_ptr = client
            .GetMixFormat()
            .map_err(|e| format!("GetMixFormat: {e}"))?;
        if mix_ptr.is_null() {
            return Err("GetMixFormat returned null".into());
        }
        let mix = *mix_ptr;
        // 100-ns units; 200 ms buffer.
        let buffer_duration: i64 = 2_000_000;
        client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK,
                buffer_duration,
                0,
                mix_ptr,
                None,
            )
            .map_err(|e| {
                CoTaskMemFree(Some(mix_ptr as *const _));
                format!("initialize computer audio capture: {e}")
            })?;

        let capture: IAudioCaptureClient = client
            .GetService()
            .map_err(|e| {
                CoTaskMemFree(Some(mix_ptr as *const _));
                format!("open computer audio capture stream: {e}")
            })?;
        client.Start().map_err(|e| {
            CoTaskMemFree(Some(mix_ptr as *const _));
            format!("start computer audio capture: {e}")
        })?;

        let src_rate = mix.nSamplesPerSec.max(1);
        let src_channels = mix.nChannels.max(1) as usize;
        let bits = mix.wBitsPerSample;
        let tag = mix.wFormatTag as u32;
        let is_float = tag == 3 /* WAVE_FORMAT_IEEE_FLOAT */
            || (tag == 0xFFFE && bits == 32); // WAVE_FORMAT_EXTENSIBLE often float mix
        let is_pcm16 = tag == WAVE_FORMAT_PCM && bits == 16;
        let is_pcm32 = tag == WAVE_FORMAT_PCM && bits == 32;

        let started = Instant::now();
        let mut raw_i16: Vec<i16> = Vec::new();
        // Collect at source rate as interleaved i16 (stereo-normalized later).
        let mut mono_or_multi: Vec<f32> = Vec::new();

        while started.elapsed() < max && !stop.load(Ordering::Relaxed) {
            let packet = match capture.GetNextPacketSize() {
                Ok(p) => p,
                Err(_) => break,
            };
            if packet == 0 {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            let mut data_ptr: *mut u8 = std::ptr::null_mut();
            let mut frames: u32 = 0;
            let mut flags: u32 = 0;
            if capture
                .GetBuffer(&mut data_ptr, &mut frames, &mut flags, None, None)
                .is_err()
            {
                break;
            }
            let silent = (flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32) != 0;
            if !silent && !data_ptr.is_null() && frames > 0 {
                let n = (frames as usize) * src_channels;
                if is_float || (bits == 32 && !is_pcm16) {
                    let slice = std::slice::from_raw_parts(data_ptr as *const f32, n);
                    mono_or_multi.extend_from_slice(slice);
                } else if is_pcm16 {
                    let slice = std::slice::from_raw_parts(data_ptr as *const i16, n);
                    for &s in slice {
                        mono_or_multi.push(s as f32 / 32768.0);
                    }
                } else if is_pcm32 {
                    let slice = std::slice::from_raw_parts(data_ptr as *const i32, n);
                    for &s in slice {
                        mono_or_multi.push(s as f32 / 2147483648.0);
                    }
                } else {
                    // Unknown format — treat as silence for this packet.
                }
            }
            let _ = capture.ReleaseBuffer(frames);
            // Cap ~25 MB float (~6M samples).
            if mono_or_multi.len() > 6_000_000 {
                break;
            }
        }

        let _ = client.Stop();
        CoTaskMemFree(Some(mix_ptr as *const _));

        // Convert to stereo f32 then resample to 24 kHz.
        let stereo_f = to_stereo_f32(&mono_or_multi, src_channels);
        let resampled = resample_linear(&stereo_f, src_rate, 24_000);
        raw_i16.reserve(resampled.len());
        for &s in &resampled {
            let v = (s.clamp(-1.0, 1.0) * 32767.0).round() as i16;
            raw_i16.push(v);
        }

        let mut guard = samples.lock().map_err(|_| "audio buffer lock poisoned")?;
        *guard = raw_i16;
        if guard.is_empty() {
            return Err("computer audio packet has no data".into());
        }
        Ok(())
    }
}

fn to_stereo_f32(interleaved: &[f32], channels: usize) -> Vec<f32> {
    if channels == 0 {
        return Vec::new();
    }
    let frames = interleaved.len() / channels;
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let base = i * channels;
        let l = interleaved[base];
        let r = if channels > 1 {
            interleaved[base + 1]
        } else {
            l
        };
        out.push(l);
        out.push(r);
    }
    out
}

/// Linear resample of interleaved stereo from `src_hz` to `dst_hz`.
fn resample_linear(stereo: &[f32], src_hz: u32, dst_hz: u32) -> Vec<f32> {
    if src_hz == 0 || dst_hz == 0 || stereo.len() < 2 {
        return stereo.to_vec();
    }
    if src_hz == dst_hz {
        return stereo.to_vec();
    }
    let src_frames = stereo.len() / 2;
    let dst_frames = ((src_frames as u64) * (dst_hz as u64) / (src_hz as u64)) as usize;
    let mut out = Vec::with_capacity(dst_frames * 2);
    for i in 0..dst_frames {
        let src_pos = (i as f64) * (src_hz as f64) / (dst_hz as f64);
        let i0 = src_pos.floor() as usize;
        let i1 = (i0 + 1).min(src_frames.saturating_sub(1));
        let t = (src_pos - i0 as f64) as f32;
        let l = stereo[i0 * 2] * (1.0 - t) + stereo[i1 * 2] * t;
        let r = stereo[i0 * 2 + 1] * (1.0 - t) + stereo[i1 * 2 + 1] * t;
        out.push(l);
        out.push(r);
    }
    out
}

fn write_wav_24k_stereo(path: &PathBuf, pcm: &[i16]) -> Result<(), String> {
    let channels: u16 = 2;
    let rate: u32 = 24_000;
    let bits: u16 = 16;
    let data_bytes = (pcm.len() * 2) as u32;
    if data_bytes > 40_000_000 {
        return Err("captured computer audio is too large".into());
    }
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&channels.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    let byte_rate = rate * channels as u32 * (bits as u32 / 8);
    out.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = channels * bits / 8;
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&bits.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).map_err(|e| format!("write captured computer audio: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stereo_and_resample() {
        let mono = vec![0.0f32, 1.0, -1.0, 0.5];
        let st = to_stereo_f32(&mono, 1);
        assert_eq!(st.len(), 8); // 4 frames * 2
        let src: Vec<f32> = (0..100).flat_map(|i| [i as f32 / 100.0, 0.0]).collect();
        let dst = resample_linear(&src, 48_000, 24_000);
        assert!(dst.len() >= 90 && dst.len() <= 110); // ~50 frames * 2
    }

    #[test]
    fn wav_header() {
        let path = std::env::temp_dir().join("ocu-replica-wav-test.wav");
        write_wav_24k_stereo(&path, &[0i16, 0, 1000, -1000]).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        let _ = std::fs::remove_file(path);
    }
}
