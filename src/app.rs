//! Leptos fullstack app: dashboard UI, `#[server]` query functions, and the
//! client-only live monitor (SSE + canvas). Compiles for both `ssr` and
//! `hydrate`; server-only code is gated so it never reaches the WASM bundle.

use leptos::prelude::*;
use leptos_meta::{MetaTags, Stylesheet, Title, provide_meta_context};
use leptos_router::components::{Route, Router, Routes};
use leptos_router::StaticSegment;
use serde::{Deserialize, Serialize};

/// Backing-store size of the live spectrogram canvas (≈ seconds of history at
/// the ~20 Hz audio-event rate).
const SPECTRO_WIDTH: u32 = 600;
const SPECTRO_HEIGHT: u32 = 160;

/// A detection as sent to the UI — produced both by the [`list_detections`]
/// server function and by the live `detection` SSE event.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DetectionDto {
    pub id: Option<i32>,
    pub common_name: String,
    pub scientific_name: String,
    pub species_code: String,
    pub confidence: f64,
    pub source: String,
    pub clip_name: Option<String>,
    /// `HH:MM:SS` for display.
    pub time: String,
    /// Full timestamp (RFC3339).
    pub timestamp: String,
    /// Review status, if reviewed: `"correct"` or `"false_positive"`.
    pub verified: Option<String>,
}

#[cfg(feature = "ssr")]
impl DetectionDto {
    pub fn from_note(
        n: &crate::store::entities::note::Model,
        review: Option<&crate::store::entities::note_review::Model>,
    ) -> DetectionDto {
        DetectionDto {
            id: Some(n.id),
            common_name: n.common_name.clone(),
            scientific_name: n.scientific_name.clone(),
            species_code: n.species_code.clone(),
            confidence: n.confidence,
            source: n.source.clone(),
            clip_name: n.clip_name.clone(),
            time: n.time.clone(),
            timestamp: n.timestamp.to_rfc3339(),
            verified: review.map(|r| r.verified.clone()),
        }
    }

    pub fn from_detection(d: &crate::Detection) -> DetectionDto {
        DetectionDto {
            id: d.id,
            common_name: d.common_name.clone(),
            scientific_name: d.scientific_name.clone(),
            species_code: d.species_code.clone(),
            confidence: d.confidence as f64,
            source: d.source.clone(),
            clip_name: d.clip_name.clone(),
            time: d.timestamp.format("%H:%M:%S").to_string(),
            timestamp: d.timestamp.to_rfc3339(),
            verified: None,
        }
    }
}

/// Live audio level + spectrum column from the `audio` SSE event.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AudioLevel {
    #[serde(default)]
    pub rms: f32,
    #[serde(default)]
    pub peak: f32,
    #[serde(default)]
    pub spectrum: Vec<f32>,
}

/// Current best-guess species (from the `live` SSE event, pre-threshold).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct LiveGuess {
    pub common_name: String,
    pub scientific_name: String,
    pub confidence: f32,
}

/// One selectable input device: `id` is opened by the backend, `label` is shown.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioDevice {
    pub id: String,
    pub label: String,
}

/// Available capture devices + the active one (by id), its sample rate, and the
/// rates that device supports.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioDevices {
    pub devices: Vec<AudioDevice>,
    pub current: String,
    pub current_rate: u32,
    pub rates: Vec<u32>,
}

/// Build the device/rate snapshot from the running controller (server-side).
#[cfg(feature = "ssr")]
async fn audio_snapshot() -> Result<AudioDevices, ServerFnError> {
    use crate::server::AppState;

    let state = expect_context::<AppState>();
    let Some(ctrl) = state.audio.get() else {
        return Ok(AudioDevices::default());
    };
    let current = ctrl.current();
    let current_rate = ctrl.current_rate();
    let rates = ctrl.supported_rates();
    let pairs = tokio::task::spawn_blocking(crate::audio::list_input_devices)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let devices = pairs
        .into_iter()
        .map(|(id, label)| AudioDevice { id, label })
        .collect();
    Ok(AudioDevices { devices, current, current_rate, rates })
}

/// Apply a device/rate switch on the controller (off the async runtime).
#[cfg(feature = "ssr")]
async fn apply_switch(device: String, rate: u32) -> Result<(), ServerFnError> {
    use crate::server::{AppState, preferences};

    let state = expect_context::<AppState>();
    let audio = state.audio.clone();
    let dev = device.clone();
    tokio::task::spawn_blocking(move || match audio.get() {
        Some(ctrl) => ctrl.switch(&dev, rate),
        None => Err(anyhow::anyhow!("audio pipeline is not running")),
    })
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))?
    .map_err(|e| ServerFnError::new(e.to_string()))?;

    preferences::save(&preferences::Preferences {
        audio_device: Some(device),
        audio_rate: Some(rate),
    })
    .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(())
}

/// Server function: list input devices, the active one, and its rates.
#[server(endpoint = "list_audio_devices")]
pub async fn list_audio_devices() -> Result<AudioDevices, ServerFnError> {
    audio_snapshot().await
}

/// Server function: switch the capture device (keeping the current rate).
#[server(endpoint = "set_audio_device")]
pub async fn set_audio_device(name: String) -> Result<AudioDevices, ServerFnError> {
    let state = expect_context::<crate::server::AppState>();
    let rate = state
        .audio
        .get()
        .map(|c| c.current_rate())
        .filter(|&r| r != 0)
        .unwrap_or(48_000);
    apply_switch(name, rate).await?;
    audio_snapshot().await
}

/// Server function: switch the capture sample rate (keeping the current device).
#[server(endpoint = "set_audio_rate")]
pub async fn set_audio_rate(rate: u32) -> Result<AudioDevices, ServerFnError> {
    let state = expect_context::<crate::server::AppState>();
    let device = state
        .audio
        .get()
        .map(|c| c.current())
        .filter(|d| !d.is_empty())
        .unwrap_or_else(|| "default".to_string());
    apply_switch(device, rate).await?;
    audio_snapshot().await
}

/// Server function: recent detections, newest first.
#[server(endpoint = "list_detections")]
pub async fn list_detections(limit: u32) -> Result<Vec<DetectionDto>, ServerFnError> {
    use crate::server::AppState;
    use crate::store::repo;

    let state = expect_context::<AppState>();
    let rows = repo::recent(&state.db, limit.max(1) as u64, 0)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows
        .iter()
        .map(|(n, r)| DetectionDto::from_note(n, r.as_ref()))
        .collect())
}

/// Server function: search detections by name / species code.
#[server(endpoint = "search_detections")]
pub async fn search_detections(
    query: String,
    limit: u32,
) -> Result<Vec<DetectionDto>, ServerFnError> {
    use crate::server::AppState;
    use crate::store::repo;

    let state = expect_context::<AppState>();
    let rows = repo::search(&state.db, query.trim(), limit.max(1) as u64)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(rows
        .iter()
        .map(|(n, r)| DetectionDto::from_note(n, r.as_ref()))
        .collect())
}

/// Server function: set a detection's review status (`correct` / `false_positive`).
#[server(endpoint = "review_detection")]
pub async fn review_detection(id: i32, verified: String) -> Result<(), ServerFnError> {
    use crate::server::AppState;
    use crate::store::repo;

    if verified != "correct" && verified != "false_positive" {
        return Err(ServerFnError::new("invalid review status"));
    }
    let state = expect_context::<AppState>();
    repo::set_review(&state.db, id, &verified)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Document shell (server-rendered HTML wrapper around the app).
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8" />
                <meta name="viewport" content="width=device-width, initial-scale=1" />
                <AutoReload options=options.clone() />
                <HydrationScripts options />
                <MetaTags />
            </head>
            <body>
                <App />
            </body>
        </html>
    }
}

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    view! {
        <Stylesheet id="leptos" href="/pkg/birdnet-rs.css" />
        <Title text="BirdNET-RS" />
        <Router>
            <Routes fallback=|| "Not found.".into_view()>
                <Route path=StaticSegment("") view=Dashboard />
            </Routes>
        </Router>
    }
}

#[component]
fn Dashboard() -> impl IntoView {
    let detections = RwSignal::new(Vec::<DetectionDto>::new());
    let level = RwSignal::new(AudioLevel::default());
    let connected = RwSignal::new(false);
    let query = RwSignal::new(String::new());
    let guess = RwSignal::new(LiveGuess::default());
    let devices = RwSignal::new(AudioDevices::default());

    // Client-only: load initial data + open the live SSE stream after mount.
    Effect::new(move |_| setup_live(detections, level, connected, guess, devices));

    // Reload the list from the current search query (empty → recent).
    let reload = move || {
        let q = query.get_untracked();
        leptos::task::spawn_local(async move {
            let res = if q.trim().is_empty() {
                list_detections(50).await
            } else {
                search_detections(q, 50).await
            };
            if let Ok(list) = res {
                detections.set(list);
            }
        });
    };

    view! {
        <header>
            <h1>"BirdNET-RS"</h1>
            <span class="sub">"realtime soundscape detections"</span>
            <span id="status" class:live=move || connected.get()>
                {move || if connected.get() { "live" } else { "connecting…" }}
            </span>
        </header>
        <main>
            <section class="panel">
                <div class="panel-head">
                    <h2>"Live audio monitor"</h2>
                    <select
                        class="device"
                        title="Input device"
                        prop:value=move || devices.get().current
                        on:change=move |ev| {
                            let name = event_target_value(&ev);
                            leptos::task::spawn_local(async move {
                                if let Ok(d) = set_audio_device(name).await {
                                    devices.set(d);
                                }
                            });
                        }
                    >
                        <For each=move || devices.get().devices key=|d| d.id.clone() let:d>
                            <option value=d.id>{d.label}</option>
                        </For>
                    </select>
                    <select
                        class="device rate"
                        title="Sample rate"
                        prop:value=move || devices.get().current_rate.to_string()
                        on:change=move |ev| {
                            if let Ok(rate) = event_target_value(&ev).parse::<u32>() {
                                leptos::task::spawn_local(async move {
                                    if let Ok(d) = set_audio_rate(rate).await {
                                        devices.set(d);
                                    }
                                });
                            }
                        }
                    >
                        <For each=move || devices.get().rates key=|r| *r let:r>
                            <option value=r.to_string()>{format!("{} Hz", r)}</option>
                        </For>
                    </select>
                </div>
                <div class="monitor">
                    <div class="guess">
                        {move || {
                            let g = guess.get();
                            if g.common_name.is_empty() {
                                "Listening…".to_string()
                            } else {
                                format!(
                                    "Closest match: {} ({}) — {:.0}%",
                                    g.common_name, g.scientific_name, g.confidence * 100.0,
                                )
                            }
                        }}
                    </div>
                    <div class="vu-row">
                        <span class="vu-label">"RMS"</span>
                        <div class="vu">
                            <div class="vu-fill"
                                style:width=move || format!("{:.1}%", db_pct(level.get().rms))></div>
                        </div>
                    </div>
                    <div class="vu-row">
                        <span class="vu-label">"Peak"</span>
                        <div class="vu">
                            <div class="vu-fill"
                                style:width=move || format!("{:.1}%", db_pct(level.get().peak))></div>
                        </div>
                    </div>
                    <canvas id="live-spectro" width=SPECTRO_WIDTH height=SPECTRO_HEIGHT></canvas>
                    <div class="hint">
                        "Live spectrogram of the audio input (scrolls right→left). Movement here means the mic is delivering audio."
                    </div>
                </div>
            </section>

            <section class="panel">
                <div class="panel-head">
                    <h2>"Detections"</h2>
                    <input
                        class="search"
                        type="search"
                        placeholder="Search species or code…"
                        on:input=move |ev| {
                            query.set(event_target_value(&ev));
                            reload();
                        }
                    />
                </div>
                <table>
                    <thead>
                        <tr>
                            <th>"Time"</th><th>"Species"</th><th>"Confidence"</th>
                            <th>"Source"</th><th>"Spectrogram"</th><th>"Clip"</th><th>"Review"</th>
                        </tr>
                    </thead>
                    <tbody>
                        <For
                            each=move || detections.get()
                            key=|d| (d.id, d.timestamp.clone())
                            let:d
                        >
                            {detection_row(d, detections)}
                        </For>
                    </tbody>
                </table>
                <Show when=move || detections.get().is_empty()>
                    <div class="empty">"No detections yet — waiting for birds…"</div>
                </Show>
            </section>
        </main>
    }
}

/// Update a detection's review status both on the server and in the local list.
fn submit_review(id: i32, status: &'static str, detections: RwSignal<Vec<DetectionDto>>) {
    leptos::task::spawn_local(async move {
        if review_detection(id, status.to_string()).await.is_ok() {
            detections.update(|v| {
                if let Some(d) = v.iter_mut().find(|d| d.id == Some(id)) {
                    d.verified = Some(status.to_string());
                }
            });
        }
    });
}

/// Render one detection row.
fn detection_row(d: DetectionDto, detections: RwSignal<Vec<DetectionDto>>) -> impl IntoView {
    let pct = (d.confidence * 100.0).round() as i32;
    let id = d.id;
    let has_clip = d.clip_name.is_some() && id.is_some();
    let verified = d.verified.clone();
    let row_class = match verified.as_deref() {
        Some("correct") => "reviewed-correct",
        Some("false_positive") => "reviewed-false",
        _ => "flash",
    };
    view! {
        <tr class=row_class>
            <td>{d.time.clone()}</td>
            <td>
                <div>{d.common_name.clone()}</div>
                <div class="sci">{d.scientific_name.clone()}</div>
            </td>
            <td class="conf">
                {format!("{pct}%")}
                <div class="bar"><span style:width=move || format!("{pct}%")></span></div>
            </td>
            <td>{d.source.clone()}</td>
            <td>
                {has_clip.then(|| view! {
                    <img class="spectro-thumb" loading="lazy"
                        src=format!("/media/spectrogram/{}", id.unwrap()) />
                })}
            </td>
            <td>
                {has_clip.then(|| view! {
                    <audio controls preload="none"
                        src=format!("/media/clip/{}", id.unwrap())></audio>
                })}
            </td>
            <td class="review">
                {id.map(|id| view! {
                    <button class="ok" title="Correct"
                        on:click=move |_| submit_review(id, "correct", detections)>"✓"</button>
                    <button class="no" title="False positive"
                        on:click=move |_| submit_review(id, "false_positive", detections)>"✗"</button>
                })}
                <span class="verdict">{match verified.as_deref() {
                    Some("correct") => "✓",
                    Some("false_positive") => "✗",
                    _ => "",
                }}</span>
            </td>
        </tr>
    }
}

/// Amplitude `[0,1]` → bar width percent on a -60..0 dBFS scale.
fn db_pct(amp: f32) -> f32 {
    if amp <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * amp.log10();
    (((db + 60.0) / 60.0) * 100.0).clamp(0.0, 100.0)
}

// ---------------------------------------------------------------------------
// Client-only live wiring (SSE + canvas). No-op on the server.
// ---------------------------------------------------------------------------

#[cfg(not(feature = "hydrate"))]
fn setup_live(
    _: RwSignal<Vec<DetectionDto>>,
    _: RwSignal<AudioLevel>,
    _: RwSignal<bool>,
    _: RwSignal<LiveGuess>,
    _: RwSignal<AudioDevices>,
) {
}

#[cfg(feature = "hydrate")]
fn setup_live(
    detections: RwSignal<Vec<DetectionDto>>,
    level: RwSignal<AudioLevel>,
    connected: RwSignal<bool>,
    guess: RwSignal<LiveGuess>,
    devices: RwSignal<AudioDevices>,
) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;
    use web_sys::{Event, EventSource, MessageEvent};

    // Initial load via server functions.
    leptos::task::spawn_local(async move {
        if let Ok(list) = list_detections(50).await {
            detections.set(list);
        }
    });
    leptos::task::spawn_local(async move {
        if let Ok(d) = list_audio_devices().await {
            devices.set(d);
        }
    });

    let Ok(es) = EventSource::new("/stream") else {
        return;
    };

    let det_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(txt) = e.data().as_string()
            && let Ok(d) = serde_json::from_str::<DetectionDto>(&txt)
        {
            detections.update(|v| {
                v.insert(0, d);
                v.truncate(200);
            });
        }
    });
    let _ = es.add_event_listener_with_callback("detection", det_cb.as_ref().unchecked_ref());
    det_cb.forget();

    let audio_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(txt) = e.data().as_string()
            && let Ok(a) = serde_json::from_str::<AudioLevel>(&txt)
        {
            draw_spectrum_column(&a.spectrum);
            level.set(a);
        }
    });
    let _ = es.add_event_listener_with_callback("audio", audio_cb.as_ref().unchecked_ref());
    audio_cb.forget();

    let live_cb = Closure::<dyn FnMut(MessageEvent)>::new(move |e: MessageEvent| {
        if let Some(txt) = e.data().as_string()
            && let Ok(g) = serde_json::from_str::<LiveGuess>(&txt)
        {
            guess.set(g);
        }
    });
    let _ = es.add_event_listener_with_callback("live", live_cb.as_ref().unchecked_ref());
    live_cb.forget();

    let open_cb = Closure::<dyn FnMut(Event)>::new(move |_| connected.set(true));
    es.set_onopen(Some(open_cb.as_ref().unchecked_ref()));
    open_cb.forget();

    let err_cb = Closure::<dyn FnMut(Event)>::new(move |_| connected.set(false));
    es.set_onerror(Some(err_cb.as_ref().unchecked_ref()));
    err_cb.forget();

    std::mem::forget(es); // keep the stream open for the app's lifetime
}

/// Inferno-like colormap matching the server-side clip spectrogram.
#[cfg(feature = "hydrate")]
fn colormap(v: f32) -> [u8; 3] {
    const STOPS: [[f32; 3]; 5] = [
        [0.0, 0.0, 0.0],
        [0.30, 0.06, 0.43],
        [0.73, 0.21, 0.33],
        [0.98, 0.55, 0.04],
        [0.99, 0.99, 0.75],
    ];
    let v = v.clamp(0.0, 1.0);
    let scaled = v * (STOPS.len() - 1) as f32;
    let i = (scaled.floor() as usize).min(STOPS.len() - 2);
    let t = scaled - i as f32;
    let c = |a: f32, b: f32| ((a + (b - a) * t) * 255.0).round().clamp(0.0, 255.0) as u8;
    [
        c(STOPS[i][0], STOPS[i + 1][0]),
        c(STOPS[i][1], STOPS[i + 1][1]),
        c(STOPS[i][2], STOPS[i + 1][2]),
    ]
}

/// Scroll the live spectrogram left 1px and draw the newest spectrum column.
#[cfg(feature = "hydrate")]
fn draw_spectrum_column(spectrum: &[f32]) {
    use wasm_bindgen::JsCast;
    use web_sys::{CanvasRenderingContext2d, HtmlCanvasElement};

    let Some(canvas) = leptos::prelude::window()
        .document()
        .and_then(|d| d.get_element_by_id("live-spectro"))
        .and_then(|e| e.dyn_into::<HtmlCanvasElement>().ok())
    else {
        return;
    };
    let Ok(Some(ctx)) = canvas.get_context("2d") else {
        return;
    };
    let Ok(ctx) = ctx.dyn_into::<CanvasRenderingContext2d>() else {
        return;
    };

    let w = canvas.width() as f64;
    let h = canvas.height() as f64;
    let _ = ctx.draw_image_with_html_canvas_element(&canvas, -1.0, 0.0);

    let n = spectrum.len().max(1);
    let bin_h = h / n as f64;
    for (i, &v) in spectrum.iter().enumerate() {
        let [r, g, b] = colormap(v);
        ctx.set_fill_style_str(&format!("rgb({r},{g},{b})"));
        let y = h - (i as f64 + 1.0) * bin_h;
        ctx.fill_rect(w - 1.0, y, 1.0, bin_h.ceil());
    }
}
