use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::ops::Deref;
use std::pin::Pin;
use std::sync::Arc;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::config::{AppConfig, Config};
use crate::error::ForgeError;
use crate::runtime::WindowHandle;

pub(crate) type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

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
    pub(crate) window: WindowHandle,
}

mod sealed {
    pub trait Sealed {}
}

/// Sealed extractor trait for context arguments (`State<T>`, `WindowHandle`, `AppConfig`)
/// that are injected by FORGE without consuming the JSON request payload.
pub trait FromContext: sealed::Sealed + Sized {
    #[doc(hidden)]
    fn required_state_type() -> Option<(TypeId, &'static str)> {
        None
    }

    #[doc(hidden)]
    fn from_context(ctx: &InvokeContext) -> Result<Self, ForgeError>;
}

/// Shared application state extractor (`Clone` + `Deref<Target = T>`).
pub struct State<T: ?Sized>(pub(crate) Arc<T>);

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

#[doc(hidden)]
pub struct NoPayload;
#[doc(hidden)]
pub struct WithPayload;

/// Zero-macro async command handler trait supporting context extractors and an optional
/// deserializable payload parameter.
pub trait CommandHandler<Marker>: Send + Sync + 'static {
    #[doc(hidden)]
    fn required_states() -> Vec<(TypeId, &'static str)>;

    #[doc(hidden)]
    fn call(
        &self,
        ctx: InvokeContext,
        payload: Value,
    ) -> BoxFuture<'static, Result<Value, ForgeError>>;
}

pub(crate) trait ErasedCommand: Send + Sync + 'static {
    fn required_states(&self) -> Vec<(TypeId, &'static str)>;
    fn call(
        &self,
        ctx: InvokeContext,
        payload: Value,
    ) -> BoxFuture<'static, Result<Value, ForgeError>>;
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
        payload: Value,
    ) -> BoxFuture<'static, Result<Value, ForgeError>> {
        self.handler.call(ctx, payload)
    }
}

fn deserialize_payload<Req: DeserializeOwned>(payload: Value) -> Result<Req, ForgeError> {
    serde_json::from_value(payload).map_err(|err| {
        ForgeError::bad_request(
            "FORGE_INVALID_PAYLOAD",
            format!("invalid command payload: {err}"),
        )
    })
}

fn serialize_reply<Res: Serialize>(reply: Res) -> Result<Value, ForgeError> {
    serde_json::to_value(reply).map_err(|err| {
        ForgeError::new(
            "FORGE_SERIALIZE_ERROR",
            format!("failed to serialize command reply: {err}"),
        )
    })
}

macro_rules! impl_command_handler {
    ($($E:ident),*) => {
        // 1. Handlers with NO payload parameter: `async fn(E1, ..., En) -> Result<Res, Err>`
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
            fn call(&self, ctx: InvokeContext, _payload: Value) -> BoxFuture<'static, Result<Value, ForgeError>> {
                let func = self.clone();
                Box::pin(async move {
                    $(
                        let $E = $E::from_context(&ctx)?;
                    )*
                    let res = func($($E,)*).await.map_err(Into::into)?;
                    serialize_reply(res)
                })
            }
        }

        // 2. Handlers WITH a trailing payload parameter: `async fn(E1, ..., En, Req) -> Result<Res, Err>`
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
            fn call(&self, ctx: InvokeContext, payload: Value) -> BoxFuture<'static, Result<Value, ForgeError>> {
                let func = self.clone();
                Box::pin(async move {
                    $(
                        let $E = $E::from_context(&ctx)?;
                    )*
                    let req: Req = deserialize_payload(payload)?;
                    let res = func($($E,)* req).await.map_err(Into::into)?;
                    serialize_reply(res)
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
