use crate::{Error, Result, connection::Connection};
use futures_util::{Stream, StreamExt};
use serde_json::Value;
use std::{collections::HashMap, pin::Pin, time::Duration};
use tonic::{
    Code, Request, Status, Streaming,
    metadata::{Ascii, MetadataValue},
    transport::{Channel, Endpoint},
};
pub mod wire {
    tonic::include_proto!("mirrorneuron.runtime.v1");
}
use wire::{
    RuntimeFrame, RuntimeReply, RuntimeRequest,
    runtime_control_service_client::RuntimeControlServiceClient,
};

struct OutgoingFrames(Pin<Box<dyn Stream<Item = RuntimeFrame> + Send>>);
impl Stream for OutgoingFrames {
    type Item = RuntimeFrame;
    fn poll_next(
        mut self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<RuntimeFrame>> {
        self.0.as_mut().poll_next(cx)
    }
}

pub const FRAME_BYTES: usize = 64 * 1024;
const MESSAGE_BYTES: usize = 16 * 1024 * 1024;
pub type Frames = Pin<Box<dyn Stream<Item = Result<RuntimeFrame>> + Send>>;
pub type Chunks = Pin<Box<dyn Stream<Item = Result<bytes::Bytes>> + Send>>;
pub use bytes::Bytes;
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Capabilities,
    Catalog,
    Prepare,
    Execute,
    Read,
    Events,
    ReadFile,
}
pub struct Reply {
    pub value: Value,
    pub has_body: bool,
    pub status: u16,
    pub headers: HashMap<String, String>,
}
impl Reply {
    pub fn decode(reply: RuntimeReply) -> Result<Self> {
        if reply.version != 1 {
            return Err(Error::protocol());
        }
        let status = u16::try_from(reply.status)
            .ok()
            .filter(|s| (100..600).contains(s))
            .ok_or_else(Error::protocol)?;
        if !reply.error_json.is_empty() {
            let e: Value =
                serde_json::from_str(&reply.error_json).map_err(|_| Error::protocol())?;
            let mut error = Error::new(
                status,
                e["code"].as_str().ok_or_else(Error::protocol)?,
                e["message"].as_str().ok_or_else(Error::protocol)?,
            );
            error.exit_code = e["exit_code"]
                .as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .unwrap_or(error.exit_code);
            *error.details = e["details"].clone();
            error.hint = e["hint"].as_str().map(str::to_owned);
            *error.headers = reply.metadata;
            return Err(error);
        }
        let has_body = !reply.value_json.is_empty();
        let value = if has_body {
            serde_json::from_str(&reply.value_json).map_err(|_| Error::protocol())?
        } else {
            Value::Null
        };
        Ok(Self {
            value,
            has_body,
            status,
            headers: reply.metadata,
        })
    }
    pub fn checked(self) -> Result<Value> {
        if self.status >= 400 {
            return Err(Error::new(
                self.status,
                self.value["code"]
                    .as_str()
                    .unwrap_or("MN_RUNTIME_OPERATION_FAILED"),
                self.value["detail"]
                    .as_str()
                    .unwrap_or("The runtime rejected the operation."),
            ));
        }
        Ok(self.value)
    }
}
pub fn request(action: &str, values: Value, request_id: &str) -> RuntimeRequest {
    RuntimeRequest {
        version: 1,
        action: action.into(),
        arguments_json: values.to_string(),
        request_id: request_id.into(),
    }
}
pub fn first_frame(action: &str, args: Value) -> RuntimeFrame {
    RuntimeFrame {
        version: 1,
        kind: "request".into(),
        request: Some(request(action, args, "")),
        ..Default::default()
    }
}
pub fn data_frame(kind: &str, data: &[u8]) -> RuntimeFrame {
    RuntimeFrame {
        version: 1,
        kind: kind.into(),
        data: data.to_vec(),
        ..Default::default()
    }
}
fn transport(s: Status) -> Error {
    match s.code() {
        Code::Unauthenticated => Error::new(
            401,
            "MN_RUNTIME_UNAUTHENTICATED",
            "The runtime rejected this connection credential.",
        ),
        Code::PermissionDenied => Error::new(
            403,
            "MN_RUNTIME_PERMISSION_DENIED",
            "The runtime denied this operation.",
        ),
        Code::Unimplemented => Error::new(
            412,
            "MN_RUNTIME_INCOMPATIBLE",
            "The runtime does not support this operation.",
        ),
        Code::FailedPrecondition => Error::new(
            409,
            "MN_RUNTIME_PRECONDITION_FAILED",
            "The runtime state changed or this operation is unavailable.",
        ),
        Code::Cancelled => Error::new(
            499,
            "MN_RUNTIME_CANCELLED",
            "The runtime request was cancelled.",
        ),
        _ => Error::new(
            if s.code() == Code::DeadlineExceeded {
                504
            } else {
                503
            },
            "MN_RUNTIME_UNAVAILABLE",
            "The runtime connection was interrupted.",
        )
        .hint(
            "Check the connection and inspect the existing job or operation before resubmitting.",
        ),
    }
}
fn checked_frames(mut frames: Streaming<RuntimeFrame>) -> Frames {
    Box::pin(async_stream::try_stream! {
        while let Some(frame) = frames.message().await.map_err(transport)? {
            if frame.version != 1 || frame.data.len() > FRAME_BYTES {
                Err(Error::protocol())?;
            }
            if let Some(reply) = &frame.reply {
                if reply.version != 1 {
                    Err(Error::protocol())?;
                }
                if !reply.error_json.is_empty() {
                    Reply::decode(reply.clone())?;
                }
            }
            yield frame;
        }
    })
}
#[derive(Clone)]
pub struct Client {
    stub: RuntimeControlServiceClient<Channel>,
    auth: MetadataValue<Ascii>,
}
impl Client {
    pub fn new(c: &Connection) -> Result<Self> {
        let endpoint = Endpoint::from_shared(format!("http://{}", c.target))
            .map_err(|_| Error::storage())?
            .connect_timeout(Duration::from_secs(10));
        let mut auth: MetadataValue<Ascii> = format!("Bearer {}", c.token)
            .parse()
            .map_err(|_| Error::storage())?;
        auth.set_sensitive(true);
        Ok(Self {
            stub: RuntimeControlServiceClient::new(endpoint.connect_lazy())
                .max_encoding_message_size(MESSAGE_BYTES)
                .max_decoding_message_size(MESSAGE_BYTES),
            auth,
        })
    }
    fn authenticated<T>(&self, t: T, timeout: Option<Duration>) -> Request<T> {
        let mut req = Request::new(t);
        req.metadata_mut()
            .insert("authorization", self.auth.clone());
        if let Some(d) = timeout {
            req.set_timeout(d);
        }
        req
    }
    pub async fn call(
        &self,
        method: Method,
        action: &str,
        args: Value,
        id: &str,
        timeout: Duration,
    ) -> Result<Reply> {
        let mut stub = self.stub.clone();
        let req = self.authenticated(request(action, args, id), Some(timeout));
        let result = match method {
            Method::Capabilities => stub.get_capabilities(req).await,
            Method::Read => stub.read(req).await,
            Method::Prepare => stub.prepare(req).await,
            Method::Execute => stub.execute(req).await,
            Method::Catalog => stub.catalog(req).await,
            _ => return Err(Error::protocol()),
        };
        Reply::decode(result.map_err(transport)?.into_inner())
    }
    pub async fn command(&self, action: &str, args: Value, seconds: u64) -> Result<Value> {
        self.call(
            Method::Execute,
            action,
            args,
            "",
            Duration::from_secs(seconds),
        )
        .await?
        .checked()
    }
    pub async fn frames(&self, method: Method, action: &str, args: Value) -> Result<Frames> {
        let mut stub = self.stub.clone();
        let req = self.authenticated(request(action, args, ""), None);
        let result = match method {
            Method::Events => stub.events(req).await,
            Method::ReadFile => stub.read_file(req).await,
            _ => return Err(Error::protocol()),
        };
        Ok(checked_frames(result.map_err(transport)?.into_inner()))
    }
    pub async fn download(
        &self,
        method: Method,
        action: &str,
        args: Value,
    ) -> Result<(Reply, Chunks)> {
        let mut frames = self.frames(method, action, args).await?;
        let f = frames
            .next()
            .await
            .transpose()?
            .filter(|f| f.kind == "headers")
            .ok_or_else(Error::protocol)?;
        let reply = Reply::decode(f.reply.ok_or_else(Error::protocol)?)?;
        let chunks = async_stream::try_stream! {while let Some(f)=frames.next().await{let f=f?;if f.kind=="data"{yield bytes::Bytes::from(f.data);}}};
        Ok((reply, Box::pin(chunks)))
    }
    pub async fn upload(&self, action: &str, args: Value, mut chunks: Chunks) -> Result<Reply> {
        let first = first_frame(action, args);
        // An input error must abort the RPC, never turn a truncated upload into a clean EOF.
        let (failed, mut failure) = tokio::sync::oneshot::channel();
        let outgoing = async_stream::stream! {yield first;while let Some(item)=chunks.next().await{match item {
            Ok(data)=>for piece in data.chunks(FRAME_BYTES){yield data_frame("data",piece);},
            Err(e)=>{let _=failed.send(e);std::future::pending::<()>().await;return;}
        }}};
        let req = self.authenticated(
            OutgoingFrames(Box::pin(outgoing)),
            Some(Duration::from_secs(3600)),
        );
        let mut stub = self.stub.clone();
        tokio::select! {biased; Ok(e)=&mut failure=>Err(e), result=stub.upload(req)=>Reply::decode(result.map_err(transport)?.into_inner())}
    }
    pub async fn ui(
        &self,
        frames: Pin<Box<dyn Stream<Item = RuntimeFrame> + Send>>,
    ) -> Result<Frames> {
        Ok(checked_frames(
            self.stub
                .clone()
                .job_ui(self.authenticated(OutgoingFrames(frames), None))
                .await
                .map_err(transport)?
                .into_inner(),
        ))
    }
}
