//! Exports the support world on a single private stdio invocation.
use crate::{
    Error, Result, catalog,
    connection::{self, Connection},
    control::{self, Bytes, Client, wire},
    wit::backend::{self, exports::mirrorneuron::client::support as w},
};
use futures_util::{Stream, StreamExt, stream};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    io,
    path::{Path, PathBuf},
    pin::Pin,
    time::Duration,
};

#[derive(Clone)]
struct Backend(PathBuf);
type Batch<T> = Pin<Box<dyn Stream<Item = Vec<T>> + Send>>;
type WireResult<T> = std::result::Result<T, w::Failure>;
type Response<T> = anyhow::Result<WireResult<T>>;
impl From<Error> for w::Failure {
    fn from(e: Error) -> Self {
        Self {
            code: e.code,
            message: e.message,
            status: e.status,
            exit_code: e.exit_code,
            hint: e.hint,
            details_json: e.details.to_string(),
            headers: (*e.headers).into_iter().collect(),
        }
    }
}
impl From<control::Reply> for w::Reply {
    fn from(r: control::Reply) -> Self {
        Self {
            status: r.status,
            headers: r.headers.into_iter().collect(),
            value_json: r.has_body.then(|| r.value.to_string()),
        }
    }
}
fn serialized(result: Result<Value>) -> Response<String> {
    Ok(result.map(|v| v.to_string()).map_err(Into::into))
}
fn revision(c: &Connection) -> String {
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(c).expect("serializable connection"))
    )
}
impl Backend {
    fn client(&self, expected: Option<&str>) -> Result<Client> {
        let c = Connection::load(&self.0)?;
        if expected.is_some_and(|s| s != revision(&c)) {
            return Err(Error::new(
                409,
                "MN_CLIENT_CONNECTION_MISMATCH",
                "The active connection changed during this operation.",
            ));
        }
        Client::new(&c)
    }
}
fn method(m: w::Method) -> control::Method {
    match m {
        w::Method::Capabilities => control::Method::Capabilities,
        w::Method::Read => control::Method::Read,
        w::Method::Catalog => control::Method::Catalog,
        w::Method::Prepare => control::Method::Prepare,
        w::Method::Execute => control::Method::Execute,
        w::Method::Events => control::Method::Events,
        w::Method::ReadFile => control::Method::ReadFile,
    }
}
fn arguments(s: &str) -> Result<Value> {
    serde_json::from_str::<Value>(s)
        .ok()
        .filter(Value::is_object)
        .ok_or_else(|| Error::invalid("Expected operation arguments."))
}
impl w::Handler<()> for Backend {
    async fn connect(
        &self,
        (): (),
        host: String,
        token: Option<String>,
        port: Option<u16>,
        runtime_home: Option<String>,
    ) -> Response<String> {
        serialized(
            connection::connect(
                &self.0,
                &host,
                token.as_deref(),
                port,
                runtime_home.as_deref().map(Path::new),
            )
            .await,
        )
    }
    async fn remove_node(&self, (): (), node: String) -> Response<String> {
        serialized(connection::remove(&self.0, &node).await)
    }
    async fn status(&self, (): ()) -> Response<String> {
        serialized((|| {
            let connection = match Connection::load(&self.0) {
                Ok(c) => c.public(),
                Err(e) if e.code == "MN_CLIENT_NOT_CONNECTED" => json!({"status":"disconnected"}),
                Err(e) => return Err(e),
            };
            let role = connection::role(&self.0)?;
            Ok(json!({"mode":role,"runtime_installed":role!="client","connection":connection}))
        })())
    }
    async fn list_blueprints(
        &self,
        (): (),
        repository: Option<String>,
        category: Option<String>,
    ) -> Response<String> {
        serialized(catalog::list(&self.0, repository.as_deref(), category.as_deref()).await)
    }
    async fn pin(&self, (): ()) -> Response<(String, String)> {
        Ok(Connection::load(&self.0)
            .map(|c| (revision(&c), c.runtime_home))
            .map_err(Into::into))
    }
    async fn invoke(&self, (): (), r: w::Call) -> Response<w::Reply> {
        Ok(async {
            let client = self.client(r.profile_revision.as_deref())?;
            client
                .call(
                    method(r.rpc),
                    &r.action,
                    arguments(&r.arguments_json)?,
                    &r.request_id,
                    Duration::from_secs(r.timeout_seconds.into()),
                )
                .await
        }
        .await
        .map(Into::into)
        .map_err(Into::into))
    }
    async fn observe(&self, (): (), r: w::Call) -> Response<(w::Reply, Batch<WireResult<Bytes>>)> {
        let res = async {
            self.client(r.profile_revision.as_deref())?
                .download(method(r.rpc), &r.action, arguments(&r.arguments_json)?)
                .await
        }
        .await;
        Ok(res
            .map(|(reply, chunks)| {
                (
                    reply.into(),
                    Box::pin(chunks.map(|c| vec![c.map_err(Into::into)])) as Batch<_>,
                )
            })
            .map_err(Into::into))
    }
    async fn upload(
        &self,
        (): (),
        r: w::Call,
        data: Batch<WireResult<Bytes>>,
    ) -> Response<w::Reply> {
        let chunks = data
            .flat_map(stream::iter)
            .map(|c| c.map_err(|e| Error::new(e.status, &e.code, &e.message)));
        Ok(async {
            self.client(r.profile_revision.as_deref())?
                .upload(&r.action, arguments(&r.arguments_json)?, Box::pin(chunks))
                .await
        }
        .await
        .map(Into::into)
        .map_err(Into::into))
    }
    async fn job_ui(
        &self,
        (): (),
        args: String,
        outgoing: Batch<w::Frame>,
    ) -> Response<Batch<WireResult<w::Frame>>> {
        let res = async {
            let first = control::first_frame("job.ui", arguments(&args)?);
            let frames =
                stream::once(async { first }).chain(outgoing.flat_map(stream::iter).map(|f| {
                    wire::RuntimeFrame {
                        version: 1,
                        kind: f.kind,
                        data: f.data.to_vec(),
                        request: None,
                        reply: f.metadata.map(|r| wire::RuntimeReply {
                            version: 1,
                            status: r.status.into(),
                            metadata: r.headers.into_iter().collect(),
                            value_json: r.value_json.unwrap_or_default(),
                            error_json: String::new(),
                        }),
                    }
                }));
            self.client(None)?.ui(Box::pin(frames)).await
        }
        .await;
        Ok(res
            .map(|incoming| {
                Box::pin(incoming.map(|item| {
                    vec![
                        item.map(|f| w::Frame {
                            kind: f.kind,
                            data: f.data.into(),
                            metadata: f.reply.map(|r| w::Reply {
                                status: r.status as u16,
                                headers: r.metadata.into_iter().collect(),
                                value_json: (!r.value_json.is_empty()).then_some(r.value_json),
                            }),
                        })
                        .map_err(Into::into),
                    ]
                })) as Batch<_>
            })
            .map_err(Into::into))
    }
}
// A server belongs to one caller. Flush completion must be scoped to that
// invocation when multiple API requests run in the same WASI process.
struct Pipes<R, W>(std::sync::Mutex<Option<(W, R)>>);
impl<
    R: tokio::io::AsyncRead + Unpin + Send + Sync + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
> wrpc_transport::Accept for Pipes<R, W>
{
    type Context = ();
    type Incoming = R;
    type Outgoing = W;
    async fn accept(&self) -> io::Result<((), W, R)> {
        let (w, r) = self
            .0
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| io::Error::other("session already opened"))?;
        Ok(((), w, r))
    }
}

struct Output<W> {
    pipe: W,
    flushed: std::sync::Arc<tokio::sync::Notify>,
}
impl<W: tokio::io::AsyncWrite + Unpin> tokio::io::AsyncWrite for Output<W> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bytes: &[u8],
    ) -> std::task::Poll<io::Result<usize>> {
        Pin::new(&mut self.pipe).poll_write(cx, bytes)
    }
    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.pipe).poll_flush(cx)
    }
    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        Pin::new(&mut self.pipe).poll_shutdown(cx)
    }
}
struct Flush;
impl<
    R: tokio::io::AsyncRead + Unpin + Send + Sync + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
> wrpc_transport::frame::ConnHandler<R, Output<W>> for Flush
{
    async fn on_egress(mut tx: Output<W>, _: io::Result<()>) {
        use tokio::io::AsyncWriteExt;
        let _ = tx.flush().await;
        tx.flushed.notify_one();
    }
}
pub(crate) async fn serve_pipes<R, W>(root: PathBuf, read: R, write: W) -> anyhow::Result<()>
where
    R: tokio::io::AsyncRead + Unpin + Send + Sync + 'static,
    W: tokio::io::AsyncWrite + Unpin + Send + Sync + 'static,
{
    let flushed = std::sync::Arc::new(tokio::sync::Notify::new());
    let server = wrpc_transport::Server::<(), R, Output<W>, Flush>::new();
    let registrations = backend::serve(&server, Backend(root)).await?;
    server
        .accept(Pipes(std::sync::Mutex::new(Some((
            Output {
                pipe: write,
                flushed: flushed.clone(),
            },
            read,
        )))))
        .await?;
    let mut calls = stream::select_all(registrations.into_iter().map(|(_, _, s)| s));
    calls
        .next()
        .await
        .ok_or_else(|| anyhow::anyhow!("support invocation closed"))??
        .await?;
    flushed.notified().await;
    Ok(())
}
pub async fn serve() -> anyhow::Result<()> {
    #[cfg(not(target_os = "wasi"))]
    {
        use std::os::fd::AsFd;
        let fd = std::io::stdout().as_fd().try_clone_to_owned()?;
        serve_pipes(
            connection::home()?,
            tokio::io::stdin(),
            tokio::fs::File::from_std(std::fs::File::from(fd)),
        )
        .await
    }
    #[cfg(target_os = "wasi")]
    {
        serve_pipes(
            connection::home()?,
            crate::wasi::stdin(),
            crate::wasi::stdout(),
        )
        .await
    }
}
