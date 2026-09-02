//! TC.4/TC.5 — the one-per-plugin adapter between the SDK's shape types and
//! org's generated WIT bindings.
//!
//! Design: `lattice/docs/dev/architecture/typed-configuration.md`.
//!
//! `lattice-plugin-sdk` is deliberately WIT-agnostic: a proc-macro crate cannot
//! name a per-world WIT type, which is why `#[derive(PluginOption)]` hands back
//! an `OptionKind` the plugin maps at the `register-option` call site. The same
//! holds for `#[derive(ConfigShape)]`, so this module is org's mapping — written
//! once, not per option, exactly like the `wit_ty` one-liner beside it.
//!
//! What it also hides is the ARENA. WIT has no recursive types, so a schema and
//! a value cross as a flat node list plus a root index. The SDK flattens; this
//! renames the nodes. No plugin should ever write one of those by hand.

use lattice_plugin_sdk::shape::{
    self, flatten_schema, flatten_value, unflatten_value, ConfigShape, ScalarKind, ShapeError,
    Value,
};

use crate::lattice::plugin_host::config;

fn scalar(kind: ScalarKind) -> config::OptionType {
    match kind {
        ScalarKind::Bool => config::OptionType::Boolean,
        ScalarKind::Int => config::OptionType::Integer,
        ScalarKind::Str => config::OptionType::String,
    }
}

/// A type's declared shape, in the form `register-structured-option` takes.
pub fn schema_of<T: ConfigShape>() -> config::ConfigSchema {
    let (nodes, root) = flatten_schema(&T::schema());
    config::ConfigSchema {
        nodes: nodes.iter().map(node_to_wit).collect(),
        root,
    }
}

fn node_to_wit(node: &shape::SchemaNode) -> config::SchemaNode {
    match node {
        shape::SchemaNode::Scalar(k) => config::SchemaNode::Scalar(scalar(*k)),
        shape::SchemaNode::Enum(forms) => config::SchemaNode::EnumOf(forms.clone()),
        shape::SchemaNode::List(child) => config::SchemaNode::ListOf(*child),
        shape::SchemaNode::Record(fields) => config::SchemaNode::Record(
            fields
                .iter()
                .map(|f| config::SchemaField {
                    name: f.name.clone(),
                    schema: f.schema,
                    required: f.required,
                    doc: f.doc.clone(),
                })
                .collect(),
        ),
    }
}

/// A value, in the form `set-option-value` takes.
pub fn value_to_wit(value: &Value) -> config::ConfigValue {
    let (nodes, root) = flatten_value(value);
    config::ConfigValue {
        nodes: nodes.iter().map(value_node_to_wit).collect(),
        root,
    }
}

fn value_node_to_wit(node: &shape::ValueNode) -> config::ValueNode {
    match node {
        shape::ValueNode::Bool(b) => config::ValueNode::Bool(*b),
        shape::ValueNode::Int(i) => config::ValueNode::Int(*i),
        shape::ValueNode::Str(s) => config::ValueNode::String(s.clone()),
        shape::ValueNode::List(children) => config::ValueNode::List(children.clone()),
        shape::ValueNode::Record(fields) => config::ValueNode::Record(fields.clone()),
    }
}

fn value_node_from_wit(node: &config::ValueNode) -> shape::ValueNode {
    match node {
        config::ValueNode::Bool(b) => shape::ValueNode::Bool(*b),
        config::ValueNode::Int(i) => shape::ValueNode::Int(*i),
        config::ValueNode::String(s) => shape::ValueNode::Str(s.clone()),
        config::ValueNode::List(children) => shape::ValueNode::List(children.clone()),
        config::ValueNode::Record(fields) => shape::ValueNode::Record(fields.clone()),
    }
}

/// Read an option's current value and rebuild `T` from it.
///
/// `None` when the option does not exist. `Err` when it exists and does not fit
/// `T` — which the host has usually already prevented, since it validates every
/// write against the declared schema; what survives to here is the checking a
/// schema cannot express (an enum spelled as a string, a path, a bound).
pub fn read_option<T: ConfigShape>(name: &str) -> Option<Result<T, ShapeError>> {
    let raw = config::get_option_value(name)?;
    let nodes: Vec<shape::ValueNode> = raw.nodes.iter().map(value_node_from_wit).collect();
    Some(unflatten_value(&nodes, raw.root).and_then(|v| T::from_value(&v)))
}

/// Declare `name` with `T`'s shape and `default`'s value.
///
/// Returns `false` exactly when the host refused, which it does for a name
/// collision or a default that does not fit its own schema — never a trap.
pub fn register_option<T: ConfigShape>(name: &str, default: &T, doc: &str) -> bool {
    config::register_structured_option(
        name,
        &schema_of::<T>(),
        &value_to_wit(&default.to_value()),
        doc,
    )
}

/// Test-only: a TOML fixture as a declared value.
///
/// The production path never parses TOML — that is TC.6's whole point, and why
/// `toml` is a dev-dependency now rather than something shipped inside the
/// component. But TOML is still the nicest way to WRITE a fixture, and the
/// alternative is pages of nested `Value::record([...])` in which a test's
/// intent disappears. So the fixtures stay as they were and this turns them
/// into the tree the code under test actually receives.
///
/// `wrapper` is the array-of-tables key the fixture nests under (`section`,
/// `command`, `template`), because a TOML document cannot be an array and the
/// declared shape is a list.
#[cfg(test)]
pub(crate) fn from_toml<T: ConfigShape>(src: &str, wrapper: &str) -> T {
    fn convert(v: &toml::Value) -> Value {
        match v {
            toml::Value::String(s) => Value::Str(s.clone()),
            toml::Value::Integer(i) => Value::Int(*i),
            toml::Value::Boolean(b) => Value::Bool(*b),
            toml::Value::Array(items) => Value::List(items.iter().map(convert).collect()),
            toml::Value::Table(t) => Value::record(t.iter().map(|(k, v)| (k.clone(), convert(v)))),
            other => panic!("fixture has no config-value shape: {other:?}"),
        }
    }
    let doc: toml::Table = toml::from_str(src).expect("the fixture is valid TOML");
    let list = doc
        .get(wrapper)
        .cloned()
        .unwrap_or(toml::Value::Array(Vec::new()));
    T::from_value(&convert(&list)).expect("the fixture fits the declared shape")
}
