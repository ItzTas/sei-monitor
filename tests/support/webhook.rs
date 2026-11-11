use std::future::IntoFuture;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{Method, StatusCode};
use serde_json::Value;
use tokio::sync::oneshot;

type Bodies = Arc<Mutex<Vec<Value>>>;

pub struct Webhook {
    address: SocketAddr,
    bodies: Bodies,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Webhook {
    pub fn start() -> Webhook {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind the webhook listener");
        listener
            .set_nonblocking(true)
            .expect("make the webhook listener non-blocking");
        let address = listener
            .local_addr()
            .expect("read the webhook listener address");
        let bodies = Bodies::default();
        let router = Router::new()
            .fallback(receive)
            .with_state(Arc::clone(&bodies));
        let (shutdown, stop) = oneshot::channel();
        let thread = thread::Builder::new()
            .name("webhook".to_owned())
            .spawn(move || serve(listener, router, stop))
            .expect("start the webhook thread");
        Webhook {
            address,
            bodies,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    pub fn url(&self) -> String {
        format!("http://{}/hook", self.address)
    }

    pub fn bodies(&self) -> Vec<Value> {
        self.bodies
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Drop for Webhook {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let Some(thread) = self.thread.take() else {
            return;
        };
        if thread.join().is_err() && !thread::panicking() {
            panic!("the webhook server thread panicked");
        }
    }
}

async fn receive(State(bodies): State<Bodies>, method: Method, body: Bytes) -> StatusCode {
    if method != Method::POST {
        return StatusCode::METHOD_NOT_ALLOWED;
    }
    let received = serde_json::from_slice(&body)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).into_owned()));
    bodies
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push(received);
    StatusCode::OK
}

fn serve(listener: std::net::TcpListener, router: Router, stop: oneshot::Receiver<()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the webhook runtime");
    runtime.block_on(async move {
        let listener =
            tokio::net::TcpListener::from_std(listener).expect("register the webhook listener");
        let server = tokio::spawn(axum::serve(listener, router).into_future());
        let _ = stop.await;
        server.abort();
    });
}
