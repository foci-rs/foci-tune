//! In-process Moonraker stand-in: HTTP routes plus a JSON-RPC WebSocket.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

#[derive(Clone, Debug)]
enum Command {
    Push(String),
    DropSockets,
}

struct Inner {
    settings: Value,
    status: Value,
    scripts: Vec<String>,
    script_error: Option<String>,
    emergency_stops: u32,
    firmware_restarts: u32,
}

struct Shared {
    inner: Mutex<Inner>,
    refuse_ws: AtomicBool,
    commands: broadcast::Sender<Command>,
}

impl Shared {
    fn call(&self, method: &str, params: &Value) -> Result<Value, String> {
        let mut inner = self.inner.lock().unwrap();
        match method {
            "server.files.roots" => Ok(json!([
                {"name": "config", "path": "/home/pi/printer_data/config", "permissions": "rw"},
                {"name": "gcodes", "path": "/home/pi/printer_data/gcodes", "permissions": "rw"},
            ])),
            "printer.objects.query" => {
                let status = if params.get("configfile").is_some() {
                    json!({"configfile": {"settings": inner.settings.clone()}})
                } else {
                    inner.status.clone()
                };
                Ok(json!({"eventtime": 1.0, "status": status}))
            }
            "printer.objects.subscribe" => {
                if inner.status["webhooks"]["state"] == "disconnected" {
                    return Err("Klippy Disconnected".to_owned());
                }
                Ok(json!({"eventtime": 1.0, "status": inner.status.clone()}))
            }
            "printer.gcode.script" => {
                let script = params["script"].as_str().unwrap_or_default().to_owned();
                inner.scripts.push(script);
                match &inner.script_error {
                    Some(message) => Err(message.clone()),
                    None => Ok(json!("ok")),
                }
            }
            "printer.emergency_stop" => {
                inner.emergency_stops += 1;
                Ok(json!("ok"))
            }
            "printer.firmware_restart" => {
                inner.firmware_restarts += 1;
                Ok(json!("ok"))
            }
            other => Err(format!("Method not found: {other}")),
        }
    }
}

fn http_reply(result: Result<Value, String>) -> Response {
    match result {
        Ok(value) => Json(json!({"result": value})).into_response(),
        Err(message) => (
            StatusCode::BAD_REQUEST,
            Json(json!({"error": {"code": 400, "message": message}})),
        )
            .into_response(),
    }
}

async fn roots(State(shared): State<Arc<Shared>>) -> Response {
    http_reply(shared.call("server.files.roots", &Value::Null))
}

async fn query(
    State(shared): State<Arc<Shared>>,
    Query(params): Query<serde_json::Map<String, Value>>,
) -> Response {
    http_reply(shared.call("printer.objects.query", &Value::Object(params)))
}

async fn script(State(shared): State<Arc<Shared>>, Json(body): Json<Value>) -> Response {
    http_reply(shared.call("printer.gcode.script", &body))
}

async fn emergency_stop(State(shared): State<Arc<Shared>>) -> Response {
    http_reply(shared.call("printer.emergency_stop", &Value::Null))
}

async fn firmware_restart(State(shared): State<Arc<Shared>>) -> Response {
    http_reply(shared.call("printer.firmware_restart", &Value::Null))
}

async fn websocket(State(shared): State<Arc<Shared>>, ws: WebSocketUpgrade) -> Response {
    if shared.refuse_ws.load(Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let commands = shared.commands.subscribe();
    ws.on_upgrade(move |socket| serve_socket(shared, socket, commands))
}

async fn serve_socket(
    shared: Arc<Shared>,
    mut socket: WebSocket,
    mut commands: broadcast::Receiver<Command>,
) {
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                Ok(Command::Push(text)) => {
                    if socket.send(Message::Text(text.into())).await.is_err() {
                        return;
                    }
                }
                Ok(Command::DropSockets) | Err(_) => return,
            },
            message = socket.recv() => {
                let Some(Ok(Message::Text(text))) = message else {
                    if matches!(message, Some(Ok(_))) {
                        continue;
                    }
                    return;
                };
                let Ok(request) = serde_json::from_str::<Value>(text.as_str()) else {
                    continue;
                };
                let method = request["method"].as_str().unwrap_or_default();
                let reply = match shared.call(method, &request["params"]) {
                    Ok(result) => json!({"jsonrpc": "2.0", "result": result, "id": request["id"]}),
                    Err(message) => json!({
                        "jsonrpc": "2.0",
                        "error": {"code": -32601, "message": message},
                        "id": request["id"],
                    }),
                };
                if socket.send(Message::Text(reply.to_string().into())).await.is_err() {
                    return;
                }
            }
        }
    }
}

/// Handle to a running fake Moonraker; the server stops when it is dropped.
pub struct FakeMoonraker {
    addr: SocketAddr,
    shared: Arc<Shared>,
    task: JoinHandle<()>,
}

impl FakeMoonraker {
    pub async fn start() -> Self {
        let (commands, _) = broadcast::channel(64);
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                settings: json!({}),
                status: json!({
                    "webhooks": {"state": "ready"},
                    "toolhead": {"homed_axes": "", "max_velocity": 300.0, "max_accel": 3000.0},
                    "idle_timeout": {"state": "Idle"},
                    "print_stats": {"state": "standby"},
                }),
                scripts: Vec::new(),
                script_error: None,
                emergency_stops: 0,
                firmware_restarts: 0,
            }),
            refuse_ws: AtomicBool::new(false),
            commands,
        });
        let app = Router::new()
            .route("/server/files/roots", get(roots))
            .route("/printer/objects/query", get(query))
            .route("/printer/gcode/script", post(script))
            .route("/printer/emergency_stop", post(emergency_stop))
            .route("/printer/firmware_restart", post(firmware_restart))
            .route("/websocket", get(websocket))
            .with_state(shared.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { addr, shared, task }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn set_settings(&self, settings: Value) {
        self.shared.inner.lock().unwrap().settings = settings;
    }

    pub fn fail_scripts_with(&self, message: &str) {
        self.shared.inner.lock().unwrap().script_error = Some(message.to_owned());
    }

    pub fn scripts(&self) -> Vec<String> {
        self.shared.inner.lock().unwrap().scripts.clone()
    }

    pub fn emergency_stops(&self) -> u32 {
        self.shared.inner.lock().unwrap().emergency_stops
    }

    pub fn firmware_restarts(&self) -> u32 {
        self.shared.inner.lock().unwrap().firmware_restarts
    }

    pub fn push(&self, method: &str, params: Value) {
        let webhooks = match method {
            "notify_klippy_ready" => Some("ready"),
            "notify_klippy_shutdown" => Some("shutdown"),
            "notify_klippy_disconnected" => Some("disconnected"),
            _ => None,
        };
        if let Some(state) = webhooks {
            self.shared.inner.lock().unwrap().status["webhooks"]["state"] = json!(state);
        }
        let text = json!({"jsonrpc": "2.0", "method": method, "params": params}).to_string();
        let _ = self.shared.commands.send(Command::Push(text));
    }

    pub fn refuse_websockets(&self, refuse: bool) {
        self.shared.refuse_ws.store(refuse, Ordering::SeqCst);
    }

    pub fn drop_websockets(&self) {
        let _ = self.shared.commands.send(Command::DropSockets);
    }
}

impl Drop for FakeMoonraker {
    fn drop(&mut self) {
        self.task.abort();
    }
}
