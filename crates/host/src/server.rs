//! HTTP routes, the WebSocket, and the static web shell.

use std::net::Ipv4Addr;
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;

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
use crate::space::Space;
use crate::space::SpaceId;

const INDEX: &str = include_str!("../web/index.html");
const SHELL_JS: &str = include_str!("../web/shell.js");
const SHELL_CSS: &str = include_str!("../web/shell.css");
const PROTOCOL_JS: &str = include_str!("../web/protocol.js");
/// `flash`, the note line the pane modules share (`../note.js`); widgets must not import it.
const NOTE_JS: &str = include_str!("../web/note.js");
/// The widgets (PROTOCOL.md, DESIGN.md "Widgets"): pane modules import them as
/// `../widgets/<name>.js`, so they resolve under any proxy prefix.
const WIDGETS: [(&str, &str); 6] = [
    (
        "color-swatch.js",
        include_str!("../web/widgets/color-swatch.js"),
    ),
    (
        "number-field.js",
        include_str!("../web/widgets/number-field.js"),
    ),
    (
        "palette-grid.js",
        include_str!("../web/widgets/palette-grid.js"),
    ),
    (
        "property-list.js",
        include_str!("../web/widgets/property-list.js"),
    ),
    ("toolbar.js", include_str!("../web/widgets/toolbar.js")),
    ("tree.js", include_str!("../web/widgets/tree.js")),
];

/// The shell's pages: `/` goes to the browser's own space, `/s/<space>/` is the layout and
/// `/s/<space>/pane/<id>` one pane. A malformed space id is a 400 from the path extractor.
pub async fn serve(
    host: Host,
    port: u16,
) -> Result<()> {
    let app = Router::new()
        .route("/", get(index))
        .route("/s/{space}/", get(space_index))
        .route("/s/{space}/pane/{id}", get(pane_index))
        .route("/shell.js", get(shell_js))
        .route("/shell.css", get(shell_css))
        .route("/protocol.js", get(protocol_js))
        .route("/note.js", get(note_js))
        .route("/widgets/{name}", get(widget))
        .route("/modules/{kind}", get(module))
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

async fn space_index(Path(_space): Path<SpaceId>) -> Html<String> {
    Html(with_base(INDEX, 2))
}

async fn pane_index(Path((_space, _pane)): Path<(SpaceId, PaneId)>) -> Html<String> {
    Html(with_base(INDEX, 3))
}

/// `depth` levels deep, so relative URLs need that many `..` to reach the root, behind any
/// proxy prefix.
fn with_base(
    html: &str,
    depth: usize,
) -> String {
    let base = format!("<head>\n  <base href=\"{}\">", "../".repeat(depth));
    html.replacen("<head>", &base, 1)
}

async fn shell_js() -> Response {
    ([(header::CONTENT_TYPE, "text/javascript")], SHELL_JS).into_response()
}

async fn shell_css() -> Response {
    ([(header::CONTENT_TYPE, "text/css")], SHELL_CSS).into_response()
}

async fn protocol_js() -> Response {
    ([(header::CONTENT_TYPE, "text/javascript")], PROTOCOL_JS).into_response()
}

async fn note_js() -> Response {
    ([(header::CONTENT_TYPE, "text/javascript")], NOTE_JS).into_response()
}

async fn widget(Path(name): Path<String>) -> Response {
    match WIDGETS.iter().find(|(file, _source)| *file == name) {
        Some((_file, source)) => {
            ([(header::CONTENT_TYPE, "text/javascript")], *source).into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
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

#[derive(Deserialize)]
struct WsQuery {
    space: SpaceId,
    /// Subscribe to this pane only (the `/s/<space>/pane/<id>` view).
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
    let space = match host.space(&query.space) {
        Ok(space) => space,
        Err(err) => {
            error!("space {}: {err}", query.space);
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    };
    ws.on_upgrade(move |socket| connection(socket, space, query))
}

/// Numbers the client connections, so a space knows which one drove a pane last.
static CONNECTIONS: AtomicU64 = AtomicU64::new(1);

async fn connection(
    socket: WebSocket,
    space: Space,
    query: WsQuery,
) {
    let conn = CONNECTIONS.fetch_add(1, Ordering::Relaxed);
    serve_connection(socket, &space, &query, conn).await;
    if let Err(err) = space.disconnect(conn) {
        warn!("space {}: {err}", query.space);
    }
}

async fn serve_connection(
    socket: WebSocket,
    space: &Space,
    query: &WsQuery,
    conn: u64,
) {
    let (mut sink, mut stream) = socket.split();
    // Subscribe before snapshotting so nothing between the two is missed.
    let mut rx = space.subscribe();
    for frame in space.snapshot(query.pane) {
        if sink.send(Message::Text(frame.text.into())).await.is_err() {
            return;
        }
    }
    debug!(
        "client joined space={} pane={:?} ro={}",
        query.space,
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
                        if let Err(err) = space.update(conn, pane_id, payload) {
                            warn!("pane {pane_id}: {err}");
                        }
                    }
                    Incoming::Layout { layout: op } => {
                        if let Err(err) = space.apply(op) {
                            // Correct the client's optimistic state.
                            error!("layout: {err}");
                            let text = space.layout_json().to_string();
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
    fn space_pages_point_back_to_root() {
        assert!(with_base(INDEX, 2).contains("<head>\n  <base href=\"../../\">"));
        assert!(with_base(INDEX, 3).contains("<head>\n  <base href=\"../../../\">"));
        assert!(!INDEX.contains("<base"));
        assert!(!INDEX.contains("=\"/"));
    }

    #[test]
    fn widgets_import_only_widgets() {
        for (file, source) in WIDGETS {
            for line in source.lines().filter(|line| line.starts_with("import ")) {
                assert!(line.contains(" from './"), "{file}: {line}");
            }
        }
    }

    #[test]
    fn panes_import_widgets_protocol_and_note() {
        let mut registry = crate::registry::Registry::default();
        registry.add::<pane_clock::Clock>();
        registry.add::<pane_inspector::Inspector>();
        registry.add::<pane_notes::Notes>();
        registry.add::<pane_outliner::Outliner>();
        registry.add::<pane_palette::Palette>();
        registry.add::<pane_view2d::View2d>();
        registry.add::<pane_view3d::View3d>();
        registry.add::<pane_widgets::Widgets>();
        let widgets: Vec<_> = WIDGETS.iter().map(|(file, _source)| *file).collect();
        for kind in registry.kinds() {
            let source = registry.module(kind).unwrap();
            for line in source.lines().filter(|line| line.starts_with("import ")) {
                let ok = line.ends_with(" from '../protocol.js';")
                    || line.ends_with(" from '../note.js';")
                    || widgets
                        .iter()
                        .any(|w| line.ends_with(&format!(" from '../widgets/{w}';")));
                assert!(ok, "{kind}: {line}");
            }
        }
        assert!(NOTE_JS.contains("export function flash("));
        assert!(!NOTE_JS.lines().any(|line| line.starts_with("import ")));
    }
}
