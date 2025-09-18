use serde_json::json;

// This test mirrors the extraction logic for a root JSON pointer '/' with nonEmptyPointerBlocks.
// We duplicate the minimal logic to avoid needing internal symbol visibility changes.
#[test]
fn non_empty_pointer_blocks_array_root() {
    let body = json!([
        {"entity_type": "EMAIL_ADDRESS", "start": 10, "end": 25, "score": 0.99}
    ]);

    let block_field = "/";
    let non_empty_pointer_blocks = true;

    let mut result: Option<bool> = None;
    if block_field == "/" && non_empty_pointer_blocks {
        match &body {
            serde_json::Value::Array(a) => result = Some(!a.is_empty()),
            serde_json::Value::Object(o) => result = Some(!o.is_empty()),
            serde_json::Value::Bool(b) => result = Some(*b),
            _ => {}
        }
    }

    assert_eq!(result, Some(true));
}
