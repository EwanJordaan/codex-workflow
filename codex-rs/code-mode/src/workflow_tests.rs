use pretty_assertions::assert_eq;
use serde_json::json;

use super::compile_workflow_source;

const SIMPLE_WORKFLOW: &str = r#"
export const meta = {
  name: "simple",
  description: "Return structured input",
};

return { received: args };
"#;

#[test]
fn compile_injects_validated_metadata_args_and_isolated_body() {
    let compiled = compile_workflow_source(SIMPLE_WORKFLOW, Some(&json!({ "value": 7 })))
        .expect("workflow should compile");

    assert!(compiled.contains(r#"const args = {"value":7};"#));
    assert!(compiled.contains(
        r#"const meta = Object.freeze({"description":"Return structured input","name":"simple"});"#
    ));
    assert!(compiled.contains("const __workflowMain = new __AsyncFunction("));
    assert!(compiled.contains("delete globalThis[name]"));
}

#[test]
fn compile_accepts_single_quoted_metadata() {
    let compiled = compile_workflow_source(
        "export const meta = { name: 'single', description: 'Single quoted' };\nreturn 1;",
        None,
    )
    .expect("single-quoted metadata should compile");

    assert!(compiled.contains("const args = undefined;"));
    assert!(compiled.contains(r#""name":"single""#));
}

#[test]
fn compile_rejects_missing_or_empty_metadata() {
    assert_eq!(
        compile_workflow_source("return 1", None),
        Err("workflow must start with `export const meta = { name, description }`".to_string())
    );
    assert_eq!(
        compile_workflow_source(
            "export const meta = { name: '', description: 'No name' }; return 1;",
            None,
        ),
        Err("workflow metadata name and description must be non-empty".to_string())
    );
}
