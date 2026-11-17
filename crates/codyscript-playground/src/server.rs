//! Serves the page, the checklist and the module, from memory, on
//! 127.0.0.1 only.

use axum::Router;
use axum::body::Bytes;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::IntoResponse;
use axum::routing::{MethodRouter, get};

use crate::cases;
use crate::wasm::Module;

const PAGE: &str = include_str!("index.html");

pub fn serve(port: u16, module: Module) -> Result<(), String> {
    let router = Router::new()
        .route("/", get_file("text/html; charset=utf-8", PAGE.into()))
        .route(
            "/cases.json",
            get_file("application/json", cases::json().into()),
        )
        .route(
            "/pkg/codyscript_wasm.js",
            get_file("text/javascript", module.js.into()),
        )
        .route(
            "/pkg/codyscript_wasm_bg.wasm",
            get_file("application/wasm", module.wasm.into()),
        );

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| format!("starting the runtime: {e}"))?;
    runtime.block_on(async {
        let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(|e| format!("can't listen on {address}: {e}"))?;
        println!("CodyScript playground: http://{address}  (Ctrl+C to stop)");
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = tokio::signal::ctrl_c().await;
            })
            .await
            .map_err(|e| e.to_string())
    })
}

/// A route answering GET with `body`. Never cached: a restart may bring a
/// new build.
fn get_file(content_type: &'static str, body: Bytes) -> MethodRouter {
    get(move || {
        let body = body.clone();
        async move {
            (
                [(CONTENT_TYPE, content_type), (CACHE_CONTROL, "no-store")],
                body,
            )
                .into_response()
        }
    })
}
