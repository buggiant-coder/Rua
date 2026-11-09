use clap::ValueEnum;
use serde::Serialize;

#[derive(Debug, Clone, Copy, ValueEnum, Default)]
pub enum OutputFormat {
    #[default]
    Human,
    Json,
}

pub fn print_json<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("Failed to serialize output: {e}"),
    }
}

pub fn print_value(value: &serde_json::Value, format: OutputFormat) {
    match format {
        OutputFormat::Json => print_json(value),
        OutputFormat::Human => print_human(value),
    }
}

fn print_human(value: &serde_json::Value) {
    match value {
        serde_json::Value::Array(arr) => {
            if arr.is_empty() {
                println!("(no results)");
                return;
            }
            for (i, item) in arr.iter().enumerate() {
                if i > 0 {
                    println!();
                }
                print_human_object(item);
            }
        }
        serde_json::Value::Object(_) => print_human_object(value),
        other => println!("{other}"),
    }
}

fn print_human_object(value: &serde_json::Value) {
    if let Some(obj) = value.as_object() {
        for (key, val) in obj {
            match val {
                serde_json::Value::Null => {}
                serde_json::Value::String(s) => println!("  {key}: {s}"),
                serde_json::Value::Bool(b) => println!("  {key}: {b}"),
                serde_json::Value::Number(n) => println!("  {key}: {n}"),
                serde_json::Value::Array(arr) => {
                    if arr.is_empty() {
                        println!("  {key}: []");
                    } else {
                        println!("  {key}:");
                        for item in arr {
                            match item {
                                serde_json::Value::String(s) => println!("    - {s}"),
                                other => println!("    - {other}"),
                            }
                        }
                    }
                }
                serde_json::Value::Object(_) => {
                    println!("  {key}: {val}");
                }
            }
        }
    } else {
        println!("{value}");
    }
}
