use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rua_json_repair::repair_json;

fn bench_valid_json_fast_path(c: &mut Criterion) {
    let mut obj = serde_json::Map::new();
    for i in 0..50 {
        obj.insert(
            format!("key_{}", i),
            serde_json::Value::String(format!("value_{}_with_some_padding_text_here", i)),
        );
    }
    let input = serde_json::to_string(&serde_json::Value::Object(obj)).unwrap();
    assert!(
        input.len() > 3000 && input.len() < 5000,
        "input size: {}",
        input.len()
    );

    c.bench_function("valid_4kb_fast_path", |b| {
        b.iter(|| {
            let result = repair_json(black_box(&input)).unwrap();
            assert!(!result.changed);
        })
    });
}

fn bench_malformed_4kb(c: &mut Criterion) {
    let mut parts = Vec::new();
    parts.push("{".to_string());
    for i in 0..50 {
        if i > 0 {
            parts.push(",".to_string());
        }

        if i % 3 == 0 {
            parts.push(format!("'key_{}': 'value_{}_padding_text_here'", i, i));
        } else if i % 3 == 1 {
            parts.push(format!("key_{}: \"value_{}_padding_text_here\"", i, i));
        } else {
            parts.push(format!("\"key_{}\": \"value_{}_padding_text_here\",", i, i));
        }
    }
    parts.push(",}".to_string());
    let input = parts.join("");

    c.bench_function("malformed_4kb", |b| {
        b.iter(|| {
            let result = repair_json(black_box(&input)).unwrap();
            assert!(result.changed);
        })
    });
}

fn bench_tool_args_1kb(c: &mut Criterion) {
    let inner = r#"{"command": "find /home/user -name '*.py' -type f | head -20", "working_directory": "/home/user/project", "timeout": 30000, "environment": {"PATH": "/usr/bin:/usr/local/bin", "HOME": "/home/user"}, "description": "Search for Python files in the user's home directory and list the first 20 results found"}"#;
    let input = format!("```json\n{}\n```", inner);
    assert!(
        input.len() > 500 && input.len() < 1500,
        "input size: {}",
        input.len()
    );

    c.bench_function("tool_args_1kb", |b| {
        b.iter(|| {
            let result = repair_json(black_box(&input)).unwrap();
            assert!(result.changed);
        })
    });
}

fn bench_max_input(c: &mut Criterion) {
    let mut obj = serde_json::Map::new();
    for i in 0..3000 {
        obj.insert(
            format!("key_{}", i),
            serde_json::Value::String("x".repeat(80)),
        );
    }
    let input = serde_json::to_string(&serde_json::Value::Object(obj)).unwrap();

    c.bench_function("max_256kb_valid", |b| {
        b.iter(|| {
            let result = repair_json(black_box(&input)).unwrap();
            assert!(!result.changed);
        })
    });
}

fn bench_malicious_nested(c: &mut Criterion) {
    let open: String = std::iter::repeat_n('[', 60).collect();
    let close: String = std::iter::repeat_n(']', 60).collect();
    let input = format!("{}1{}", open, close);

    c.bench_function("nested_60_levels", |b| {
        b.iter(|| {
            let result = repair_json(black_box(&input)).unwrap();
            assert!(!result.changed);
        })
    });
}

criterion_group!(
    benches,
    bench_valid_json_fast_path,
    bench_malformed_4kb,
    bench_tool_args_1kb,
    bench_max_input,
    bench_malicious_nested,
);
criterion_main!(benches);
