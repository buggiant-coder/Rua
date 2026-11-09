use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MetadataFilter {
    Eq {
        field: String,
        value: serde_json::Value,
    },

    Ne {
        field: String,
        value: serde_json::Value,
    },

    Contains {
        field: String,
        value: String,
    },

    Exists {
        field: String,
    },

    And(Vec<MetadataFilter>),

    Or(Vec<MetadataFilter>),
}

impl MetadataFilter {
    pub fn to_sql(&self) -> (String, Vec<serde_json::Value>) {
        match self {
            MetadataFilter::Eq { field, value } => {
                let sql = format!(
                    "json_extract(metadata, '$.{}') = json_extract(?{{}}, '$')",
                    escape_json_path(field)
                );
                (sql, vec![value.clone()])
            }
            MetadataFilter::Ne { field, value } => {
                let sql = format!(
                    "json_extract(metadata, '$.{}') != json_extract(?{{}}, '$')",
                    escape_json_path(field)
                );
                (sql, vec![value.clone()])
            }
            MetadataFilter::Contains { field, value } => {
                let sql = format!(
                    "EXISTS (SELECT 1 FROM json_each(json_extract(metadata, '$.{}')) WHERE value = json_extract(?{{}}, '$'))",
                    escape_json_path(field)
                );
                (sql, vec![serde_json::Value::String(value.clone())])
            }
            MetadataFilter::Exists { field } => {
                let sql = format!(
                    "json_extract(metadata, '$.{}') IS NOT NULL",
                    escape_json_path(field)
                );
                (sql, vec![])
            }
            MetadataFilter::And(filters) => {
                let mut parts = Vec::new();
                let mut params = Vec::new();
                for f in filters {
                    let (sql, p) = f.to_sql();
                    parts.push(format!("({sql})"));
                    params.extend(p);
                }
                (parts.join(" AND "), params)
            }
            MetadataFilter::Or(filters) => {
                let mut parts = Vec::new();
                let mut params = Vec::new();
                for f in filters {
                    let (sql, p) = f.to_sql();
                    parts.push(format!("({sql})"));
                    params.extend(p);
                }
                (parts.join(" OR "), params)
            }
        }
    }

    pub fn eq(field: impl Into<String>, value: serde_json::Value) -> Self {
        MetadataFilter::Eq {
            field: field.into(),
            value,
        }
    }

    pub fn and(filters: Vec<MetadataFilter>) -> Self {
        MetadataFilter::And(filters)
    }
}

pub fn escape_json_path(field: &str) -> String {
    field
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
        .collect()
}
