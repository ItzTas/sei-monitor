use std::future::IntoFuture;
use std::net::SocketAddr;
use std::thread::{self, JoinHandle};

use axum::Router;
use tokio::sync::oneshot;

pub(super) struct Server {
    address: SocketAddr,
    shutdown: Option<oneshot::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Server {
    pub(super) fn start(router: Router) -> Server {
        let listener =
            std::net::TcpListener::bind("127.0.0.1:0").expect("bind the fake SEI listener");
        listener
            .set_nonblocking(true)
            .expect("make the fake SEI listener non-blocking");
        let address = listener
            .local_addr()
            .expect("read the fake SEI listener address");
        let (shutdown, stop) = oneshot::channel();
        let thread = thread::Builder::new()
            .name("fake-sei".to_owned())
            .spawn(move || serve(listener, router, stop))
            .expect("start the fake SEI thread");
        Server {
            address,
            shutdown: Some(shutdown),
            thread: Some(thread),
        }
    }

    pub(super) fn address(&self) -> SocketAddr {
        self.address
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let Some(thread) = self.thread.take() else {
            return;
        };
        if thread.join().is_err() && !thread::panicking() {
            panic!("the fake SEI server thread panicked");
        }
    }
}

fn serve(listener: std::net::TcpListener, router: Router, stop: oneshot::Receiver<()>) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build the fake SEI runtime");
    runtime.block_on(async move {
        let listener =
            tokio::net::TcpListener::from_std(listener).expect("register the fake SEI listener");
        let server = tokio::spawn(axum::serve(listener, router).into_future());
        let _ = stop.await;
        server.abort();
    });
}
