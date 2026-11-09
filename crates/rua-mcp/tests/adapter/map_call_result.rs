use mcp_manager::types::{McpCallResult, McpContent};
use rua_mcp::tool_adapter::McpToolAdapter;

#[test]
fn text_content_maps_to_text_part() {
    let mcp_result = McpCallResult {
        structured_content: None,
        content: vec![McpContent::Text {
            text: "hello world".into(),
        }],
        is_error: false,
    };
    let result = McpToolAdapter::map_call_result(&mcp_result);
    assert!(!result.is_error);
    assert_eq!(result.content.len(), 1);
    assert_eq!(
        result.content[0],
        agent_types::ContentPart::Text {
            text: "hello world".into(),
        }
    );
}

#[test]
fn image_content_maps_to_image_part_with_data_uri() {
    let mcp_result = McpCallResult {
        structured_content: None,
        content: vec![McpContent::Image {
            data: "iVBORw0KGgo=".into(),
            mime_type: "image/png".into(),
        }],
        is_error: false,
    };
    let result = McpToolAdapter::map_call_result(&mcp_result);
    assert_eq!(result.content.len(), 1);
    match &result.content[0] {
        agent_types::ContentPart::Image {
            url,
            media_type,
            alt,
        } => {
            assert_eq!(url, "data:image/png;base64,iVBORw0KGgo=");
            assert_eq!(media_type.as_deref(), Some("image/png"));
            assert!(alt.is_none());
        }
        other => panic!("expected Image, got {other:?}"),
    }
}

#[test]
fn resource_with_text_maps_to_text_part() {
    let mcp_result = McpCallResult {
        structured_content: None,
        content: vec![McpContent::Resource {
            uri: "file:///tmp/data.txt".into(),
            text: Some("resource content".into()),
        }],
        is_error: false,
    };
    let result = McpToolAdapter::map_call_result(&mcp_result);
    assert_eq!(result.content.len(), 1);
    assert_eq!(
        result.content[0],
        agent_types::ContentPart::Text {
            text: "resource content".into(),
        }
    );
}

#[test]
fn resource_without_text_maps_to_file_part() {
    let mcp_result = McpCallResult {
        structured_content: None,
        content: vec![McpContent::Resource {
            uri: "file:///tmp/binary.dat".into(),
            text: None,
        }],
        is_error: false,
    };
    let result = McpToolAdapter::map_call_result(&mcp_result);
    assert_eq!(result.content.len(), 1);
    match &result.content[0] {
        agent_types::ContentPart::File { url, name, .. } => {
            assert_eq!(url, "file:///tmp/binary.dat");
            assert_eq!(name, "file:///tmp/binary.dat");
        }
        other => panic!("expected File, got {other:?}"),
    }
}

#[test]
fn mixed_content_preserves_all_parts() {
    let mcp_result = McpCallResult {
        structured_content: None,
        content: vec![
            McpContent::Text {
                text: "before".into(),
            },
            McpContent::Image {
                data: "AAAA".into(),
                mime_type: "image/jpeg".into(),
            },
            McpContent::Text {
                text: "after".into(),
            },
        ],
        is_error: false,
    };
    let result = McpToolAdapter::map_call_result(&mcp_result);
    assert_eq!(result.content.len(), 3);
    assert!(
        matches!(&result.content[0], agent_types::ContentPart::Text { text } if text == "before")
    );
    assert!(matches!(
        &result.content[1],
        agent_types::ContentPart::Image { .. }
    ));
    assert!(
        matches!(&result.content[2], agent_types::ContentPart::Text { text } if text == "after")
    );
}

#[test]
fn error_result_sets_is_error() {
    let mcp_result = McpCallResult {
        structured_content: None,
        content: vec![McpContent::Text {
            text: "something failed".into(),
        }],
        is_error: true,
    };
    let result = McpToolAdapter::map_call_result(&mcp_result);
    assert!(result.is_error);
}

#[test]
fn empty_content_produces_single_empty_text() {
    let mcp_result = McpCallResult {
        structured_content: None,
        content: vec![],
        is_error: false,
    };
    let result = McpToolAdapter::map_call_result(&mcp_result);
    assert_eq!(result.content.len(), 1);
    assert_eq!(
        result.content[0],
        agent_types::ContentPart::Text {
            text: String::new(),
        }
    );
}

#[test]
fn no_inline_truncation_for_large_text() {
    let big_text = "x".repeat(100_000);
    let mcp_result = McpCallResult {
        structured_content: None,
        content: vec![McpContent::Text {
            text: big_text.clone(),
        }],
        is_error: false,
    };
    let result = McpToolAdapter::map_call_result(&mcp_result);
    assert_eq!(result.content.len(), 1);
    match &result.content[0] {
        agent_types::ContentPart::Text { text } => {
            assert_eq!(text.len(), 100_000, "text must not be truncated inline");
        }
        other => panic!("expected Text, got {other:?}"),
    }
    assert!(result.truncation.is_none());
}
