//! WebAssembly Interface Types (WIT) support for wasmcloud.
//!
//! This module provides types and utilities for working with WIT interface
//! specifications. WIT is used to describe the capabilities that components
//! require and that plugins provide.
//!
//! # Key Types
//!
//! - [`WitWorld`] - A collection of imports and exports representing a WIT world
//! - [`WitInterface`] - A specific interface specification with namespace, package, and version
//!
//! # Interface Matching
//!
//! The [`WitInterface::contains`] method is used to determine if one interface
//! specification can satisfy another. This is crucial for matching component
//! requirements with plugin capabilities.

use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
};

/// A collection of WIT interfaces representing a world definition.
///
/// A WIT world describes the imports and exports that a component or
/// plugin provides. This is used for capability matching between
/// workloads and plugins.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WitWorld {
    /// The interfaces that this world imports (requires from the host)
    pub imports: HashSet<WitInterface>,
    /// The interfaces that this world exports (provides to the host)
    pub exports: HashSet<WitInterface>,
}

impl WitWorld {
    /// This function checks if the world includes a specific interface. This is
    /// slightly different than checking directly against the imports or exports
    /// because it takes into account the possibility of interface nesting and
    /// versioning. See [`WitInterface::contains`] for more details.
    pub fn includes(&self, interface: &WitInterface) -> bool {
        self.imports.iter().any(|i| i.contains(interface))
            || self.exports.iter().any(|e| e.contains(interface))
    }
}

/// Represents a WIT interface specification with namespace, package, and optional version.
///
/// A `WitInterface` identifies a specific set of interfaces from a WIT package.
/// It follows the format: `namespace:package/interface1,interface2@version`
/// where interfaces and version are optional.
///
/// # Examples
/// - `wasi:http` - Just namespace and package
/// - `wasi:http/incoming-handler` - With a single interface
/// - `wasi:http/incoming-handler,outgoing-handler@0.2.0` - Multiple interfaces with version
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WitInterface {
    /// The namespace of the interface (e.g., "wasi")
    pub namespace: String,
    /// The package name (e.g., "http", "blobstore")
    pub package: String,
    /// The specific interfaces within the package (e.g., "incoming-handler", "types")
    pub interfaces: HashSet<String>,
    // TODO: This is a nice way to represent a version, but it doesn't account for
    // compatible versions. We should revisit this and implement https://docs.rs/semver/1.0.27/semver/struct.VersionReq.html
    /// Optional semantic version for the interface
    pub version: Option<semver::Version>,
    /// Additional configuration parameters for this interface
    pub config: HashMap<String, String>,
}

impl WitInterface {
    /// Returns the instance name of this WitInterface, aka the namespace:package@version
    /// identifier without the interfaces or config.
    pub fn instance(&self) -> String {
        if let Some(v) = &self.version {
            format!("{}:{}@{v}", self.namespace, self.package)
        } else {
            format!("{}:{}", self.namespace, self.package)
        }
    }

    /// Merges another WitInterface into this one, returning a boolean
    /// indicating whether the merge was successful (aka if the [`WitInterface::instance`]s matched).
    pub fn merge(&mut self, other: &WitInterface) -> bool {
        if self.instance() != other.instance() {
            return false;
        }

        self.interfaces.extend(other.interfaces.clone());
        self.config.extend(other.config.clone());
        true
    }

    /// Checks if this interface contains (is a superset of) another interface.
    ///
    /// This method is used to determine if a plugin or component that provides
    /// this interface can satisfy a requirement for the other interface.
    ///
    /// # Arguments
    /// * `other` - The interface to check against
    ///
    /// # Returns
    /// `true` if:
    /// - The namespace and package match exactly
    /// - If this interface has a version, it must match the other's version
    /// - The other's interfaces are a subset of this interface's interfaces
    pub fn contains(&self, other: &WitInterface) -> bool {
        // Namespace and package must match
        if self.namespace != other.namespace || self.package != other.package {
            return false;
        }

        // If both interfaces specify a version, they must match
        if let Some(v) = &self.version
            && let Some(ov) = &other.version
            && v != ov
        {
            return false;
        }

        self.interfaces.is_superset(&other.interfaces)
    }
}

impl Display for WitInterface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.namespace, self.package)?;
        if !self.interfaces.is_empty() && !self.interfaces.is_empty() {
            write!(f, "/")?;
            let interfaces: Vec<_> = self.interfaces.clone().into_iter().collect();
            write!(f, "{}", interfaces.join(","))?;
        }
        if let Some(v) = &self.version {
            write!(f, "@{}", v)?;
        }
        Ok(())
    }
}

impl std::hash::Hash for WitInterface {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.namespace.hash(state);
        self.package.hash(state);
        for iface in &self.interfaces {
            iface.hash(state);
        }
        self.version.hash(state);
        for (k, v) in &self.config {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl From<&str> for WitInterface {
    fn from(s: &str) -> Self {
        // Expected format: namespace:package/interface@version
        // Also supports for convenience: namespace:package/interface,interface2,interface3@version
        // interface and version are optional

        let (main, version) = match s.split_once('@') {
            Some((m, v)) => (m, Some(v)),
            None => (s, None),
        };
        let (namespace_package, interface) = match main.split_once('/') {
            Some((np, iface)) => (np, Some(iface)),
            None => (main, None),
        };
        let (namespace, package) = match namespace_package.split_once(':') {
            Some((ns, pkg)) => (ns, pkg),
            None => ("", namespace_package),
        };
        let interfaces = match interface {
            Some(iface) => iface
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect(),
            None => HashSet::new(),
        };
        let version = version.and_then(|v| semver::Version::parse(v).ok());

        WitInterface {
            namespace: namespace.to_string(),
            package: package.to_string(),
            interfaces,
            version,
            config: HashMap::new(),
        }
    }
}

impl From<String> for WitInterface {
    fn from(s: String) -> Self {
        WitInterface::from(s.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests for From<&str> parsing
    #[test]
    fn test_parse_basic_namespace_package() {
        let wit = WitInterface::from("wasi:blobstore");
        assert_eq!(wit.namespace, "wasi");
        assert_eq!(wit.package, "blobstore");
        assert!(wit.interfaces.is_empty());
        assert!(wit.version.is_none());
    }

    #[test]
    fn test_parse_single_interface() {
        let wit = WitInterface::from("wasi:http/incoming-handler");
        assert_eq!(wit.namespace, "wasi");
        assert_eq!(wit.package, "http");
        assert_eq!(wit.interfaces.len(), 1);
        assert!(wit.interfaces.contains("incoming-handler"));
        assert!(wit.version.is_none());
    }

    #[test]
    fn test_parse_multiple_interfaces() {
        let wit = WitInterface::from("wasi:http/incoming-handler,outgoing-handler,types");
        assert_eq!(wit.namespace, "wasi");
        assert_eq!(wit.package, "http");
        assert_eq!(wit.interfaces.len(), 3);
        assert!(wit.interfaces.contains("incoming-handler"));
        assert!(wit.interfaces.contains("outgoing-handler"));
        assert!(wit.interfaces.contains("types"));
        assert!(wit.version.is_none());
    }

    #[test]
    fn test_parse_with_version() {
        let wit = WitInterface::from("wasi:blobstore/types@0.2.0");
        assert_eq!(wit.namespace, "wasi");
        assert_eq!(wit.package, "blobstore");
        assert_eq!(wit.interfaces.len(), 1);
        assert!(wit.interfaces.contains("types"));
        assert_eq!(wit.version, Some(semver::Version::parse("0.2.0").unwrap()));
    }

    #[test]
    fn test_parse_multiple_interfaces_with_version() {
        let wit = WitInterface::from("wasi:keyvalue/store,atomics,batch@0.2.0-draft");
        assert_eq!(wit.namespace, "wasi");
        assert_eq!(wit.package, "keyvalue");
        assert_eq!(wit.interfaces.len(), 3);
        assert!(wit.interfaces.contains("store"));
        assert!(wit.interfaces.contains("atomics"));
        assert!(wit.interfaces.contains("batch"));
        assert_eq!(
            wit.version,
            Some(semver::Version::parse("0.2.0-draft").unwrap())
        );
    }

    #[test]
    fn test_parse_no_namespace() {
        let wit = WitInterface::from("blobstore/types");
        assert_eq!(wit.namespace, "");
        assert_eq!(wit.package, "blobstore");
        assert_eq!(wit.interfaces.len(), 1);
        assert!(wit.interfaces.contains("types"));
    }

    #[test]
    fn test_parse_no_namespace_with_version() {
        let wit = WitInterface::from("mypackage/interface1,interface2@1.0.0");
        assert_eq!(wit.namespace, "");
        assert_eq!(wit.package, "mypackage");
        assert_eq!(wit.interfaces.len(), 2);
        assert!(wit.interfaces.contains("interface1"));
        assert!(wit.interfaces.contains("interface2"));
        assert_eq!(wit.version, Some(semver::Version::parse("1.0.0").unwrap()));
    }

    #[test]
    fn test_parse_spaces_in_interfaces() {
        let wit = WitInterface::from("wasi:http/incoming-handler, outgoing-handler , types");
        assert_eq!(wit.namespace, "wasi");
        assert_eq!(wit.package, "http");
        assert_eq!(wit.interfaces.len(), 3);
        assert!(wit.interfaces.contains("incoming-handler"));
        assert!(wit.interfaces.contains("outgoing-handler"));
        assert!(wit.interfaces.contains("types"));
    }

    #[test]
    fn test_parse_empty_interface_segments() {
        // Test with trailing comma
        let wit = WitInterface::from("wasi:http/incoming-handler,");
        assert_eq!(wit.interfaces.len(), 1);
        assert!(wit.interfaces.contains("incoming-handler"));

        // Test with leading comma
        let wit2 = WitInterface::from("wasi:http/,incoming-handler");
        assert_eq!(wit2.interfaces.len(), 1);
        assert!(wit2.interfaces.contains("incoming-handler"));

        // Test with double comma
        let wit3 = WitInterface::from("wasi:http/incoming-handler,,outgoing-handler");
        assert_eq!(wit3.interfaces.len(), 2);
        assert!(wit3.interfaces.contains("incoming-handler"));
        assert!(wit3.interfaces.contains("outgoing-handler"));
    }

    #[test]
    fn test_parse_invalid_version() {
        let wit = WitInterface::from("wasi:blobstore/types@invalid-version");
        assert_eq!(wit.namespace, "wasi");
        assert_eq!(wit.package, "blobstore");
        assert!(wit.interfaces.contains("types"));
        assert!(wit.version.is_none()); // Invalid version is ignored
    }

    #[test]
    fn test_parse_prerelease_version() {
        let wit = WitInterface::from("wasi:logging/logging@0.1.0-draft");
        assert_eq!(wit.namespace, "wasi");
        assert_eq!(wit.package, "logging");
        assert!(wit.interfaces.contains("logging"));
        assert_eq!(
            wit.version,
            Some(semver::Version::parse("0.1.0-draft").unwrap())
        );
    }

    #[test]
    fn test_parse_complex_version() {
        let wit = WitInterface::from("wasi:cli/environment@0.2.0-rc.2024-12-05");
        assert_eq!(wit.namespace, "wasi");
        assert_eq!(wit.package, "cli");
        assert!(wit.interfaces.contains("environment"));
        assert_eq!(
            wit.version,
            Some(semver::Version::parse("0.2.0-rc.2024-12-05").unwrap())
        );
    }

    #[test]
    fn test_parse_colon_in_package_name() {
        // Edge case: what if package name itself contains a colon (shouldn't happen but let's test)
        let wit = WitInterface::from("foo:bar:baz/interface");
        assert_eq!(wit.namespace, "foo");
        assert_eq!(wit.package, "bar:baz"); // Everything after first colon becomes package
        assert!(wit.interfaces.contains("interface"));
    }

    #[test]
    fn test_parse_just_package() {
        let wit = WitInterface::from("mypackage");
        assert_eq!(wit.namespace, "");
        assert_eq!(wit.package, "mypackage");
        assert!(wit.interfaces.is_empty());
        assert!(wit.version.is_none());
    }

    #[test]
    fn test_parse_just_package_with_version() {
        let wit = WitInterface::from("mypackage@1.0.0");
        assert_eq!(wit.namespace, "");
        assert_eq!(wit.package, "mypackage");
        assert!(wit.interfaces.is_empty());
        assert_eq!(wit.version, Some(semver::Version::parse("1.0.0").unwrap()));
    }

    // Tests for contains function
    #[test]
    fn test_contains_matching_namespace_and_package() {
        // TRUE: Same namespace and package, no interfaces, no version
        let wit1 = WitInterface::from("wasi:blobstore");
        let wit2 = WitInterface::from("wasi:blobstore");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_different_namespace() {
        // FALSE: Different namespace
        let wit1 = WitInterface::from("wasi:blobstore");
        let wit2 = WitInterface::from("custom:blobstore");
        assert!(!wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_different_package() {
        // FALSE: Different package
        let wit1 = WitInterface::from("wasi:blobstore");
        let wit2 = WitInterface::from("wasi:keyvalue");
        assert!(!wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_interface_subset() {
        // TRUE: wit2's interfaces are a subset of wit1's interfaces
        let wit1 = WitInterface::from("wasi:blobstore/types,container,blobstore");
        let wit2 = WitInterface::from("wasi:blobstore/types,container");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_interface_exact_match() {
        // TRUE: Exact same interfaces
        let wit1 = WitInterface::from("wasi:blobstore/types,container");
        let wit2 = WitInterface::from("wasi:blobstore/types,container");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_interface_not_subset() {
        // FALSE: wit2 has interfaces that wit1 doesn't have
        let wit1 = WitInterface::from("wasi:blobstore/types");
        let wit2 = WitInterface::from("wasi:blobstore/types,container");
        assert!(!wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_empty_interfaces() {
        // TRUE: Both have no interfaces specified
        let wit1 = WitInterface::from("wasi:blobstore");
        let wit2 = WitInterface::from("wasi:blobstore");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_empty_subset_of_non_empty() {
        // TRUE: Empty set is a subset of any set
        let wit1 = WitInterface::from("wasi:blobstore/types,container");
        let wit2 = WitInterface::from("wasi:blobstore");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_version_matching() {
        // TRUE: Same version
        let wit1 = WitInterface::from("wasi:blobstore/types@0.2.0");
        let wit2 = WitInterface::from("wasi:blobstore/types@0.2.0");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_version_mismatch() {
        // FALSE: Different versions
        let wit1 = WitInterface::from("wasi:blobstore/types@0.2.0");
        let wit2 = WitInterface::from("wasi:blobstore/types@0.3.0");
        assert!(!wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_version_missing() {
        // TRUE: wit1 has version, wit2 doesn't, wit2 is less restrictive
        let wit1 = WitInterface::from("wasi:blobstore/types@0.2.0");
        let wit2 = WitInterface::from("wasi:blobstore/types");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_version_not_required() {
        // TRUE: wit1 has no version requirement, wit2 can have any version
        let wit1 = WitInterface::from("wasi:blobstore/types");
        let wit2 = WitInterface::from("wasi:blobstore/types@0.2.0");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_complex_scenario() {
        // TRUE: Same namespace, package, version, and wit2's interfaces are subset
        let wit1 = WitInterface::from("wasi:http/types,incoming-handler,outgoing-handler@0.2.0");
        let wit2 = WitInterface::from("wasi:http/types,incoming-handler@0.2.0");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_single_interface_vs_multiple() {
        // TRUE: Single interface is subset of multiple
        let wit1 = WitInterface::from("wasi:cli/environment,exit,stdin,stdout");
        let wit2 = WitInterface::from("wasi:cli/environment");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_overlapping_but_not_subset() {
        // FALSE: wit2 has 'stderr' which wit1 doesn't have
        let wit1 = WitInterface::from("wasi:cli/stdin,stdout");
        let wit2 = WitInterface::from("wasi:cli/stdout,stderr");
        assert!(!wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_with_config() {
        // Config doesn't affect contains logic, only namespace, package, interfaces, and version matter
        let mut wit1 = WitInterface::from("wasi:blobstore/types");
        wit1.config.insert("key".to_string(), "value1".to_string());

        let mut wit2 = WitInterface::from("wasi:blobstore/types");
        wit2.config.insert("key".to_string(), "value2".to_string());

        // TRUE: Config differences are ignored
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_no_namespace() {
        // TRUE: Both have empty namespace (just package)
        let wit1 = WitInterface::from("blobstore/types");
        let wit2 = WitInterface::from("blobstore/types");
        assert!(wit1.contains(&wit2));
    }

    #[test]
    fn test_contains_mixed_empty_namespace() {
        // FALSE: One has namespace, other doesn't
        let wit1 = WitInterface::from("wasi:blobstore/types");
        let wit2 = WitInterface::from("blobstore/types");
        assert!(!wit1.contains(&wit2));
    }
}
