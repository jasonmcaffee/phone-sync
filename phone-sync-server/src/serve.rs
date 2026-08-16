//! Connection-level hardening for the HTTP server (task-1556).
//!
//! On 2026-08-16 the Windows box ran out of RAM with every process working set summing to less than
//! half of what was in use. The missing 54 GB was kernel nonpaged pool tagged `AfdB` — Winsock
//! socket buffers — and it was released the instant the reverse proxy in front of this server was
//! restarted. The trigger was this service being replaced while the proxy still held connections to
//! the previous instance, during the task-1542 work that first made the web gallery stream video.
//!
//! Two things follow from that, and this module is both of them.
//!
//! **A killed process must not be able to leave much behind.** Service Manager stops services with
//! `taskkill /F`, which is `TerminateProcess`: no signal handler runs, no destructor runs, and the
//! kernel is left holding whatever each socket had queued. The only way to bound that is to bound
//! the sockets themselves, so the listening socket is created with explicit send/receive buffer
//! sizes and TCP keepalive, all of which Windows' `accept` copies onto every accepted connection.
//! Without an explicit size Windows auto-tunes these upwards with no ceiling this process controls.
//!
//! **A connection nobody is reading must not live forever.** `axum::serve` has no notion of a
//! connection that has stopped making progress, and a response body is only polled when the socket
//! can take more — so a client that stops reading mid-video stalls the stream in a way no
//! body-level timeout can ever observe. The watchdog here therefore counts bytes at the socket,
//! below HTTP, and drops the whole connection when that count stops moving.
//!
//! Shutdown is graceful with a hard deadline, which matters for a manual run or a Ctrl-C; under
//! Service Manager's `taskkill /F` it never gets the chance, which is exactly why the buffer bounds
//! above are the part that actually protects the box.

use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;

use axum::Router;
use hyper_util::rt::{TokioExecutor, TokioIo};
use hyper_util::server::conn::auto::Builder as ConnBuilder;
use hyper_util::service::TowerToHyperService;
use socket2::{Domain, Protocol, Socket, TcpKeepalive, Type};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};

/// Per-connection limits applied to every socket this server accepts.
#[derive(Clone, Debug)]
pub struct ServeLimits {
    /// SO_SNDBUF for accepted sockets. This is the ceiling on what an abruptly killed process can
    /// leave pinned in kernel memory per connection.
    pub send_buffer_bytes: usize,
    /// SO_RCVBUF for accepted sockets, which also caps the advertised receive window.
    pub recv_buffer_bytes: usize,
    /// How long a connection may be idle before TCP starts probing whether the peer still exists.
    pub keepalive_idle: Duration,
    /// Gap between keepalive probes once probing starts.
    pub keepalive_interval: Duration,
    /// No bytes in either direction for this long drops the connection.
    pub idle_timeout: Duration,
    /// How often the watchdog looks at a connection's byte counter.
    pub idle_check_interval: Duration,
    /// How long in-flight connections get to finish after a shutdown signal.
    pub shutdown_grace: Duration,
}

impl Default for ServeLimits {
    /// Values chosen for this box: every byte of public traffic reaches this service over loopback
    /// via the reverse proxy, and the iOS app reaches it over the LAN, so a 1 MB window is orders of
    /// magnitude more than either path can use — while capping what a killed process can strand.
    fn default() -> Self {
        Self {
            send_buffer_bytes: 1024 * 1024,
            recv_buffer_bytes: 1024 * 1024,
            keepalive_idle: Duration::from_secs(60),
            keepalive_interval: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(300),
            idle_check_interval: Duration::from_secs(15),
            shutdown_grace: Duration::from_secs(5),
        }
    }
}

impl ServeLimits {
    /// Overrides the defaults from `PHONE_SYNC_*` environment variables, so a limit can be adjusted
    /// on the live box without a rebuild.
    pub fn from_env() -> Self {
        let defaults = Self::default();
        Self {
            send_buffer_bytes: parsed_env("PHONE_SYNC_SOCKET_SEND_BUFFER_BYTES", defaults.send_buffer_bytes),
            recv_buffer_bytes: parsed_env("PHONE_SYNC_SOCKET_RECV_BUFFER_BYTES", defaults.recv_buffer_bytes),
            keepalive_idle: secs_env("PHONE_SYNC_SOCKET_KEEPALIVE_SECS", defaults.keepalive_idle),
            keepalive_interval: secs_env(
                "PHONE_SYNC_SOCKET_KEEPALIVE_INTERVAL_SECS",
                defaults.keepalive_interval,
            ),
            idle_timeout: secs_env("PHONE_SYNC_CONNECTION_IDLE_TIMEOUT_SECS", defaults.idle_timeout),
            idle_check_interval: secs_env(
                "PHONE_SYNC_CONNECTION_IDLE_CHECK_SECS",
                defaults.idle_check_interval,
            ),
            shutdown_grace: secs_env("PHONE_SYNC_SHUTDOWN_GRACE_SECS", defaults.shutdown_grace),
        }
    }
}

/// Reads a numeric environment variable, falling back when unset or unparseable.
/// @param key - the environment variable name
/// @param fallback - value used when the variable is absent or invalid
fn parsed_env<T: std::str::FromStr>(key: &str, fallback: T) -> T {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(fallback)
}

/// Reads a duration expressed in whole seconds, falling back when unset or unparseable.
/// @param key - the environment variable name
/// @param fallback - value used when the variable is absent or invalid
fn secs_env(key: &str, fallback: Duration) -> Duration {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
        .unwrap_or(fallback)
}

/// Creates the listening socket with the options every accepted connection should inherit.
///
/// Windows' `accept` copies SO_SNDBUF, SO_RCVBUF and SO_KEEPALIVE from the listening socket onto the
/// accepted one, so setting them once here is what bounds all of them.
/// @param addr - the address to listen on
/// @param limits - the per-connection limits to imprint on the listening socket
pub fn build_listener(addr: SocketAddr, limits: &ServeLimits) -> io::Result<std::net::TcpListener> {
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_send_buffer_size(limits.send_buffer_bytes)?;
    socket.set_recv_buffer_size(limits.recv_buffer_bytes)?;
    socket.set_tcp_keepalive(
        &TcpKeepalive::new()
            .with_time(limits.keepalive_idle)
            .with_interval(limits.keepalive_interval),
    )?;
    socket.set_tcp_nodelay(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    Ok(socket.into())
}

/// Accepts and serves connections until the shutdown future resolves.
///
/// Replaces `axum::serve` so that each connection can be watched and dropped when it stops making
/// progress, and so shutdown can stop waiting after a deadline instead of hanging on a stream whose
/// reader has gone away.
/// @param listener - the bound listener, already carrying the inherited socket options
/// @param app - the router to serve
/// @param limits - the per-connection limits and timeouts
/// @param shutdown - resolves when the server should stop accepting
pub async fn serve(
    listener: TcpListener,
    app: Router,
    limits: ServeLimits,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> io::Result<()> {
    let service = TowerToHyperService::new(app);
    let mut connections = tokio::task::JoinSet::new();
    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(pair) => pair,
                    Err(e) => {
                        tracing::warn!("accept failed: {e}");
                        continue;
                    }
                };
                connections.spawn(serve_connection(stream, peer, service.clone(), limits.clone()));
            }
            _ = &mut shutdown => {
                tracing::info!("shutdown signal received; {} connections in flight", connections.len());
                break;
            }
            // Reap finished connection tasks so the set cannot grow without bound.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }

    // Give in-flight work a moment, then abort what is left. Aborting drops each connection's
    // socket, which closes it now rather than leaving the kernel to flush a stream nobody is
    // reading.
    match tokio::time::timeout(limits.shutdown_grace, drain(&mut connections)).await {
        Ok(()) => tracing::info!("all connections finished cleanly"),
        Err(_) => {
            tracing::warn!(
                "{} connections still open after {:?}; dropping them",
                connections.len(),
                limits.shutdown_grace
            );
            connections.shutdown().await;
        }
    }
    Ok(())
}

/// Waits for every spawned connection task to finish.
async fn drain(connections: &mut tokio::task::JoinSet<()>) {
    while connections.join_next().await.is_some() {}
}

/// Serves one connection, dropping it if its byte counter stops moving.
///
/// The counter is read at the socket rather than at the HTTP layer on purpose: a response body is
/// only polled when the socket has room, so a client that has stopped reading produces no HTTP-level
/// event at all — the only visible symptom is that bytes stop moving.
/// @param stream - the accepted connection
/// @param peer - the peer address, for the log line when a connection is dropped
/// @param service - the router, adapted to hyper's service trait
/// @param limits - the per-connection limits and timeouts
async fn serve_connection(
    stream: TcpStream,
    peer: SocketAddr,
    service: TowerToHyperService<Router>,
    limits: ServeLimits,
) {
    let progress = Arc::new(AtomicU64::new(0));
    let io = TokioIo::new(MonitoredStream {
        inner: stream,
        progress: progress.clone(),
    });

    let builder = ConnBuilder::new(TokioExecutor::new());
    let connection = builder.serve_connection_with_upgrades(io, service);
    tokio::pin!(connection);

    tokio::select! {
        result = &mut connection => {
            if let Err(e) = result {
                // A client hanging up mid-response is ordinary, so this stays at debug.
                tracing::debug!("connection from {peer} ended: {e}");
            }
        }
        _ = watch_for_stall(progress, &limits) => {
            tracing::warn!(
                "dropping connection from {peer}: no bytes moved for {:?}",
                limits.idle_timeout
            );
        }
    }
}

/// Resolves once the connection has moved no bytes for the whole idle window.
/// @param progress - the connection's byte counter
/// @param limits - supplies the idle window and how often it is sampled
async fn watch_for_stall(progress: Arc<AtomicU64>, limits: &ServeLimits) {
    let mut last = progress.load(Ordering::Relaxed);
    let mut stalled = Duration::ZERO;
    loop {
        tokio::time::sleep(limits.idle_check_interval).await;
        let now = progress.load(Ordering::Relaxed);
        if now != last {
            last = now;
            stalled = Duration::ZERO;
            continue;
        }
        stalled += limits.idle_check_interval;
        if stalled >= limits.idle_timeout {
            return;
        }
    }
}

/// A TCP stream that counts every byte it moves, so a watchdog can tell work from stalling.
struct MonitoredStream {
    inner: TcpStream,
    progress: Arc<AtomicU64>,
}

impl AsyncRead for MonitoredStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let result = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &result {
            let moved = buf.filled().len() - before;
            if moved > 0 {
                self.progress.fetch_add(moved as u64, Ordering::Relaxed);
            }
        }
        result
    }
}

impl AsyncWrite for MonitoredStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(written)) = &result {
            self.progress.fetch_add(*written as u64, Ordering::Relaxed);
        }
        result
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let result = Pin::new(&mut self.inner).poll_write_vectored(cx, bufs);
        if let Poll::Ready(Ok(written)) = &result {
            self.progress.fetch_add(*written as u64, Ordering::Relaxed);
        }
        result
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }
}
