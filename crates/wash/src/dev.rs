use anyhow::{self, Context as _};
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{trace, warn};
use wasmcloud_runtime::Runtime;
use wasmtime::{
    AsContextMut as _, StoreContextMut,
    component::{Component, Instance, InstancePre, Linker, types::ComponentItem},
};

use crate::runtime::Ctx;

/// A hash of the component bytes, type aliased for clarity
type ComponentKey = String;

/// The DevPluginManager is responsible for coordinating the registration and instantiation of
/// plugin components in the development loop.
///
/// At the moment it does not support multiple implementations of the same interface, and only one
/// plugin can be registered for a given interface name (an interface name is wasi:blobstore/blobstore@0.2.0-draft,
/// for example).
///
/// This struct intentionally doesn't derive clone as it should be Arc-ed to share it across threads.
#[derive(Default)]
pub struct DevPluginManager {
    /// Map from interface name → component key (deduplicates lookups)
    interface_map: HashMap<String, ComponentKey>,
    /// Map from component key → compiled component
    components: HashMap<ComponentKey, Component>,
    /// Map from component id (uuid) → component key (hash) -> instantiated instance (created lazily)
    /// Instances is keyed differently than components, as it's keyed by a specific Instance of a component
    /// This allows us to instantiate the same component multiple times with different configurations.
    instances: RwLock<HashMap<String, HashMap<ComponentKey, Instance>>>,
}

impl DevPluginManager {
    /// Register a plugin's exported instances to be available for instantiation. This does not instantiate
    /// the plugin, but rather prepares it for use by compiling the Wasm bytes into a [`Component`].
    ///
    /// This only skips the `wasmcloud:wash/plugin` export, which is used internally by the plugin system.
    pub fn register_plugin(&mut self, runtime: &Runtime, wasm: &[u8]) -> anyhow::Result<()> {
        let (component, exported_instances) = compile_plugin_component(runtime, wasm)?;

        // The component key is simply a hash of the Wasm bytes
        let component_key = format!("{:x}", Sha256::digest(wasm));
        for (name, item) in exported_instances {
            // We don't need to expose the plugin export to the dev components
            // Additionally, this would actually error since each plugin exports this interface.
            // TODO: It's probably a good idea to skip registering wasi@0.2 interfaces
            match name.split_once('@') {
                Some(("wasmcloud:wash/plugin", _)) => {
                    trace!(name, "skipping internal plugin export");
                    continue;
                }
                None => {
                    if name == "wasmcloud:wash/plugin" {
                        trace!(name, "skipping internal plugin export");
                        continue;
                    }
                }
                _ => {}
            }
            if let ComponentItem::ComponentInstance(_) = item {
                // Register the interface name to the component key
                if self.interface_map.contains_key(&name) {
                    anyhow::bail!("another plugin already implements the interface '{name}'");
                }
                self.interface_map
                    .insert(name.clone(), component_key.clone());
            } else {
                warn!(name, "exported item is not a component instance, skipping");
            }
        }

        self.components.insert(component_key, component);

        Ok(())
    }

    /// Looks up a component for an interface name, or returns None if not found.
    pub fn get_component(&self, name: &str) -> Option<Component> {
        let key = self.interface_map.get(name)?;
        self.components.get(key).cloned()
    }

    /// Preinstantiate an instance of a component for a given interface name.
    pub fn preinstantiate_instance(
        &self,
        linker: &Linker<Ctx>,
        name: &str,
    ) -> anyhow::Result<InstancePre<Ctx>> {
        trace!(name, "preinstantiating instance for component");
        let component = self.get_component(name).context("component not found")?;
        let instance = linker.instantiate_pre(&component)?;
        Ok(instance)
    }

    /// Get or instantiate an instance of a component for a given interface name.
    ///
    /// This is designed to be used in the development loop, where components are instantiated lazily. It also
    /// makes an assumption that instances exporting multiple different interfaces will only be instantiated once per component.
    pub async fn get_or_instantiate_instance(
        &self,
        store: &mut StoreContextMut<'_, Ctx>,
        pre: InstancePre<Ctx>,
        name: &str,
    ) -> anyhow::Result<Instance> {
        let key = self
            .interface_map
            .get(name)
            .context("component not found")?;

        // This is cloned to avoid holding a reference to the store
        let id = store.data().id.clone();

        if let Some(instances) = self.instances.read().await.get(&id) {
            if let Some(instance) = instances.get(key) {
                trace!(id, "found existing instance for component");
                return Ok(*instance);
            }
        }

        let instance = pre.instantiate_async(store.as_context_mut()).await?;
        trace!(id, key, "instantiated new instance for component");

        self.instances
            .write()
            .await
            .entry(id.to_string())
            .and_modify(|e| {
                e.insert(key.to_owned(), instance);
            })
            .or_insert(HashMap::from([(key.to_owned(), instance)]));

        Ok(instance)
    }

    /// Clone this DevManager and clear the instances map, removing all instantiated plugin components.
    ///
    /// Must be done before each instantiation of a component to ensure that the
    /// plugins are using the same store as the instantiated component.
    pub fn clear_instances(self: Arc<Self>) -> Arc<Self> {
        Arc::new(Self {
            interface_map: self.interface_map.clone(),
            components: self.components.clone(),
            instances: RwLock::new(HashMap::new()),
        })
    }
}

/// Compiles a [`Component`] from the provided wasm bytes and extracts the
/// [`ComponentItem::ComponentInstance`]s that it exports. This is used primarily to
/// allow the plugin system to use these exports to fil
///
/// Returns a tuple containing the compiled [`Component`] and a vector of tuples
/// where each tuple contains the name of the exported component instance and the
/// corresponding [`ComponentItem`].
fn compile_plugin_component(
    runtime: &Runtime,
    wasm: &[u8],
) -> anyhow::Result<(Component, Vec<(String, ComponentItem)>)> {
    let component = wasmtime::component::Component::new(runtime.engine(), wasm)
        .context("failed to create component from wasm")?;
    let exports = component
        .component_type()
        .exports(component.engine())
        .filter_map(|(name, item)| {
            // An instance in this case is something like `wasi:blobstore/blobstore@0.2.0-draft`, or an
            // entire interface that's exported.
            if matches!(item, ComponentItem::ComponentInstance(_)) {
                Some((name.to_string(), item))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    Ok((component, exports))
}
