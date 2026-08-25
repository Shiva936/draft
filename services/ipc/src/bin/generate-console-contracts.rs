use draft_ipc::console_contracts::{typescript_declarations, ConsoleContractsSchema};
use schemars::schema_for;
use serde_json::{json, Value};
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root");
    let mut arguments = std::env::args_os().skip(1);
    let typescript = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("console/web/src/generated-contracts.ts"));
    let schema = arguments
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("proto/schemas/console-http.schema.json"));
    if arguments.next().is_some() {
        panic!("usage: generate-console-contracts [typescript-output schema-output]");
    }
    std::fs::write(&typescript, typescript_declarations()).expect("write TypeScript DTOs");
    let mut contract = serde_json::to_value(schema_for!(ConsoleContractsSchema))
        .expect("build Console HTTP schema");
    let object = contract.as_object_mut().expect("schema root object");
    object.insert(
        "$id".into(),
        Value::String("https://draft.dev/schemas/console-http.schema.json".into()),
    );
    object.insert("title".into(), Value::String("Draft Console HTTP".into()));
    apply_registered_schema_versions(&mut contract);
    let json = serde_json::to_vec_pretty(&contract).expect("serialize Console DTO schema");
    std::fs::write(&schema, json).expect("write Console DTO schema");
}

fn apply_registered_schema_versions(root: &mut Value) {
    use draft_core::contracts::{current_version, ContractId};
    set_version_property(root, current_version(ContractId::ConsoleContractsSchema));
    let definitions_key = if root.get("$defs").is_some() {
        "$defs"
    } else {
        "definitions"
    };
    let definitions = root
        .get_mut(definitions_key)
        .and_then(Value::as_object_mut)
        .expect("Console schema definitions");
    for (name, contract) in [
        ("ConsoleSessionDto", ContractId::ConsoleSession),
        ("WorkspaceRevisionDto", ContractId::ConsoleWorkspaceRevision),
        ("RegistryProjectDto", ContractId::ConsoleRegistryProject),
        ("TaskDefinitionDto", ContractId::ConsoleTaskDefinition),
        (
            "ExtensionCatalogSourceDto",
            ContractId::ConsoleCatalogSource,
        ),
        ("ServiceJobDto", ContractId::ConsoleServiceJob),
        ("ApiFailureDto", ContractId::ConsoleApiFailure),
    ] {
        let definition = definitions
            .get_mut(name)
            .unwrap_or_else(|| panic!("registered Console schema definition {name}"));
        set_version_property(definition, current_version(contract));
    }
}

fn set_version_property(value: &mut Value, version: u32) {
    let properties = value
        .get_mut("properties")
        .and_then(Value::as_object_mut)
        .expect("registered contract properties");
    assert!(
        properties.contains_key("schema_version"),
        "registered contract must declare schema_version"
    );
    properties.insert(
        "schema_version".into(),
        json!({ "type": "integer", "const": version }),
    );
}
