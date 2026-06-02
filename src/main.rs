//! birdnet-rs server entrypoint (Leptos fullstack, `ssr`).
//!
//! Starts the realtime audio→inference→storage daemon, then serves the Leptos
//! app, its `#[server]` functions, and the plain axum routes (SSE + media) on
//! the Leptos-configured address.

#[cfg(feature = "ssr")]
async fn server_fn_handler(
    axum::extract::State(state): axum::extract::State<birdnet_rs::server::AppState>,
    req: axum::http::Request<axum::body::Body>,
) -> impl axum::response::IntoResponse {
    leptos_axum::handle_server_fns_with_context(
        move || leptos::prelude::provide_context(state.clone()),
        req,
    )
    .await
}

#[cfg(feature = "ssr")]
#[tokio::main]
async fn main() {
    use axum::Router;
    use axum::routing::get;
    use leptos::logging::log;
    use leptos::prelude::*;
    use leptos_axum::{LeptosRoutes, generate_route_list};

    use birdnet_rs::app::{App, shell};
    use birdnet_rs::config::Settings;
    use birdnet_rs::server::{AppState, ServeState, SseManager, pipeline, routes};
    use birdnet_rs::store;

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,ort=warn,sqlx=warn".into()),
        )
        .init();

    // Config + datastore.
    let config_path =
        std::env::var("BIRDNET_CONFIG").unwrap_or_else(|_| "config.yaml".to_string());
    let settings = Settings::load(std::path::Path::new(&config_path))
        .unwrap_or_else(|e| {
            eprintln!("config error: {e}");
            std::process::exit(1);
        });
    let db = store::connect(&settings.output.sqlite.path)
        .await
        .unwrap_or_else(|e| {
            eprintln!("datastore error: {e}");
            std::process::exit(1);
        });

    let state = AppState {
        db,
        sse: SseManager::new(),
        export_path: settings.realtime.audio.export.path.clone(),
    };

    // Start the realtime pipeline. Non-fatal: if the model/mic is unavailable
    // we still serve the dashboard (the audio monitor just stays idle).
    let _capture = match pipeline::start(&settings, state.clone()) {
        Ok(handle) => Some(handle),
        Err(e) => {
            tracing::warn!("realtime pipeline not started: {e} — serving UI only");
            None
        }
    };

    // Leptos wiring.
    let conf = get_configuration(None).unwrap();
    let leptos_options = conf.leptos_options;
    let addr = leptos_options.site_addr;
    let leptos_routes = generate_route_list(App);
    let serve_state = ServeState { leptos_options: leptos_options.clone(), app: state.clone() };

    let app: Router<()> = Router::new()
        // `#[server]` functions, with our shared state injected as context.
        .route("/api/{*fn_name}", get(server_fn_handler).post(server_fn_handler))
        // Plain routes that don't fit server fns: SSE + binary media.
        .merge(routes::router())
        // Leptos page routes (SSR) — provide the same state as context.
        .leptos_routes_with_context(
            &serve_state,
            leptos_routes,
            {
                let app_state = state.clone();
                move || provide_context(app_state.clone())
            },
            {
                let opts = leptos_options.clone();
                move || shell(opts.clone())
            },
        )
        .fallback(leptos_axum::file_and_error_handler::<ServeState, _>(shell))
        .with_state(serve_state);

    log!("birdnet-rs listening on http://{addr}");
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, app.into_make_service()).await.unwrap();
}

#[cfg(not(feature = "ssr"))]
fn main() {
    // The client entrypoint is `birdnet_rs::hydrate` (see lib.rs).
}
