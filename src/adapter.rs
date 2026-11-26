//! CLI and HTTP consumers call the WIT imports. The helper inherits just private
//! pipes for the invocation. Dropping the caller also terminates its helper.
use crate::{
    Error, Result,
    control::{Bytes, Chunks, Method, Reply},
    wit::client::mirrorneuron::client::support as w,
};
use futures_util::{Stream, StreamExt, stream};
use serde_json::Value;
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    process::Stdio,
    sync::Mutex,
};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
pub use w::Frame;
pub use w::Reply as FrameMetadata;
use wrpc_transport::{
    Invoke,
    frame::{self, Incoming, Outgoing},
};
pub type UiFrames = Pin<Box<dyn Stream<Item = Result<Frame>> + Send>>;

struct Pipes(Mutex<Option<(ChildStdin, ChildStdout)>>);
impl Invoke for Pipes {
    type Context = ();
    type Outgoing = Outgoing;
    type Incoming = Incoming;
    async fn invoke<P>(
        &self,
        (): (),
        instance: &str,
        func: &str,
        params: Bytes,
        paths: impl AsRef<[P]> + Send,
    ) -> anyhow::Result<(Outgoing, Incoming)>
    where
        P: AsRef<[Option<usize>]> + Send + Sync,
    {
        let (tx, rx) = self
            .0
            .lock()
            .expect("private invocation lock")
            .take()
            .ok_or_else(|| anyhow::anyhow!("invocation already consumed"))?;
        frame::invoke(tx, rx, instance, func, params, paths).await
    }
}
fn unavailable() -> Error {
    Error::new(
        503,
        "MN_CLIENT_SUPPORT_UNAVAILABLE",
        "Cannot communicate with mn-client-support.",
    )
    .hint("Install the three matching Rust client packages together.")
}
impl From<w::Failure> for Error {
    fn from(e: w::Failure) -> Self {
        Self {
            code: e.code,
            message: e.message,
            status: e.status,
            exit_code: e.exit_code,
            hint: e.hint,
            details: serde_json::from_str(&e.details_json).unwrap_or(Value::Null),
            headers: Box::new(e.headers.into_iter().collect()),
        }
    }
}
fn failure(e: Error) -> w::Failure {
    w::Failure {
        code: e.code,
        message: e.message,
        status: e.status,
        exit_code: e.exit_code,
        hint: e.hint,
        details_json: e.details.to_string(),
        headers: (*e.headers).into_iter().collect(),
    }
}
fn value(s: String) -> Result<Value> {
    serde_json::from_str(&s).map_err(|_| Error::protocol())
}
fn result<T>(v: anyhow::Result<std::result::Result<T, w::Failure>>) -> Result<T> {
    v.map_err(|_| unavailable())?.map_err(Into::into)
}
fn reply(r: w::Reply) -> Result<Reply> {
    Ok(Reply {
        value: r
            .value_json
            .as_deref()
            .map(serde_json::from_str)
            .transpose()
            .map_err(|_| Error::protocol())?
            .unwrap_or(Value::Null),
        has_body: r.value_json.is_some(),
        status: r.status,
        headers: r.headers.into_iter().collect(),
    })
}
#[derive(Clone)]
pub struct Adapter {
    home: PathBuf,
    helper: Option<PathBuf>,
    revision: Option<String>,
}
impl Adapter {
    pub fn new(root: &Path) -> Self {
        Self {
            home: root.into(),
            helper: std::env::current_exe()
                .and_then(std::fs::canonicalize)
                .ok()
                .map(|exe| exe.with_file_name("mn-client-support")),
            revision: None,
        }
    }
    fn open(&self) -> Result<(Child, Pipes)> {
        let helper = self.helper.as_ref().ok_or_else(unavailable)?;
        let mut child = Command::new(helper)
            .arg("serve-stdio")
            .env("MN_HOME", &self.home)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|_| unavailable())?;
        let pipes = Pipes(Mutex::new(Some((
            child.stdin.take().unwrap(),
            child.stdout.take().unwrap(),
        ))));
        Ok((child, pipes))
    }
    pub async fn connect(
        &self,
        host: &str,
        token: Option<&str>,
        port: Option<u16>,
        local: Option<&str>,
    ) -> Result<Value> {
        let (_child, p) = self.open()?;
        value(result(w::connect(&p, (), host, token, port, local).await)?)
    }
    pub async fn remove(&self, node: &str) -> Result<Value> {
        let (_child, p) = self.open()?;
        value(result(w::remove_node(&p, (), node).await)?)
    }
    pub async fn status(&self) -> Result<Value> {
        let (_child, p) = self.open()?;
        value(result(w::status(&p, ()).await)?)
    }
    pub async fn catalog(&self, repo: Option<&str>, category: Option<&str>) -> Result<Value> {
        let (_child, p) = self.open()?;
        value(result(w::list_blueprints(&p, (), repo, category).await)?)
    }
    pub async fn pin(&self) -> Result<(Self, String)> {
        let (_child, p) = self.open()?;
        let (revision, runtime_home) = result(w::pin(&p, ()).await)?;
        Ok((
            Self {
                revision: Some(revision),
                ..self.clone()
            },
            runtime_home,
        ))
    }
    fn request(
        &self,
        method: Method,
        action: &str,
        args: Value,
        id: &str,
        seconds: u32,
    ) -> w::Call {
        let rpc = match method {
            Method::Capabilities => w::Method::Capabilities,
            Method::Read => w::Method::Read,
            Method::Execute => w::Method::Execute,
            Method::Prepare => w::Method::Prepare,
            Method::Catalog => w::Method::Catalog,
            Method::Events => w::Method::Events,
            Method::ReadFile => w::Method::ReadFile,
        };
        w::Call {
            rpc,
            action: action.into(),
            arguments_json: args.to_string(),
            request_id: id.into(),
            timeout_seconds: seconds,
            profile_revision: self.revision.clone(),
        }
    }
    pub async fn call(
        &self,
        method: Method,
        action: &str,
        args: Value,
        id: &str,
        seconds: u32,
    ) -> Result<Reply> {
        let (_child, p) = self.open()?;
        reply(result(
            w::invoke(&p, (), &self.request(method, action, args, id, seconds)).await,
        )?)
    }
    pub async fn command(&self, action: &str, args: Value, seconds: u32) -> Result<Value> {
        self.call(Method::Execute, action, args, "", seconds)
            .await?
            .checked()
    }
    pub async fn download(
        &self,
        method: Method,
        action: &str,
        args: Value,
    ) -> Result<(Reply, Chunks)> {
        let (child, p) = self.open()?;
        let req = self.request(method, action, args, "", 0);
        let (res, io) = w::observe(&p, (), &req).await.map_err(|_| unavailable())?;
        let (meta, chunks) = res.map_err(Error::from)?;
        Ok((reply(meta)?, drive(child, chunks, io)))
    }
    pub async fn upload(&self, action: &str, args: Value, chunks: Chunks) -> Result<Reply> {
        let (_child, p) = self.open()?;
        let req = self.request(Method::Execute, action, args, "", 3600);
        let (res, io) = w::upload(
            &p,
            (),
            &req,
            Box::pin(chunks.map(|v| vec![v.map_err(failure)])),
        )
        .await
        .map_err(|_| unavailable())?;
        let r = res.map_err(Error::from)?;
        if let Some(io) = io {
            io.await.map_err(|_| unavailable())?;
        }
        reply(r)
    }
    pub async fn ui(
        &self,
        args: Value,
        outgoing: Pin<Box<dyn Stream<Item = Frame> + Send>>,
    ) -> Result<UiFrames> {
        let (child, p) = self.open()?;
        let data = args.to_string();
        let (res, io) = w::job_ui(&p, (), &data, Box::pin(outgoing.map(|f| vec![f])))
            .await
            .map_err(|_| unavailable())?;
        Ok(drive(child, res.map_err(Error::from)?, io))
    }
}
type Batch<T> = Pin<Box<dyn Stream<Item = Vec<std::result::Result<T, w::Failure>>> + Send>>;
fn drive<T: Send + 'static>(
    child: Child,
    batches: Batch<T>,
    io: Option<impl Future<Output = anyhow::Result<()>> + Send + 'static>,
) -> Pin<Box<dyn Stream<Item = Result<T>> + Send>> {
    Box::pin(async_stream::try_stream! {
        let _child=child;
        let mut io=Box::pin(async move{if let Some(io)=io{io.await?;}Ok::<(),anyhow::Error>(())});
        let mut done=false;let mut stream=batches.flat_map(stream::iter);
        loop {
            let next=tokio::select!{
                result=&mut io, if !done=>{done=true;result.map(|_|None).map_err(|_|unavailable())},
                next=stream.next()=>Ok(Some(next)),
            }?;
            match next {Some(Some(v))=>yield v.map_err(Error::from)?,Some(None)=>break,None=>continue}
        }
        if !done {io.await.map_err(|_|unavailable())?;}
    })
}
