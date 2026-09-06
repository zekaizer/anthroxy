use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, Uri};
use axum::response::Response;
use axum::routing::any;

/// One request as the backend saw it.
#[derive(Debug, Clone)]
pub struct Received {
    pub method: Method,
    pub path_and_query: String,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Received {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("upstream received JSON")
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|v| v.to_str().ok())
    }
}

pub type Handler = Arc<dyn Fn(&Received) -> Response + Send + Sync>;

#[derive(Clone)]
struct MockState {
    handler: Handler,
    received: Arc<Mutex<Vec<Received>>>,
}

/// An Anthropic-compatible backend whose behaviour is a closure.
pub struct MockUpstream {
    pub addr: SocketAddr,
    received: Arc<Mutex<Vec<Received>>>,
    _task: tokio::task::JoinHandle<()>,
}

impl MockUpstream {
    pub async fn start(handler: impl Fn(&Received) -> Response + Send + Sync + 'static) -> Self {
        let received = Arc::new(Mutex::new(Vec::new()));
        let state = MockState {
            handler: Arc::new(handler),
            received: received.clone(),
        };
        let app = Router::new().fallback(any(record)).with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            addr,
            received,
            _task: task,
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn received(&self) -> Vec<Received> {
        self.received.lock().unwrap().clone()
    }

    pub fn last(&self) -> Received {
        self.received()
            .pop()
            .expect("upstream received at least one request")
    }
}

async fn record(
    State(state): State<MockState>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let received = Received {
        method,
        path_and_query: uri
            .path_and_query()
            .map(|p| p.to_string())
            .unwrap_or_default(),
        headers,
        body,
    };
    let response = (state.handler)(&received);
    state.received.lock().unwrap().push(received);
    response
}

/// `200 application/json` with `body`.
pub fn json_response(status: u16, body: serde_json::Value) -> Response {
    Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

/// Echoes what was received so tests can assert on the upstream view.
pub fn echo(received: &Received) -> Response {
    let headers: serde_json::Map<String, serde_json::Value> = received
        .headers
        .iter()
        .map(|(k, v)| {
            (
                k.to_string(),
                serde_json::Value::String(v.to_str().unwrap_or("<bin>").to_owned()),
            )
        })
        .collect();
    json_response(
        200,
        serde_json::json!({
            "echo": {
                "path": received.path_and_query,
                "headers": headers,
                "body": serde_json::from_slice::<serde_json::Value>(&received.body).ok(),
            }
        }),
    )
}
