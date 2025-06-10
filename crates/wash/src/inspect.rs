use std::{collections::HashSet, io::Read, path::Path};

use anyhow::{Context, ensure};
use wit_component::{DecodedWasm, OutputToString};
use wit_parser::{Resolve, WorldId};

pub async fn load_component(component_path: &Path) -> anyhow::Result<Vec<u8>> {
    ensure!(component_path.exists(), "Component path does not exist");

    tokio::fs::read(&component_path).await.with_context(|| {
        format!(
            "failed to read component file: {}",
            component_path.display()
        )
    })
}

pub async fn decode_component(component_bytes: impl Read) -> anyhow::Result<DecodedWasm> {
    wit_component::decode_reader(component_bytes).context("failed to decode component bytes")
}

pub async fn print_component_wit(component: DecodedWasm) -> anyhow::Result<String> {
    let resolve = component.resolve();
    let main = component.package();

    let mut printer = wit_component::WitPrinter::new(OutputToString::default());
    printer
        .print(resolve, main, &[])
        .context("failed to print WIT world from a component")?;

    Ok(printer.output.to_string())
}

/// A parsed representation of a WIT package, interface, function and version.
///
/// For example: `wasi:keyvalue/atomics.increment@0.2.0-draft`:
/// -  `wasi` is the namespace,
/// -  `keyvalue` is the package name,
/// -  `atomics` is the interface,
/// -  `increment` is the function,
/// -  `0.2.0-draft` is the version.
///
/// Interfaces, function and version are optional for maximum flexibility of this struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitSpec {
    /// WIT namespace
    pub namespace: String,
    /// WIT package name
    pub package: String,
    /// WIT interfaces, if omitted will be used to match any interface
    pub interfaces: Option<HashSet<String>>,
    /// WIT interface function
    pub function: Option<String>,
    /// Version of WIT interface
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyKind {
    Import,
    Export,
}

/// A component that has been inspected for its WIT dependencies
pub struct InspectedComponent {
    pub component_bytes: Vec<u8>,
    pub resolve: Resolve,
    pub world_id: WorldId,
    pub imports: Vec<DependencySpec>,
    pub exports: Vec<DependencySpec>,
}

/// Specification for a single dependency in a given project
///
/// [`DependencySpec`]s are normally gleaned from some source of project metadata, for example:
///
/// - dependency overrides in a project-level `wasmcloud.toml`
/// - dependency overrides in a workspace-level `wasmcloud.toml`
/// - WIT interface of a project
///
/// A `DependencySpec` represents a single dependency in the project, categorized into what part it is expected
/// to play in in fulfilling WIT interfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencySpec {
    /// Specification of the WIT interface that this dependency fulfills.
    ///
    /// Note that this specification may cover *more than one* interface.
    pub(crate) wit: WitSpec,

    /// The kind of dependency this is, either an import or an export
    pub(crate) kind: DependencyKind,

    /// Whether this dependency should delegated to the workspace
    pub(crate) delegated_to_workspace: bool,

    /// Image reference to the component that should be inserted/used
    ///
    /// This reference *can* be missing if an override is specified with no image,
    /// which can happen in at least two cases:
    /// - custom WIT-defined interface imports/exports (we may not know at WIT processing what their overrides will be)
    /// - project-level with workspace delegation  (we may not know what the image ref is at the project level)
    pub(crate) image_ref: Option<String>,

    /// Whether this dependency represents a WebAssembly component, rather than a (standalone binary) provider
    pub(crate) is_component: bool,

    /// The link name this dependency should be connected over
    ///
    /// In the vast majority of cases, this will be "default", but it may not be
    /// if the same interface is used to link to multiple different providers/components
    pub(crate) link_name: String,
    // /// Configurations that must be created and/or consumed by this dependency
    // pub(crate) configs: Vec<wadm_types::ConfigProperty>,

    // /// Configurations for links that are created for this dependency
    // pub(crate) link_configs: Vec<wadm_types::ConfigProperty>,

    // /// Secrets that must be created and/or consumed by this dependency
    // ///
    // /// [`SecretProperty`] here support a special `policy` value which is 'env'.
    // /// Paired with a key that looks like "$`SOME_VALUE`", the value will be extracted from ENV *prior* and
    // pub(crate) secrets: Vec<wadm_types::SecretProperty>,
}

// Resolve the dependencies of a given WIT world that map to WADM components
//
// Normally, this means converting imports that the component depends on to
// components that can be run on the lattice.
// pub fn discover_dependencies_from_wit(
//     resolve: Resolve,
//     world_id: WorldId,
// ) -> anyhow::Result<Vec<DependencySpec>> {
//     let deps: Vec<DependencySpec> = Vec::new();
//     // let mut deps: Vec<DependencySpec> = Vec::new();
//     let world = resolve
//         .worlds
//         .get(world_id)
//         .context("selected WIT world is missing")?;
//     // Process imports
//     for (_key, item) in &world.imports {
//         if let wit_parser::WorldItem::Interface { id, .. } = item {
//             let iface = resolve
//                 .interfaces
//                 .get(*id)
//                 .context("unexpectedly missing iface")?;
//             let pkg = resolve
//                 .packages
//                 .get(iface.package.context("iface missing package")?)
//                 .context("failed to find package")?;
//             let interface_name = iface.name.as_ref().context("interface missing name")?;
//             // let iface_name = &format!("{}:{}/{interface_name}", pkg.name.namespace, pkg.name.name,);

//             // if let Some(new_dep) = DependencySpec::from_wit_import_iface(iface_name) {
//             //     // If the dependency already exists for a different interface, add the interface to the existing dependency
//             //     if let Some(DependencySpec::Receives(ref mut dep)) = deps.iter_mut().find(|d| {
//             //         d.wit().namespace == pkg.name.namespace && d.wit().package == pkg.name.name
//             //     }) {
//             //         if let Some(ref mut interfaces) = dep.wit.interfaces {
//             //             interfaces.insert(interface_name.to_string());
//             //         } else {
//             //             dep.wit.interfaces = Some(HashSet::from([iface_name.to_string()]));
//             //         }
//             //     } else {
//             //         deps.push(new_dep);
//             //     }
//             // }
//         }
//     }
//     // Process exports
//     for (_key, item) in &world.exports {
//         if let wit_parser::WorldItem::Interface { id, .. } = item {
//             // let iface = resolve
//             //     .interfaces
//             //     .get(*id)
//             //     .context("unexpectedly missing iface")?;
//             // let pkg = resolve
//             //     .packages
//             //     .get(iface.package.context("iface missing package")?)
//             //     .context("failed to find package")?;
//             // let interface_name = iface.name.as_ref().context("interface missing name")?;
//             // let iface_name = &format!("{}:{}/{interface_name}", pkg.name.namespace, pkg.name.name,);
//             // if let Some(new_dep) = DependencySpec::from_wit_export_iface(iface_name) {
//             //     // If the dependency already exists for a different interface, add the interface to the existing dependency
//             //     if let Some(DependencySpec::Invokes(ref mut dep)) = deps.iter_mut().find(|d| {
//             //         d.wit().namespace == pkg.name.namespace && d.wit().package == pkg.name.name
//             //     }) {
//             //         if let Some(ref mut interfaces) = dep.wit.interfaces {
//             //             // SAFETY: we already checked at `iface.name`
//             //             interfaces.insert(interface_name.to_string());
//             //         } else {
//             //             dep.wit.interfaces = Some(HashSet::from([iface_name.to_string()]));
//             //         }
//             //     } else {
//             //         deps.push(new_dep);
//             //     }
//             // }
//         }
//     }

//     Ok(deps)
// }

#[cfg(test)]
mod test {
    use std::{fs::File, path::Path};

    use crate::inspect::{decode_component, load_component, print_component_wit};

    #[tokio::test]
    async fn can_load_and_decode_component() {
        let component_path = Path::new("./tests/fixtures/http_hello_world_rust.wasm");
        let component_bytes = load_component(component_path)
            .await
            .expect("should be able to load component");
        let decoded = decode_component(component_bytes.as_slice())
            .await
            .expect("should be able to decode component");
        let wit = print_component_wit(decoded)
            .await
            .expect("should be able to print component WIT");
        eprintln!("{wit}");
    }

    #[tokio::test]
    async fn can_load_and_decode_component_reader() {
        let component_file = File::open("./tests/fixtures/http_hello_world_rust.wasm")
            .expect("should be able to open component file");
        let decoded = decode_component(&component_file)
            .await
            .expect("should be able to decode component");
        let wit = print_component_wit(decoded)
            .await
            .expect("should be able to print component WIT");
        eprintln!("{wit}");
    }
}
