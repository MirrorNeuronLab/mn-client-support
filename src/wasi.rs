//! WASI Preview 2 host I/O for standalone command components.
use std::{
    fs,
    future::Future,
    io,
    net::TcpListener,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use wasip2::io::streams::{InputStream, OutputStream, StreamError};

/// wasi-libc initially resolves relative paths from the first preopen. Apply
/// the launcher's WASI working directory before parsing any user paths.
pub fn enter_cwd() -> io::Result<()> {
    if let Some(dir) = wasip2::cli::environment::initial_cwd() {
        std::env::set_current_dir(dir)?;
    }
    Ok(())
}

/// WASI has no advisory file locks. Hold a kernel socket across the transaction.
/// A killed process releases it immediately. All processes derive the same
/// address from the canonical directory and purpose.
pub fn lock(root: &Path, name: &str) -> io::Result<TcpListener> {
    use sha2::{Digest, Sha256};
    let path = root.canonicalize()?.join(name);
    let hash = Sha256::digest(path.as_os_str().as_encoded_bytes());
    let port = 20000 + u16::from_be_bytes([hash[0], hash[1]]) % 40000;
    let started = std::time::Instant::now();
    loop {
        match TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)) {
            Err(error)
                if error.kind() == io::ErrorKind::AddrInUse
                    && started.elapsed() < Duration::from_secs(5) =>
            {
                std::thread::sleep(Duration::from_millis(10))
            }
            result => return result,
        }
    }
}

pub struct Input {
    stream: InputStream,
    retry: Option<Pin<Box<tokio::time::Sleep>>>,
}
pub fn stdin() -> Input {
    Input {
        stream: wasip2::cli::stdin::get_stdin(),
        retry: None,
    }
}
impl AsyncRead for Input {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if let Some(delay) = &mut self.retry {
            std::task::ready!(delay.as_mut().poll(cx));
            self.retry = None;
        }
        match self.stream.read(buf.remaining().min(64 * 1024) as u64) {
            Ok(bytes) if !bytes.is_empty() || buf.remaining() == 0 => {
                buf.put_slice(&bytes);
                Poll::Ready(Ok(()))
            }
            Ok(_) => {
                let mut retry = Box::pin(tokio::time::sleep(Duration::from_millis(5)));
                let _ = retry.as_mut().poll(cx);
                self.retry = Some(retry);
                Poll::Pending
            }
            Err(StreamError::Closed) => Poll::Ready(Ok(())),
            Err(e) => Poll::Ready(Err(io::Error::other(format!("{e:?}")))),
        }
    }
}

pub struct Output(OutputStream);
pub fn stdout() -> Output {
    Output(wasip2::cli::stdout::get_stdout())
}
impl AsyncWrite for Output {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let n = buf.len().min(4096);
        Poll::Ready(
            self.0
                .blocking_write_and_flush(&buf[..n])
                .map(|_| n)
                .map_err(|e| io::Error::other(format!("{e:?}"))),
        )
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.poll_flush(cx)
    }
}

// Standard WASI has no child processes. The installed POSIX launcher handles
// only these host utilities. Arguments are separate files and never shell code.
pub async fn utility(kind: &str, args: &[&str], limit: Duration) -> io::Result<(bool, Vec<u8>)> {
    let directory = PathBuf::from(std::env::var_os("MN_WASI_HOST_DIR").ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "This operation requires the installed host launcher",
        )
    })?);
    let request = tempfile::Builder::new()
        .prefix("call.")
        .tempdir_in(directory)?;
    for (i, arg) in args.iter().enumerate() {
        fs::write(request.path().join(i.to_string()), arg)?;
    }
    fs::write(request.path().join("kind"), kind)?;
    fs::write(request.path().join("ready"), [])?;
    let start = tokio::time::Instant::now();
    while !request.path().join("done").is_file() {
        if start.elapsed() >= limit {
            return Err(io::ErrorKind::TimedOut.into());
        }
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    Ok((
        fs::read(request.path().join("done"))? == b"0",
        fs::read(request.path().join("output"))?,
    ))
}
