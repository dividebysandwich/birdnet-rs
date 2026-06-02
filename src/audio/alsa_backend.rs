//! Direct ALSA capture (Linux). Unlike cpal — which only surfaces the
//! high-level PCM plugins (`default`, `pulse`, `pipewire`, `jack`) — this lets
//! the UI pick the *actual* input: a specific PipeWire/Pulse source, or a
//! hardware card on plain-ALSA systems. The selected device id is an ALSA PCM
//! name opened directly (e.g. `pulse:<source>`, `sysdefault:CARD=...`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use alsa::pcm::{Access, Format, HwParams, PCM};
use alsa::{Direction, ValueOr};
use tokio::sync::mpsc;

use super::resample::Resampler48k;
use super::{AudioFrame, CaptureHandle, downmix};

/// Input devices as `(pcm_name, label)`. On PipeWire/Pulse systems this lists
/// the real sources; on plain ALSA it lists hardware cards.
pub fn list_devices() -> Vec<(String, String)> {
    let mut out = vec![("default".to_string(), "System default".to_string())];

    let sources = pulse_sources();
    if sources.is_empty() {
        // Plain-ALSA system: expose hardware capture devices.
        out.extend(alsa_hardware());
    } else {
        out.extend(sources);
    }
    out
}

/// Query PipeWire/Pulse input sources via `pactl` (monitors excluded).
fn pulse_sources() -> Vec<(String, String)> {
    let Ok(output) = std::process::Command::new("pactl").args(["list", "sources"]).output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&output.stdout);

    let mut res = Vec::new();
    let mut name: Option<String> = None;
    let mut desc: Option<String> = None;
    for line in text.lines() {
        if line.starts_with("Source #") {
            push_source(&mut name, &mut desc, &mut res);
        } else {
            let t = line.trim();
            if let Some(v) = t.strip_prefix("Name:") {
                name = Some(v.trim().to_string());
            } else if let Some(v) = t.strip_prefix("Description:") {
                desc = Some(v.trim().to_string());
            }
        }
    }
    push_source(&mut name, &mut desc, &mut res);
    res
}

fn push_source(name: &mut Option<String>, desc: &mut Option<String>, res: &mut Vec<(String, String)>) {
    let d = desc.take();
    if let Some(n) = name.take()
        && !n.ends_with(".monitor")
    {
        let label = d.unwrap_or_else(|| n.clone());
        res.push((format!("pulse:{n}"), label));
    }
}

/// Hardware capture PCMs from ALSA hints (used when no Pulse/PipeWire server).
fn alsa_hardware() -> Vec<(String, String)> {
    let mut res = Vec::new();
    let Ok(hints) = alsa::device_name::HintIter::new_str(None, "pcm") else {
        return res;
    };
    for hint in hints {
        let Some(name) = hint.name else { continue };
        // Capture-capable (direction None means both).
        if matches!(hint.direction, Some(Direction::Playback)) {
            continue;
        }
        if name.contains("CARD=")
            && (name.starts_with("sysdefault:") || name.starts_with("hw:") || name.starts_with("plughw:"))
        {
            let label = hint
                .desc
                .map(|d| d.replace('\n', " — "))
                .unwrap_or_else(|| name.clone());
            res.push((name, label));
        }
    }
    res
}

/// Start capturing into `tx`. Capture continues until the [`CaptureHandle`] drops.
pub fn start_into(
    device: &str,
    tx: mpsc::UnboundedSender<AudioFrame>,
) -> anyhow::Result<CaptureHandle> {
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<anyhow::Result<()>>();
    let name = device.to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    let handle = std::thread::Builder::new()
        .name("birdnet-capture-alsa".into())
        .spawn(move || match open_capture(&name) {
            Ok((pcm, rate, channels)) => {
                tracing::info!("capturing from '{name}': {rate} Hz, {channels} ch (ALSA)");
                let _ = ready_tx.send(Ok(()));
                capture_loop(&pcm, rate, channels, &name, &tx, &stop_thread);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        })?;

    ready_rx
        .recv()
        .map_err(|_| anyhow::anyhow!("capture thread exited before start"))??;

    Ok(CaptureHandle::from_parts(stop, handle))
}

/// Open `name` for capture, requesting 48 kHz S16; returns the negotiated rate
/// and channel count.
fn open_capture(name: &str) -> anyhow::Result<(PCM, u32, usize)> {
    let pcm = PCM::new(name, Direction::Capture, false)
        .map_err(|e| anyhow::anyhow!("opening input '{name}': {e}"))?;
    {
        let hwp = HwParams::any(&pcm)?;
        hwp.set_access(Access::RWInterleaved)?;
        hwp.set_format(Format::s16())?;
        hwp.set_rate_near(48_000, ValueOr::Nearest)?;
        let _ = hwp.set_channels_near(1); // prefer mono; downmix otherwise
        pcm.hw_params(&hwp)?;
    }
    let (rate, channels) = {
        let hwp = pcm.hw_params_current()?;
        (hwp.get_rate()?, (hwp.get_channels()? as usize).max(1))
    };
    pcm.prepare()?;
    Ok((pcm, rate, channels))
}

fn capture_loop(
    pcm: &PCM,
    rate: u32,
    channels: usize,
    source: &str,
    tx: &mpsc::UnboundedSender<AudioFrame>,
    stop: &AtomicBool,
) {
    let io = match pcm.io_i16() {
        Ok(io) => io,
        Err(e) => {
            tracing::error!("ALSA io setup failed: {e}");
            return;
        }
    };
    let mut resampler = match Resampler48k::new(rate) {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("resampler init failed: {e}");
            return;
        }
    };

    const PERIOD: usize = 1024;
    let mut buf = vec![0i16; PERIOD * channels];

    while !stop.load(Ordering::Relaxed) {
        match io.readi(&mut buf) {
            Ok(frames) if frames > 0 => {
                let n = frames * channels;
                let f: Vec<f32> = buf[..n].iter().map(|&s| s as f32 / 32768.0).collect();
                let mono = downmix(&f, channels);
                match resampler.push(&mono) {
                    Ok(samples) if !samples.is_empty() => {
                        if tx.send(AudioFrame { source: source.to_string(), samples }).is_err() {
                            break; // consumer gone
                        }
                    }
                    Ok(_) => {}
                    Err(e) => tracing::warn!("resample error: {e}"),
                }
            }
            Ok(_) => {}
            Err(e) => {
                if pcm.try_recover(e, true).is_err() {
                    tracing::error!("ALSA capture stopped: {e}");
                    break;
                }
            }
        }
    }
}
