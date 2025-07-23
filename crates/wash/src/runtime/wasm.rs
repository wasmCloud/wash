//! Lower level utilities to lift and lower values between two components. Primarily used to
//! link imports of components to exports of other components in the plugin system.

use std::sync::Arc;

use anyhow::{Context as _, ensure};
use tracing::trace;
use wasmtime::component::types::ComponentItem;
use wasmtime::component::{Component, Linker, ResourceAny, ResourceType, Val};
use wasmtime::{AsContextMut as _, StoreContextMut};
use wasmtime_wasi::IoView as _;

use crate::runtime::{Ctx, DevPluginManager};

/// This function plugs a components imports with the exports of other components
/// that are already loaded in the plugin system.
pub fn link_imports_plugin_exports(
    linker: &mut Linker<Ctx>,
    component: &Component,
    plugin_manager: Arc<DevPluginManager>,
) -> anyhow::Result<()> {
    let ty = component.component_type();
    let imports: Vec<_> = ty.imports(component.engine()).collect();
    for (import_name, import_item) in imports.into_iter() {
        match import_item {
            ComponentItem::ComponentInstance(import_instance_ty) => {
                trace!(name = import_name, "processing component instance import");
                let (plugin_component, instance_idx) = {
                    let Some(plugin_component) = plugin_manager.get_component(import_name) else {
                        trace!(name = import_name, "import not found in plugins, skipping");
                        continue;
                    };
                    let Some((ComponentItem::ComponentInstance(_), idx)) =
                        plugin_component.export_index(None, import_name)
                    else {
                        trace!(name = import_name, "skipping non-instance import");
                        continue;
                    };
                    (plugin_component, idx)
                };
                trace!(name = import_name, index = ?instance_idx, "found import at index");

                // Preinstantiate the instance so we can use it later
                let pre = plugin_manager.preinstantiate_instance(linker, import_name)?;

                let mut linker_instance = match linker.instance(import_name) {
                    Ok(i) => i,
                    Err(e) => {
                        trace!(name = import_name, error = %e, "error finding instance in linker, skipping");
                        continue;
                    }
                };

                for (export_name, export_ty) in
                    import_instance_ty.exports(plugin_component.engine())
                {
                    match export_ty {
                        ComponentItem::ComponentFunc(_func_ty) => {
                            let (item, func_idx) = match plugin_component
                                .export_index(Some(&instance_idx), export_name)
                            {
                                Some(res) => res,
                                None => {
                                    trace!(
                                        name = import_name,
                                        fn_name = export_name,
                                        "failed to get export index, skipping"
                                    );
                                    continue;
                                }
                            };
                            ensure!(
                                matches!(item, ComponentItem::ComponentFunc(..)),
                                "expected function export, found other"
                            );
                            trace!(
                                name = import_name,
                                fn_name = export_name,
                                "linking function import"
                            );
                            let import_name: Arc<str> = import_name.into();
                            let export_name: Arc<str> = export_name.into();
                            let pre = pre.clone();
                            let plugin_manager = plugin_manager.clone();
                            linker_instance
                                .func_new_async(
                                    &export_name.clone(),
                                    move |mut store, params, results| {
                                        let import_name = import_name.clone();
                                        let export_name = export_name.clone();
                                        let plugin_manager = plugin_manager.clone();
                                        let pre = pre.clone();
                                        Box::new(async move {
                                            let instance = plugin_manager
                                                .get_or_instantiate_instance(
                                                    &mut store,
                                                    pre,
                                                    &import_name,
                                                )
                                                .await?;

                                            let func = instance
                                                .get_func(&mut store, func_idx)
                                                .context("function not found")?;
                                            trace!(
                                                name = %import_name,
                                                fn_name = %export_name,
                                                ?params,
                                                "lowering params"
                                            );
                                            let mut params_buf = Vec::with_capacity(params.len());
                                            for param_val in params.iter() {
                                                params_buf.push(
                                                    lower(&mut store, param_val)
                                                        .context("failed to lower parameter")?,
                                                );
                                            }
                                            trace!(
                                                name = %import_name,
                                                fn_name = %export_name,
                                                ?params_buf,
                                                "invoking dynamic export"
                                            );

                                            let mut results_buf =
                                                vec![Val::Bool(false); results.len()];
                                            // TODO(IMPORTANT): Enforce a timeout on this call
                                            // to prevent hanging indefinitely.
                                            func.call_async(
                                                &mut store,
                                                &params_buf,
                                                &mut results_buf,
                                            )
                                            .await
                                            .context("failed to call function")?;
                                            trace!(
                                                name = %import_name,
                                                fn_name = %export_name,
                                                ?results_buf,
                                                "lifting results"
                                            );
                                            for (i, result_val) in
                                                results_buf.into_iter().enumerate()
                                            {
                                                results[i] = lift(&mut store, result_val)
                                                    .context("failed to lift result")?;
                                            }
                                            trace!(
                                                name = %import_name,
                                                fn_name = %export_name,
                                                ?results,
                                                "invoked dynamic export"
                                            );

                                            func.post_return_async(&mut store)
                                                .await
                                                .context("failed to execute post-return")?;
                                            Ok(())
                                        })
                                    },
                                )
                                .expect("failed to create async func");
                        }
                        ComponentItem::Resource(resource_ty) => {
                            if is_host_resource_type(resource_ty) {
                                trace!(name = import_name, resource = ?resource_ty, "skipping host resource type");
                                continue;
                            }

                            let (item, _idx) = match plugin_component
                                .export_index(Some(&instance_idx), export_name)
                            {
                                Some(res) => res,
                                None => {
                                    trace!(
                                        name = import_name,
                                        resource = export_name,
                                        "failed to get resource index, skipping"
                                    );
                                    continue;
                                }
                            };
                            let ComponentItem::Resource(_) = item else {
                                trace!(
                                    name = import_name,
                                    resource = export_name,
                                    "expected resource export, found non-resource, skipping"
                                );
                                continue;
                            };

                            // TODO(ISSUE#4): This should get caught by the above host resource check, gotta figure that out
                            if export_name == "output-stream"
                                || export_name == "input-stream"
                                || export_name == "incoming-value-async-body"
                            {
                                trace!(
                                    name = import_name,
                                    resource = export_name,
                                    "skipping stream link as it is a host resource type"
                                );
                                continue;
                            }

                            trace!(name = import_name, resource = export_name, ty = ?resource_ty, "linking resource import");

                            linker_instance
                                .resource(export_name, ResourceType::host::<ResourceAny>(), |_, _| Ok(()))
                                .with_context(|| {
                                    format!(
                                        "failed to define resource import: {import_name}.{export_name}"
                                    )
                                })
                                .unwrap_or_else(|e| {
                                    trace!(name = import_name, resource = export_name, error = %e, "error defining resource import, skipping");
                                });
                        }
                        _ => {
                            trace!(
                                name = import_name,
                                fn_name = export_name,
                                "skipping non-function non-resource import"
                            );
                            continue;
                        }
                    }
                }
            }
            ComponentItem::Resource(resource_ty) => {
                trace!(
                    name = import_name,
                    ty = ?resource_ty,
                    "component import is a resource, which is not supported in this context. skipping."
                );
            }
            _ => continue,
        }
    }
    Ok(())
}

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
            if let Ok(res) = any
                .try_into_resource::<wasmtime_wasi::bindings::io::streams::OutputStream>(
                    store.as_context_mut(),
                )
            {
                trace!("lowering output stream");
                let stream = store.data_mut().table().delete(res)?;
                let resource = store.data_mut().table().push(stream)?;
                Ok(Val::Resource(
                    resource.try_into_resource_any(store.as_context_mut())?,
                ))
            } else {
                trace!(resource = ?any, "lowering resource");
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
            if let Ok(res) = any
                .try_into_resource::<wasmtime_wasi::bindings::io::streams::OutputStream>(
                    store.as_context_mut(),
                )
            {
                trace!("lifting output stream");
                let stream = store.data_mut().table().delete(res)?;
                let resource = store.data_mut().table().push(stream)?;

                Ok(Val::Resource(
                    resource.try_into_resource_any(store.as_context_mut())?,
                ))
            } else if let Ok(res) = any
                .try_into_resource::<wasmtime_wasi::bindings::io::streams::InputStream>(
                    store.as_context_mut(),
                )
            {
                trace!("lifting input stream");
                let stream = store.data_mut().table().delete(res)?;
                let resource = store.data_mut().table().push(stream)?;

                Ok(Val::Resource(
                    resource.try_into_resource_any(store.as_context_mut())?,
                ))
            } else {
                trace!(resource = ?any, "lifting resource");
                // Resource lifting logic: push the resource into the store's table and return a new Val::Resource
                let res = store.data_mut().table().push(any)?;
                Ok(Val::Resource(
                    res.try_into_resource_any(store.as_context_mut())?,
                ))
            }
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
