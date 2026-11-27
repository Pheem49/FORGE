use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;

use http::HeaderMap;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::config::{AppConfig, Config};
use crate::error::{ForgeError, codes};
use crate::kernel::assets::percent_decode;
use crate::runtime::{AppHandle, WindowHandle};

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Raw binary IPC payload or response buffer transferred over `application/octet-stream`
/// without JSON or Base64 serialization overhead.
///
/// Intentionally does not implement `serde::Serialize` or `serde::Deserialize` so zero-macro
/// `async fn` command handlers resolve unambiguously alongside JSON handlers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawBytes(pub Vec<u8>);

impl RawBytes {
    pub fn new(bytes: impl Into<Vec<u8>>) -> Self {
        Self(bytes.into())
    }

    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

impl Deref for RawBytes {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<[u8]> for RawBytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl From<Vec<u8>> for RawBytes {
    fn from(v: Vec<u8>) -> Self {
        Self(v)
    }
}

impl From<&[u8]> for RawBytes {
    fn from(slice: &[u8]) -> Self {
        Self(slice.to_vec())
    }
}

impl From<RawBytes> for Vec<u8> {
    fn from(raw: RawBytes) -> Self {
        raw.0
    }
}

/// Structured JSON metadata extractor read from the `X-Forge-Meta` IPC header,
/// enabling binary commands (`RawBytes`) to receive strongly typed arguments alongside raw bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meta<T>(pub T);

impl<T> Deref for Meta<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Wire representation passed into and out of [`ErasedCommand`].
#[doc(hidden)]
#[derive(Debug)]
pub enum IpcBody {
    Json(Value),
    Bytes(Vec<u8>),
}

/// Immutable map of registered application state, frozen once `App::setup` completes.
#[derive(Default)]
pub(crate) struct StateMap {
    inner: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

impl StateMap {
    pub(crate) fn insert<T: Send + Sync + 'static>(&mut self, value: T) {
        self.inner.insert(TypeId::of::<T>(), Arc::new(value));
    }

    pub(crate) fn get<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.inner
            .get(&TypeId::of::<T>())
            .and_then(|arc| Arc::clone(arc).downcast::<T>().ok())
    }

    pub(crate) fn contains_type_id(&self, id: &TypeId) -> bool {
        self.inner.contains_key(id)
    }
}

/// Internal context passed to command extractors during an IPC invocation.
#[derive(Clone)]
pub struct InvokeContext {
    pub(crate) states: Arc<StateMap>,
    pub(crate) config: Arc<Config>,
    pub(crate) app_handle: AppHandle,
    pub(crate) window: WindowHandle,
    pub(crate) headers: HeaderMap,
}

mod sealed {
    pub trait Sealed {}
}

/// Sealed extractor trait for context arguments (`State<T>`, `WindowHandle`, `AppHandle`, `AppConfig`, `Meta<T>`)
/// that are injected by FORGE without consuming the request body.
pub trait FromContext: sealed::Sealed + Sized {
    #[doc(hidden)]
    fn required_state_type() -> Option<(TypeId, &'static str)> {
        None
    }

    #[doc(hidden)]
    fn from_context(ctx: &InvokeContext) -> Result<Self, ForgeError>;
}

/// Shared application state extractor (`Clone` + `Deref<Target = T>`).
pub struct State<T: ?Sized>(Arc<T>);

impl<T: ?Sized> Clone for State<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl<T: ?Sized> Deref for State<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<T: Send + Sync + 'static> sealed::Sealed for State<T> {}

impl<T: Send + Sync + 'static> FromContext for State<T> {
    fn required_state_type() -> Option<(TypeId, &'static str)> {
        Some((TypeId::of::<T>(), std::any::type_name::<T>()))
    }

    fn from_context(ctx: &InvokeContext) -> Result<Self, ForgeError> {
        ctx.states.get::<T>().map(State).ok_or_else(|| {
            ForgeError::new(
                "FORGE_MISSING_STATE",
                format!("missing registered State<{}>", std::any::type_name::<T>()),
            )
        })
    }
}

impl<T: DeserializeOwned + Send + 'static> sealed::Sealed for Meta<T> {}

impl<T: DeserializeOwned + Send + 'static> FromContext for Meta<T> {
    fn from_context(ctx: &InvokeContext) -> Result<Self, ForgeError> {
        let raw_header = ctx
            .headers
            .get("x-forge-meta")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                ForgeError::bad_request(
                    codes::INVALID_PAYLOAD,
                    "missing required X-Forge-Meta header for Meta<T> extractor",
                )
            })?;

        let json_str = if let Some(encoded) = raw_header.strip_prefix("uri:") {
            percent_decode(encoded)
        } else {
            raw_header.to_string()
        };

        let parsed: T = serde_json::from_str(&json_str).map_err(|err| {
            ForgeError::bad_request(
                codes::INVALID_PAYLOAD,
                format!("invalid X-Forge-Meta JSON header: {err}"),
            )
        })?;
        Ok(Meta(parsed))
    }
}

impl sealed::Sealed for AppConfig {}

impl FromContext for AppConfig {
    fn from_context(ctx: &InvokeContext) -> Result<Self, ForgeError> {
        Ok(AppConfig(Arc::clone(&ctx.config)))
    }
}

impl sealed::Sealed for WindowHandle {}

impl FromContext for WindowHandle {
    fn from_context(ctx: &InvokeContext) -> Result<Self, ForgeError> {
        Ok(ctx.window.clone())
    }
}

impl sealed::Sealed for AppHandle {}

impl FromContext for AppHandle {
    fn from_context(ctx: &InvokeContext) -> Result<Self, ForgeError> {
        Ok(ctx.app_handle.clone())
    }
}

#[doc(hidden)]
pub struct NoPayload;
#[doc(hidden)]
pub struct WithPayload;
#[doc(hidden)]
pub struct BinaryPayload;
#[doc(hidden)]
pub struct NoPayloadBinaryReply;
#[doc(hidden)]
pub struct WithPayloadBinaryReply;
#[doc(hidden)]
pub struct BinaryInOut;

/// Zero-macro async command handler trait supporting context extractors and JSON or [`RawBytes`] payloads/replies.
pub trait CommandHandler<Marker>: Send + Sync + 'static {
    #[doc(hidden)]
    fn required_states() -> Vec<(TypeId, &'static str)>;

    #[doc(hidden)]
    fn call(
        &self,
        ctx: InvokeContext,
        body: IpcBody,
    ) -> BoxFuture<'static, Result<IpcBody, ForgeError>>;
}

pub(crate) trait ErasedCommand: Send + Sync + 'static {
    fn required_states(&self) -> Vec<(TypeId, &'static str)>;
    fn call(
        &self,
        ctx: InvokeContext,
        body: IpcBody,
    ) -> BoxFuture<'static, Result<IpcBody, ForgeError>>;
}

pub(crate) struct CommandEntry<H, Marker> {
    handler: H,
    _marker: std::marker::PhantomData<fn() -> Marker>,
}

impl<H, Marker> CommandEntry<H, Marker> {
    pub(crate) fn new(handler: H) -> Self {
        Self {
            handler,
            _marker: std::marker::PhantomData,
        }
    }
}

impl<H, Marker> ErasedCommand for CommandEntry<H, Marker>
where
    H: CommandHandler<Marker>,
    Marker: 'static,
{
    fn required_states(&self) -> Vec<(TypeId, &'static str)> {
        H::required_states()
    }

    fn call(
        &self,
        ctx: InvokeContext,
        body: IpcBody,
    ) -> BoxFuture<'static, Result<IpcBody, ForgeError>> {
        self.handler.call(ctx, body)
    }
}

fn deserialize_json_body<Req: DeserializeOwned>(body: IpcBody) -> Result<Req, ForgeError> {
    match body {
        IpcBody::Json(val) => serde_json::from_value(val).map_err(|err| {
            ForgeError::bad_request(
                codes::INVALID_PAYLOAD,
                format!("invalid command payload: {err}"),
            )
        }),
        IpcBody::Bytes(_) => Err(ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            "command expects application/json payload, but received application/octet-stream",
        )),
    }
}

fn extract_binary_body(body: IpcBody) -> Result<RawBytes, ForgeError> {
    match body {
        IpcBody::Bytes(bytes) => Ok(RawBytes(bytes)),
        IpcBody::Json(_) => Err(ForgeError::bad_request(
            codes::INVALID_PAYLOAD,
            "command expects application/octet-stream binary payload (pass Uint8Array/ArrayBuffer or Content-Type: application/octet-stream)",
        )),
    }
}

fn serialize_json_reply<Res: Serialize>(reply: Res) -> Result<IpcBody, ForgeError> {
    serde_json::to_value(reply)
        .map(IpcBody::Json)
        .map_err(|err| {
            ForgeError::new(
                codes::SERIALIZE_ERROR,
                format!("failed to serialize command reply: {err}"),
            )
        })
}

macro_rules! impl_command_handler {
    ($($E:ident),*) => {
        // 1. No payload -> JSON reply
        impl<Func, Fut, Res, Err, $($E,)*> CommandHandler<(NoPayload, ($($E,)*))> for Func
        where
            Func: Fn($($E,)*) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = Result<Res, Err>> + Send + 'static,
            Res: Serialize + Send + 'static,
            Err: Into<ForgeError> + Send + 'static,
            $($E: FromContext + Send + 'static,)*
        {
            #[allow(unused_mut)]
            fn required_states() -> Vec<(TypeId, &'static str)> {
                let mut reqs = Vec::new();
                $(
                    if let Some(item) = $E::required_state_type() {
                        reqs.push(item);
                    }
                )*
                reqs
            }

            #[allow(unused_variables, non_snake_case)]
            fn call(&self, ctx: InvokeContext, _body: IpcBody) -> BoxFuture<'static, Result<IpcBody, ForgeError>> {
                let func = self.clone();
                Box::pin(async move {
                    $(
                        let $E = $E::from_context(&ctx)?;
                    )*
                    let res = func($($E,)*).await.map_err(Into::into)?;
                    serialize_json_reply(res)
                })
            }
        }

        // 2. JSON payload -> JSON reply
        impl<Func, Fut, Req, Res, Err, $($E,)*> CommandHandler<(WithPayload, ($($E,)*), Req)> for Func
        where
            Func: Fn($($E,)* Req) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = Result<Res, Err>> + Send + 'static,
            Req: DeserializeOwned + Send + 'static,
            Res: Serialize + Send + 'static,
            Err: Into<ForgeError> + Send + 'static,
            $($E: FromContext + Send + 'static,)*
        {
            #[allow(unused_mut)]
            fn required_states() -> Vec<(TypeId, &'static str)> {
                let mut reqs = Vec::new();
                $(
                    if let Some(item) = $E::required_state_type() {
                        reqs.push(item);
                    }
                )*
                reqs
            }

            #[allow(unused_variables, non_snake_case)]
            fn call(&self, ctx: InvokeContext, body: IpcBody) -> BoxFuture<'static, Result<IpcBody, ForgeError>> {
                let func = self.clone();
                Box::pin(async move {
                    $(
                        let $E = $E::from_context(&ctx)?;
                    )*
                    let req: Req = deserialize_json_body(body)?;
                    let res = func($($E,)* req).await.map_err(Into::into)?;
                    serialize_json_reply(res)
                })
            }
        }

        // 3. RawBytes payload -> JSON reply
        impl<Func, Fut, Res, Err, $($E,)*> CommandHandler<(BinaryPayload, ($($E,)*))> for Func
        where
            Func: Fn($($E,)* RawBytes) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = Result<Res, Err>> + Send + 'static,
            Res: Serialize + Send + 'static,
            Err: Into<ForgeError> + Send + 'static,
            $($E: FromContext + Send + 'static,)*
        {
            #[allow(unused_mut)]
            fn required_states() -> Vec<(TypeId, &'static str)> {
                let mut reqs = Vec::new();
                $(
                    if let Some(item) = $E::required_state_type() {
                        reqs.push(item);
                    }
                )*
                reqs
            }

            #[allow(unused_variables, non_snake_case)]
            fn call(&self, ctx: InvokeContext, body: IpcBody) -> BoxFuture<'static, Result<IpcBody, ForgeError>> {
                let func = self.clone();
                Box::pin(async move {
                    $(
                        let $E = $E::from_context(&ctx)?;
                    )*
                    let raw = extract_binary_body(body)?;
                    let res = func($($E,)* raw).await.map_err(Into::into)?;
                    serialize_json_reply(res)
                })
            }
        }

        // 4. No payload -> RawBytes reply
        impl<Func, Fut, Err, $($E,)*> CommandHandler<(NoPayloadBinaryReply, ($($E,)*))> for Func
        where
            Func: Fn($($E,)*) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = Result<RawBytes, Err>> + Send + 'static,
            Err: Into<ForgeError> + Send + 'static,
            $($E: FromContext + Send + 'static,)*
        {
            #[allow(unused_mut)]
            fn required_states() -> Vec<(TypeId, &'static str)> {
                let mut reqs = Vec::new();
                $(
                    if let Some(item) = $E::required_state_type() {
                        reqs.push(item);
                    }
                )*
                reqs
            }

            #[allow(unused_variables, non_snake_case)]
            fn call(&self, ctx: InvokeContext, _body: IpcBody) -> BoxFuture<'static, Result<IpcBody, ForgeError>> {
                let func = self.clone();
                Box::pin(async move {
                    $(
                        let $E = $E::from_context(&ctx)?;
                    )*
                    let res: RawBytes = func($($E,)*).await.map_err(Into::into)?;
                    Ok(IpcBody::Bytes(res.0))
                })
            }
        }

        // 5. JSON payload -> RawBytes reply
        impl<Func, Fut, Req, Err, $($E,)*> CommandHandler<(WithPayloadBinaryReply, ($($E,)*), Req)> for Func
        where
            Func: Fn($($E,)* Req) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = Result<RawBytes, Err>> + Send + 'static,
            Req: DeserializeOwned + Send + 'static,
            Err: Into<ForgeError> + Send + 'static,
            $($E: FromContext + Send + 'static,)*
        {
            #[allow(unused_mut)]
            fn required_states() -> Vec<(TypeId, &'static str)> {
                let mut reqs = Vec::new();
                $(
                    if let Some(item) = $E::required_state_type() {
                        reqs.push(item);
                    }
                )*
                reqs
            }

            #[allow(unused_variables, non_snake_case)]
            fn call(&self, ctx: InvokeContext, body: IpcBody) -> BoxFuture<'static, Result<IpcBody, ForgeError>> {
                let func = self.clone();
                Box::pin(async move {
                    $(
                        let $E = $E::from_context(&ctx)?;
                    )*
                    let req: Req = deserialize_json_body(body)?;
                    let res: RawBytes = func($($E,)* req).await.map_err(Into::into)?;
                    Ok(IpcBody::Bytes(res.0))
                })
            }
        }

        // 6. RawBytes payload -> RawBytes reply
        impl<Func, Fut, Err, $($E,)*> CommandHandler<(BinaryInOut, ($($E,)*))> for Func
        where
            Func: Fn($($E,)* RawBytes) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = Result<RawBytes, Err>> + Send + 'static,
            Err: Into<ForgeError> + Send + 'static,
            $($E: FromContext + Send + 'static,)*
        {
            #[allow(unused_mut)]
            fn required_states() -> Vec<(TypeId, &'static str)> {
                let mut reqs = Vec::new();
                $(
                    if let Some(item) = $E::required_state_type() {
                        reqs.push(item);
                    }
                )*
                reqs
            }

            #[allow(unused_variables, non_snake_case)]
            fn call(&self, ctx: InvokeContext, body: IpcBody) -> BoxFuture<'static, Result<IpcBody, ForgeError>> {
                let func = self.clone();
                Box::pin(async move {
                    $(
                        let $E = $E::from_context(&ctx)?;
                    )*
                    let raw = extract_binary_body(body)?;
                    let res: RawBytes = func($($E,)* raw).await.map_err(Into::into)?;
                    Ok(IpcBody::Bytes(res.0))
                })
            }
        }
    };
}

impl_command_handler!();
impl_command_handler!(E1);
impl_command_handler!(E1, E2);
impl_command_handler!(E1, E2, E3);
impl_command_handler!(E1, E2, E3, E4);
