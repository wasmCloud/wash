use anyhow::Context as _;
use wasmtime::component::{ResourceType, Val};
use wasmtime::{AsContextMut as _, StoreContextMut};
use wasmtime_wasi::IoView as _;

use crate::runtime::Ctx;

// Thanks wasmlet

pub fn lower(store: &mut StoreContextMut<'_, Ctx>, v: &Val) -> anyhow::Result<Val> {
    match v {
        &Val::Bool(v) => Ok(Val::Bool(v)),
        &Val::S8(v) => Ok(Val::S8(v)),
        &Val::U8(v) => Ok(Val::U8(v)),
        &Val::S16(v) => Ok(Val::S16(v)),
        &Val::U16(v) => Ok(Val::U16(v)),
        &Val::S32(v) => Ok(Val::S32(v)),
        &Val::U32(v) => Ok(Val::U32(v)),
        &Val::S64(v) => Ok(Val::S64(v)),
        &Val::U64(v) => Ok(Val::U64(v)),
        &Val::Float32(v) => Ok(Val::Float32(v)),
        &Val::Float64(v) => Ok(Val::Float64(v)),
        &Val::Char(v) => Ok(Val::Char(v)),
        Val::String(v) => Ok(Val::String(v.clone())),
        Val::List(vs) => {
            let vs = vs
                .iter()
                .map(|v| lower(store, v))
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::List(vs))
        }
        Val::Record(vs) => {
            let vs = vs
                .iter()
                .map(|(name, v)| {
                    let v = lower(store, v)?;
                    Ok((name.clone(), v))
                })
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::Record(vs))
        }
        Val::Tuple(vs) => {
            let vs = vs
                .iter()
                .map(|v| lower(store, v))
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::Tuple(vs))
        }
        Val::Variant(k, v) => {
            if let Some(v) = v {
                let v = lower(store, v)?;
                Ok(Val::Variant(k.clone(), Some(Box::new(v))))
            } else {
                Ok(Val::Variant(k.clone(), None))
            }
        }
        Val::Enum(v) => Ok(Val::Enum(v.clone())),
        Val::Option(v) => {
            if let Some(v) = v {
                let v = lower(store, v)?;
                Ok(Val::Option(Some(Box::new(v))))
            } else {
                Ok(Val::Option(None))
            }
        }
        Val::Result(v) => match v {
            Ok(v) => {
                if let Some(v) = v {
                    let v = lower(store, v)?;
                    Ok(Val::Result(Ok(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Ok(None)))
                }
            }
            Err(v) => {
                if let Some(v) = v {
                    let v = lower(store, v)?;
                    Ok(Val::Result(Err(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Err(None)))
                }
            }
        },
        Val::Flags(v) => Ok(Val::Flags(v.clone())),
        &Val::Resource(any) => {
            let res = any
                .try_into_resource(store.as_context_mut())
                .context("failed to lower resource")?;
            let table = store.data_mut().table();
            if res.owned() {
                Ok(Val::Resource(
                    table.delete(res).context("owned resource not in table")?,
                ))
            } else {
                Ok(Val::Resource(
                    table
                        .get(&res)
                        .context("borrowed resource not in table")
                        .cloned()?,
                ))
            }
        }
    }
}

pub fn lift(store: &mut StoreContextMut<'_, Ctx>, v: Val) -> anyhow::Result<Val> {
    match v {
        Val::Bool(v) => Ok(Val::Bool(v)),
        Val::S8(v) => Ok(Val::S8(v)),
        Val::U8(v) => Ok(Val::U8(v)),
        Val::S16(v) => Ok(Val::S16(v)),
        Val::U16(v) => Ok(Val::U16(v)),
        Val::S32(v) => Ok(Val::S32(v)),
        Val::U32(v) => Ok(Val::U32(v)),
        Val::S64(v) => Ok(Val::S64(v)),
        Val::U64(v) => Ok(Val::U64(v)),
        Val::Float32(v) => Ok(Val::Float32(v)),
        Val::Float64(v) => Ok(Val::Float64(v)),
        Val::Char(v) => Ok(Val::Char(v)),
        Val::String(v) => Ok(Val::String(v)),
        Val::List(vs) => {
            let vs = vs
                .into_iter()
                .map(|v| lift(store, v))
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::List(vs))
        }
        Val::Record(vs) => {
            let vs = vs
                .into_iter()
                .map(|(name, v)| {
                    let v = lift(store, v)?;
                    Ok((name, v))
                })
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::Record(vs))
        }
        Val::Tuple(vs) => {
            let vs = vs
                .into_iter()
                .map(|v| lift(store, v))
                .collect::<anyhow::Result<_>>()?;
            Ok(Val::Tuple(vs))
        }
        Val::Variant(k, v) => {
            if let Some(v) = v {
                let v = lift(store, *v)?;
                Ok(Val::Variant(k, Some(Box::new(v))))
            } else {
                Ok(Val::Variant(k, None))
            }
        }
        Val::Enum(v) => Ok(Val::Enum(v)),
        Val::Option(v) => {
            if let Some(v) = v {
                let v = lift(store, *v)?;
                Ok(Val::Option(Some(Box::new(v))))
            } else {
                Ok(Val::Option(None))
            }
        }
        Val::Result(v) => match v {
            Ok(v) => {
                if let Some(v) = v {
                    let v = lift(store, *v)?;
                    Ok(Val::Result(Ok(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Ok(None)))
                }
            }
            Err(v) => {
                if let Some(v) = v {
                    let v = lift(store, *v)?;
                    Ok(Val::Result(Err(Some(Box::new(v)))))
                } else {
                    Ok(Val::Result(Err(None)))
                }
            }
        },
        Val::Flags(v) => Ok(Val::Flags(v)),
        Val::Resource(any) => {
            // Resource lifting logic: push the resource into the store's table and return a new Val::Resource
            let res = store.data_mut().table().push(any)?;
            Ok(Val::Resource(
                res.try_into_resource_any(store.as_context_mut())?,
            ))
        }
    }
}

pub type InputStream = wasmtime_wasi::bindings::io::streams::InputStream;
pub type IoError = wasmtime_wasi::bindings::io::error::Error;
pub type OutputStream = wasmtime_wasi::bindings::io::streams::OutputStream;
pub type Pollable = wasmtime_wasi::bindings::io::poll::Pollable;
pub type TcpSocket = wasmtime_wasi::bindings::sockets::tcp::TcpSocket;

pub fn input_stream() -> ResourceType {
    ResourceType::host::<InputStream>()
}
pub fn io_error() -> ResourceType {
    ResourceType::host::<IoError>()
}
pub fn output_stream() -> ResourceType {
    ResourceType::host::<OutputStream>()
}
pub fn pollable() -> ResourceType {
    ResourceType::host::<Pollable>()
}
pub fn tcp_socket() -> ResourceType {
    ResourceType::host::<TcpSocket>()
}

pub fn is_host_resource_type(ty: ResourceType) -> bool {
    ty == pollable()
        || ty == input_stream()
        || ty == output_stream()
        || ty == io_error()
        || ty == tcp_socket()
}
