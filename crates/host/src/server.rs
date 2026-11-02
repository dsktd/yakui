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
use pane_api::protocol::VERSION;
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
/// Where `protocol.js` carries the protocol version; `protocol_source` fills it in.
const VERSION_MARK: &str = "/*@VERSION*/ 0";
/// Where `index.html` names the shared modules' directory; `index_html` fills it in.
const LIB_MARK: &str = "@LIB@";
const PROTOCOL_JS: &str = include_str!("../web/protocol.js");
/// `flash`, the note line the pane modules share (`yakui/note.js`); widgets must not import it.
const NOTE_JS: &str = include_str!("../web/note.js");
/// The widgets (PROTOCOL.md "Shared web modules", DESIGN.md "Widgets"): pane modules import
/// them as `yakui/widgets/<name>.js`, which the shell's import map points at `lib/v<VERSION>/`.
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
        .route("/lib/v{version}/protocol.js", get(protocol_js))
        .route("/lib/v{version}/note.js", get(note_js))
        .route("/lib/v{version}/widgets/{name}", get(widget))
        // One release of the pre-versioning paths: a cached page still gets its modules.
        .route("/protocol.js", get(legacy_protocol_js))
        .route("/note.js", get(legacy_note_js))
        .route("/widgets/{name}", get(legacy_widget))
        .route("/modules/{kind}", get(module))
        .route("/ws", get(ws))
        .with_state(host);
    let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).await?;
    info!("http://{}", listener.local_addr()?);
    axum::serve(listener, app).await?;
    Ok(())
}

async fn index() -> Html<String> {
    Html(index_html())
}

async fn space_index(Path(_space): Path<SpaceId>) -> Html<String> {
    Html(with_base(&index_html(), 2))
}

async fn pane_index(Path((_space, _pane)): Path<(SpaceId, PaneId)>) -> Html<String> {
    Html(with_base(&index_html(), 3))
}

/// The shell page with its import map pointing `yakui/` at this host's `lib/v<VERSION>/`.
fn index_html() -> String {
    INDEX.replace(LIB_MARK, &format!("lib/v{VERSION}/"))
}

/// `protocol.js` as served: the version it reports is the one the route carries.
fn protocol_source() -> String {
    PROTOCOL_JS.replace(VERSION_MARK, &VERSION.to_string())
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

/// A `/lib/v<n>/` of any other version than this host's is a 404: the page asked for a
/// contract this host does not speak.
fn is_current(version: &str) -> bool {
    version == VERSION.to_string()
}

async fn protocol_js(Path(version): Path<String>) -> Response {
    if !is_current(&version) {
        return StatusCode::NOT_FOUND.into_response();
    }
    (
        [(header::CONTENT_TYPE, "text/javascript")],
        protocol_source(),
    )
        .into_response()
}

async fn note_js(Path(version): Path<String>) -> Response {
    if !is_current(&version) {
        return StatusCode::NOT_FOUND.into_response();
    }
    ([(header::CONTENT_TYPE, "text/javascript")], NOTE_JS).into_response()
}

/// The old unversioned paths redirect (relative, so a proxy prefix survives).
fn moved(to: String) -> Response {
    (StatusCode::TEMPORARY_REDIRECT, [(header::LOCATION, to)]).into_response()
}

async fn legacy_protocol_js() -> Response {
    moved(format!("lib/v{VERSION}/protocol.js"))
}

async fn legacy_note_js() -> Response {
    moved(format!("lib/v{VERSION}/note.js"))
}

async fn legacy_widget(Path(name): Path<String>) -> Response {
    moved(format!("../lib/v{VERSION}/widgets/{name}"))
}

async fn widget(Path((version, name)): Path<(String, String)>) -> Response {
    if !is_current(&version) {
        return StatusCode::NOT_FOUND.into_response();
    }
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
        // Wake only while the space holds a stream message back, when it comes due. The
        // connection whose message made it hold one sees it here next, so one always wakes.
        let due = space.next_due();
        let wake = tokio::time::Instant::from_std(due.unwrap_or_else(std::time::Instant::now));
        tokio::select! {
            _ = tokio::time::sleep_until(wake), if due.is_some() => {
                if let Err(err) = space.tick() {
                    warn!("space {}: {err}", query.space);
                }
            }
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
    fn import_map_is_relative_and_versioned() {
        let html = index_html();
        let map = format!("{{\"imports\":{{\"yakui/\":\"./lib/v{VERSION}/\"}}}}");
        assert!(html.contains(&map), "{html}");
        assert!(!html.contains(LIB_MARK));
        assert!(html.find("importmap").unwrap() < html.find("shell.js").unwrap());
    }

    #[test]
    fn protocol_js_reports_the_route_version() {
        assert!(PROTOCOL_JS.contains(VERSION_MARK));
        let served = protocol_source();
        assert!(served.contains(&format!("export const VERSION = {VERSION};")));
        assert!(is_current(&VERSION.to_string()));
        assert!(!is_current("0"));
    }

    #[test]
    fn widgets_have_their_header() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/web/widgets");
        let on_disk = std::fs::read_dir(dir).unwrap().count();
        assert_eq!(on_disk, WIDGETS.len(), "a widget file is not served");
        for (file, source) in WIDGETS {
            let has = |prefix: &str| source.lines().any(|line| line.starts_with(prefix));
            assert!(has("// props:"), "{file}: no `// props:` header");
            assert!(has("// emits:"), "{file}: no `// emits:` header");
        }
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
                let ok = line.ends_with(" from 'yakui/protocol.js';")
                    || line.ends_with(" from 'yakui/note.js';")
                    || widgets
                        .iter()
                        .any(|w| line.ends_with(&format!(" from 'yakui/widgets/{w}';")));
                assert!(ok, "{kind}: {line}");
            }
        }
        assert!(NOTE_JS.contains("export function flash("));
        assert!(!NOTE_JS.lines().any(|line| line.starts_with("import ")));
    }
}
