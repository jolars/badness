//! JSON Schema for `badness.toml`.
//!
//! Generates the schema from the host [`Config`] type, keeps the checked-in
//! `badness.schema.json` artifact synchronized, and checks representative
//! configuration shapes with the same validator editors consume.
//!
//! Run `UPDATE_EXPECTED=1 cargo test --test config_schema` after an intentional
//! schema change, and review the generated diff before committing it.

use std::fs;
use std::path::Path;

use badness::config::Config;
use jsonschema::Validator;
use schemars::generate::SchemaSettings;
use schemars::transform::RestrictFormats;
use serde_json::Value;

const SCHEMA_ID: &str = "https://badness.dev/badness.schema.json";
const SCHEMA_PATH: &str = "badness.schema.json";

const COMPLETE_CONFIG: &str = r#"
exclude = ["vendor/"]
extend-exclude = ["build/"]

[format]
line-width = 100
indent-width = 4
item-indent = "indent"
wrap = "stable"
math-wrap = "single-line"
line-ending = "crlf"
lang = "de"

[format.no-break-abbreviations]
default = ["ibid."]
de = ["bzw.", "Abb."]

[lint]
select = ["duplicate-label"]
ignore = ["deprecated-command"]

[build]
aux-dir = "out"
pdf-dir = "out"
pdf-filename = "thesis.pdf"
root = "main.tex"

[commands.eqrefs]
like = "cref"

[environments.myenv]
like = "align"

[environments.eqnarray]
begin = ['\bea']
end = ['\eea']
"#;

fn generate_schema_json() -> Value {
    // SchemaStore validates schemas in strict mode. Schemars otherwise emits
    // Rust-specific formats such as `uint32`, which add no constraint beyond
    // the generated integer bounds and are not part of draft 7.
    let generator = SchemaSettings::draft07()
        .with_transform(RestrictFormats::default())
        .into_generator();
    let schema = generator.into_root_schema_for::<Config>();
    let mut json = serde_json::to_value(schema).expect("schema to JSON");
    if let Value::Object(map) = &mut json {
        map.insert("$id".to_string(), Value::String(SCHEMA_ID.to_string()));
        map.insert(
            "title".to_string(),
            Value::String("Badness configuration".to_string()),
        );
        map.insert(
            "description".to_string(),
            Value::String(
                "Schema for badness.toml. Generated from the host Config types; \
                 do not hand-edit—run `UPDATE_EXPECTED=1 cargo test --test \
                 config_schema` instead."
                    .to_string(),
            ),
        );
    }
    json
}

fn schema_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(SCHEMA_PATH)
}

fn write_pretty(json: &Value) -> String {
    let mut output = serde_json::to_string_pretty(json).expect("serialize schema");
    output.push('\n');
    output
}

fn toml_to_json(source: &str) -> Value {
    let value: toml::Value = toml::from_str(source).expect("parse fixture TOML");
    serde_json::to_value(value).expect("TOML to JSON")
}

fn validator() -> Validator {
    Validator::new(&generate_schema_json()).expect("compile generated schema")
}

fn validation_errors(source: &str) -> Vec<String> {
    validator()
        .iter_errors(&toml_to_json(source))
        .map(|error| format!("{error} at {}", error.instance_path()))
        .collect()
}

#[test]
fn schema_is_in_sync_with_config_types() {
    let generated = write_pretty(&generate_schema_json());
    let path = schema_path();

    if std::env::var_os("UPDATE_EXPECTED").is_some() {
        fs::write(&path, &generated).expect("write schema");
        return;
    }

    let on_disk = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "missing {}: {error}. Run `UPDATE_EXPECTED=1 cargo test --test \
             config_schema` to create it.",
            path.display()
        )
    });

    similar_asserts::assert_eq!(
        on_disk,
        generated,
        "{} is out of date with the host Config types. Run `UPDATE_EXPECTED=1 \
         cargo test --test config_schema` to regenerate it.",
        path.display()
    );
}

#[test]
fn schema_uses_the_public_draft_7_identity() {
    let schema = generate_schema_json();
    assert_eq!(schema["$id"], SCHEMA_ID);
    assert_eq!(schema["$schema"], "http://json-schema.org/draft-07/schema#");
    assert!(
        !write_pretty(&schema).contains("\"format\": \"uint32\""),
        "schema must not expose Rust-specific formats"
    );
    validator();
}

#[test]
fn schema_accepts_a_complete_runtime_valid_config() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("badness.toml");
    fs::write(&path, COMPLETE_CONFIG).expect("write config");
    Config::load_from(&path).expect("runtime accepts config");

    let errors = validation_errors(COMPLETE_CONFIG);
    assert!(
        errors.is_empty(),
        "schema rejected a runtime-valid config:\n{}",
        errors.join("\n")
    );
}

#[test]
fn schema_rejects_unknown_keys() {
    for source in [
        "formatt = true",
        "[format]\nline-widht = 80",
        "[commands.eqrefs]\nliek = 'cref'",
        "[environments.myenv]\nliek = 'align'",
    ] {
        assert!(
            !validation_errors(source).is_empty(),
            "schema accepted unknown key in:\n{source}"
        );
    }
}

#[test]
fn schema_rejects_invalid_enums_and_types() {
    for source in [
        "[format]\nitem-indent = 'same'",
        "[format]\nwrap = 'smart'",
        "[format]\nmath-wrap = 'never'",
        "[format]\nline-ending = 'cr'",
        "[lint]\nselect = 'duplicate-label'",
        "[environments.eqnarray]\nbegin = '\\bea'",
    ] {
        assert!(
            !validation_errors(source).is_empty(),
            "schema accepted invalid value in:\n{source}"
        );
    }
}

#[test]
fn schema_rejects_widths_outside_the_runtime_range() {
    for source in [
        "[format]\nline-width = 0",
        "[format]\nline-width = 1001",
        "[format]\nindent-width = 0",
        "[format]\nindent-width = 1001",
    ] {
        assert!(
            !validation_errors(source).is_empty(),
            "schema accepted invalid width in:\n{source}"
        );
    }
}
