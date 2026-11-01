//! HTTP routes, the WebSocket, and the static web shell.

use std::net::Ipv4Addr;

use axum::Router;
use axum::extract::Path;
use axum::extract::Query;
use axum::extract::State;
use axum::extract::ws::Message;
use axum::extract::ws::WebSocket;
use axum::extract::ws::WebSocketUpgrade;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::Html;
use axum::response::IntoResponse;
use axum::response::Json;
use axum::response::Response;
use axum::routing::get;
use futures_util::SinkExt;
use futures_util::StreamExt;
use log::debug;
use log::error;
use log::info;
use log::warn;
use pane_api::PaneId;
use serde::Deserialize;
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;

use crate::Result;
use crate::host::Host;
use crate::layout::Op;

const INDEX: &str = include_str!("../web/index.html");
const SHELL_JS: &str = include_str!("../web/shell.js");
const SHELL_CSS: &str = include_str!("../web/shell.css");

pub async fn serve(
    host: Host,
    port: u16,
) -> Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/pane/{id}", get(pane_index))
        .route("/shell.js", get(shell_js))
        .route("/shell.css", get(shell_css))
        .route("/modules/{kind}", get(module))
        .route("/layout", get(layout))
        .route("/ws", get(ws))
        .with_state(host);
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    info!("http://{}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<String> {
    Html(INDEX.to_string())
}

/// One level deep, so relative URLs need `..` to reach the root, behind any proxy prefix.
async fn pane_index() -> Html<String> {
    Html(with_base(INDEX))
}

fn with_base(html: &str) -> String {
    html.replacen("<head>", "<head>\n  <base href=\"../\">", 1)
}

async fn shell_js() -> Response {
    ([(header::CONTENT_TYPE, "text/javascript")], SHELL_JS).into_response()
}

async fn shell_css() -> Response {
    ([(header::CONTENT_TYPE, "text/css")], SHELL_CSS).into_response()
}

async fn module(
    State(host): State<Host>,
    Path(kind): Path<String>,
) -> Response {
    match host.module(&kind) {
        Some(source) => ([(header::CONTENT_TYPE, "text/javascript")], source).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn layout(State(host): State<Host>) -> Json<Value> {
    Json(host.layout_json())
}

#[derive(Deserialize)]
struct WsQuery {
    /// Subscribe to this pane only (the `/pane/<id>` view).
    pane: Option<PaneId>,
    /// Read-only: client -> pane messages and layout ops are dropped. `?ro=1` or `?ro=true`.
    ro: Option<String>,
}
impl WsQuery {
    fn read_only(&self) -> bool {
        self.ro
            .as_deref()
            .is_some_and(|ro| ro != "0" && ro != "false")
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Incoming {
    Pane { pane_id: PaneId, payload: Value },
    Layout { layout: Op },
}

async fn ws(
    ws: WebSocketUpgrade,
    State(host): State<Host>,
    Query(query): Query<WsQuery>,
) -> Response {
    ws.on_upgrade(move |socket| connection(socket, host, query))
}

async fn connection(
    socket: WebSocket,
    host: Host,
    query: WsQuery,
) {
    let (mut sink, mut stream) = socket.split();
    // Subscribe before snapshotting so nothing between the two is missed.
    let mut rx = host.subscribe();
    for frame in host.snapshot(query.pane) {
        if sink.send(Message::Text(frame.text.into())).await.is_err() {
            return;
        }
    }
    debug!(
        "client joined pane={:?} ro={}",
        query.pane,
        query.read_only()
    );
    loop {
        tokio::select! {
            frame = rx.recv() => {
                let frame = match frame {
                    Ok(frame) => frame,
                    Err(RecvError::Lagged(n)) => {
                        warn!("client lagged {n} frames, closing");
                        return;
                    }
                    Err(RecvError::Closed) => return,
                };
                if let (Some(only), Some(pane)) = (query.pane, frame.pane)
                    && only != pane
                {
                    continue;
                }
                if sink.send(Message::Text(frame.text.into())).await.is_err() {
                    return;
                }
            }
            msg = stream.next() => {
                let Some(Ok(Message::Text(text))) = msg else {
                    if let Some(Ok(_)) = msg { continue; }
                    return;
                };
                let incoming = match serde_json::from_str(&text) {
                    Ok(incoming) => incoming,
                    Err(err) => {
                        warn!("bad frame: {err}");
                        continue;
                    }
                };
                if query.read_only() {
                    debug!("read-only client, dropped");
                    continue;
                }
                match incoming {
                    Incoming::Pane { pane_id, payload } => {
                        if let Err(err) = host.update(pane_id, payload) {
                            warn!("pane {pane_id}: {err}");
                        }
                    }
                    Incoming::Layout { layout: op } => {
                        if let Err(err) = host.apply(op) {
                            // Correct the client's optimistic state.
                            error!("layout: {err}");
                            let text = host.layout_json().to_string();
                            if sink.send(Message::Text(text.into())).await.is_err() {
                                return;
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_page_points_back_to_root() {
        let html = with_base(INDEX);
        assert!(html.contains("<head>\n  <base href=\"../\">"));
        assert!(!INDEX.contains("<base"));
        assert!(!INDEX.contains("=\"/"));
    }
}
