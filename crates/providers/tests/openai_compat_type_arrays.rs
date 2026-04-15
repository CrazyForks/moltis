/// Tests for type array collapsing in strict mode patching.
///
/// When tool schemas have `type: ["object", "string", ...]`, the strict mode
/// patcher must collapse these to `type: "object"` because:
/// - OpenAI strict mode requires `type` to be a single string
/// - Gemini cannot represent type arrays at all
/// - The "object" form is the intended shape for tool calling
///
/// See: https://github.com/moltis-org/moltis/issues/716
use moltis_providers::openai_compat::{patch_schema_for_strict_mode, to_openai_tools};

#[test]
fn strict_mode_collapses_object_union_types() {
    // type: ["object", "string"] should collapse to "object"
    let mut schema = serde_json::json!({
        "type": ["object", "string"],
        "properties": {
            "kind": { "type": "string" },
            "value": { "type": "integer" }
        },
        "required": ["kind"]
    });

    patch_schema_for_strict_mode(&mut schema);

    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
    let required = schema["required"].as_array().unwrap();
    assert_eq!(required.len(), 2);
}

#[test]
fn strict_mode_collapses_triple_union_types() {
    // type: ["object", "string", "integer"] should collapse to "object"
    let mut schema = serde_json::json!({
        "type": ["object", "string", "integer"],
        "properties": {
            "kind": { "type": "string" },
            "every_ms": { "type": "integer" }
        },
        "required": ["kind"]
    });

    patch_schema_for_strict_mode(&mut schema);

    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
}

#[test]
fn strict_mode_leaves_singular_string_type_unchanged() {
    let mut schema = serde_json::json!({
        "type": "string"
    });

    patch_schema_for_strict_mode(&mut schema);

    assert_eq!(schema["type"], "string");
    assert!(schema.get("additionalProperties").is_none());
}

#[test]
fn strict_mode_leaves_non_object_array_unchanged() {
    // type: ["string", "integer"] — no "object", no collapse
    let mut schema = serde_json::json!({
        "type": ["string", "integer"]
    });

    patch_schema_for_strict_mode(&mut schema);

    assert_eq!(schema["type"], serde_json::json!(["string", "integer"]));
    assert!(schema.get("additionalProperties").is_none());
}

#[test]
fn strict_mode_collapses_nested_object_union_types() {
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": {
            "schedule": {
                "type": ["object", "string", "integer"],
                "properties": {
                    "kind": { "type": "string" },
                    "delay_ms": { "type": "integer" }
                },
                "required": ["kind"]
            },
            "payload": {
                "type": ["object", "string"],
                "properties": {
                    "kind": { "type": "string" },
                    "message": { "type": "string" }
                },
                "required": ["kind"]
            }
        },
        "required": ["schedule", "payload"]
    });

    patch_schema_for_strict_mode(&mut schema);

    // Top level: singular "object" — unchanged type but patched
    assert_eq!(schema["type"], "object");

    // schedule: collapsed from array to "object"
    assert_eq!(schema["properties"]["schedule"]["type"], "object");
    assert_eq!(
        schema["properties"]["schedule"]["additionalProperties"],
        false
    );

    // payload: collapsed from array to "object"
    assert_eq!(schema["properties"]["payload"]["type"], "object");
    assert_eq!(
        schema["properties"]["payload"]["additionalProperties"],
        false
    );

    // All required entries should match actual properties
    let top_required: Vec<&str> = schema["required"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for name in &top_required {
        assert!(
            schema["properties"]
                .as_object()
                .unwrap()
                .contains_key(*name),
            "top-level required '{name}' missing from properties"
        );
    }
}

#[test]
fn to_openai_tools_collapses_union_types_end_to_end() {
    let tools = vec![serde_json::json!({
        "name": "cron",
        "description": "Manage cron jobs",
        "parameters": {
            "type": "object",
            "properties": {
                "action": { "type": "string" },
                "job": {
                    "type": ["object", "string"],
                    "properties": {
                        "name": { "type": "string" },
                        "schedule": {
                            "type": ["object", "string", "integer"],
                            "properties": {
                                "kind": { "type": "string" }
                            },
                            "required": ["kind"]
                        }
                    },
                    "required": ["name", "schedule"]
                }
            },
            "required": ["action", "job"]
        }
    })];

    let converted = to_openai_tools(&tools);
    assert_eq!(converted.len(), 1);

    let params = &converted[0]["function"]["parameters"];

    // No type arrays should remain
    let mut type_arrays = Vec::new();
    find_type_arrays(params, "root", &mut type_arrays);
    assert!(
        type_arrays.is_empty(),
        "found type arrays after pipeline: {type_arrays:?}"
    );

    // All required entries should exist in properties
    let mut orphans = Vec::new();
    find_required_orphans(params, "root", &mut orphans);
    assert!(orphans.is_empty(), "found required orphans: {orphans:?}");
}

// --- Additional coverage for recursion paths and edge cases ---

#[test]
fn collapse_inside_anyOf_variants() {
    // Type arrays inside anyOf variants should be collapsed recursively.
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": {
            "config": {
                "anyOf": [
                    { "type": ["object", "string"], "properties": { "key": { "type": "string" } }, "required": ["key"] },
                    { "type": "string" }
                ]
            }
        },
        "required": ["config"]
    });

    patch_schema_for_strict_mode(&mut schema);

    let any_of = schema["properties"]["config"]["anyOf"].as_array().unwrap();
    // First variant: collapsed from ["object", "string"] to "object"
    assert_eq!(any_of[0]["type"], "object");
    assert_eq!(any_of[0]["additionalProperties"], false);
    // Second variant: plain string, untouched
    assert_eq!(any_of[1]["type"], "string");
}

#[test]
fn collapse_inside_oneOf_variants() {
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": {
            "schedule": {
                "oneOf": [
                    { "type": ["object", "integer"], "properties": { "kind": { "type": "string" } }, "required": [] },
                    { "type": ["object", "string"], "properties": { "mode": { "type": "string" } }, "required": [] }
                ]
            }
        },
        "required": ["schedule"]
    });

    patch_schema_for_strict_mode(&mut schema);

    let one_of = schema["properties"]["schedule"]["oneOf"]
        .as_array()
        .unwrap();
    assert_eq!(one_of[0]["type"], "object");
    assert_eq!(one_of[1]["type"], "object");
}

#[test]
fn collapse_inside_array_items() {
    // Type array inside array items should be collapsed.
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": {
            "rules": {
                "type": "array",
                "items": {
                    "type": ["object", "string"],
                    "properties": { "pattern": { "type": "string" } },
                    "required": ["pattern"]
                }
            }
        },
        "required": ["rules"]
    });

    patch_schema_for_strict_mode(&mut schema);

    let items = &schema["properties"]["rules"]["items"];
    assert_eq!(items["type"], "object");
    assert_eq!(items["additionalProperties"], false);
}

#[test]
fn collapsed_optional_object_becomes_nullable() {
    // A union-type object that is NOT in the original required array
    // should become ["object", "null"] after strict mode patching.
    let mut schema = serde_json::json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "config": {
                "type": ["object", "string"],
                "properties": { "key": { "type": "string" } },
                "required": ["key"]
            }
        },
        "required": ["name"]
    });

    patch_schema_for_strict_mode(&mut schema);

    // "config" was optional, so it should now be nullable
    let config_type = &schema["properties"]["config"]["type"];
    let arr = config_type
        .as_array()
        .expect("optional union-type object should have array type after nullable conversion");
    assert!(
        arr.contains(&serde_json::json!("object")),
        "type should contain 'object'"
    );
    assert!(
        arr.contains(&serde_json::json!("null")),
        "type should contain 'null' for optional property"
    );
}

#[test]
fn object_not_first_in_array_still_collapses() {
    // "object" can appear anywhere in the array, not just first.
    let mut schema = serde_json::json!({
        "type": ["string", "integer", "object"],
        "properties": { "id": { "type": "string" } },
        "required": []
    });

    patch_schema_for_strict_mode(&mut schema);

    assert_eq!(schema["type"], "object");
    assert_eq!(schema["additionalProperties"], false);
}

#[test]
fn empty_type_array_is_not_object() {
    // type: [] — no "object" present, should not be treated as object.
    let mut schema = serde_json::json!({
        "type": []
    });

    patch_schema_for_strict_mode(&mut schema);

    assert_eq!(schema["type"], serde_json::json!([]));
    assert!(schema.get("additionalProperties").is_none());
}

// --- Test helpers ---

fn find_type_arrays(schema: &serde_json::Value, path: &str, results: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else {
        return;
    };
    if let Some(arr) = obj.get("type").and_then(|t| t.as_array()) {
        results.push(format!("{path}: type={arr:?}"));
    }
    if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
        for (key, val) in props {
            find_type_arrays(val, &format!("{path}.{key}"), results);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = obj.get(key).and_then(|v| v.as_array()) {
            for (i, variant) in variants.iter().enumerate() {
                find_type_arrays(variant, &format!("{path}.{key}[{i}]"), results);
            }
        }
    }
    if let Some(items) = obj.get("items") {
        find_type_arrays(items, &format!("{path}.items"), results);
    }
}

fn find_required_orphans(schema: &serde_json::Value, path: &str, results: &mut Vec<String>) {
    let Some(obj) = schema.as_object() else {
        return;
    };
    if let (Some(required), Some(properties)) = (
        obj.get("required").and_then(|r| r.as_array()),
        obj.get("properties").and_then(|p| p.as_object()),
    ) {
        for entry in required {
            if let Some(name) = entry.as_str() {
                if !properties.contains_key(name) {
                    results.push(format!("{path}: required '{name}' not in properties"));
                }
            }
        }
    }
    if let Some(props) = obj.get("properties").and_then(|p| p.as_object()) {
        for (key, val) in props {
            find_required_orphans(val, &format!("{path}.{key}"), results);
        }
    }
    for key in ["anyOf", "oneOf", "allOf"] {
        if let Some(variants) = obj.get(key).and_then(|v| v.as_array()) {
            for (i, variant) in variants.iter().enumerate() {
                find_required_orphans(variant, &format!("{path}.{key}[{i}]"), results);
            }
        }
    }
    if let Some(items) = obj.get("items") {
        find_required_orphans(items, &format!("{path}.items"), results);
    }
}
}
