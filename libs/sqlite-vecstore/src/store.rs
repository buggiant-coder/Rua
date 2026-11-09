use std::path::Path;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::error::{Result, VecStoreError};
use crate::filter::MetadataFilter;
use crate::schema;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: String,

    pub content: String,

    #[serde(default)]
    pub metadata: serde_json::Value,

    #[serde(skip)]
    pub embedding: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub document: Document,

    pub score: f64,
}

#[derive(Debug, Clone, Default)]
pub struct VecStoreConfig {
    pub dimension: Option<usize>,
}

pub struct VecStore {
    conn: Connection,
    config: VecStoreConfig,
}

impl VecStore {
    pub fn open(path: impl AsRef<Path>, config: VecStoreConfig) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA busy_timeout=5000;")?;
        schema::init_schema(&conn)?;
        Ok(Self { conn, config })
    }

    pub fn open_in_memory(config: VecStoreConfig) -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        schema::init_schema(&conn)?;
        Ok(Self { conn, config })
    }

    pub fn embeddings_match(&self, model: &str, dimension: usize) -> Result<bool> {
        let mismatches: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM vec_documents WHERE embedding IS NOT NULL AND (dimension != ?1 OR COALESCE(json_extract(metadata, '$.embedding_model'), '') != ?2)",
            rusqlite::params![dimension as i64, model], |row| row.get(0),
        )?;
        Ok(mismatches == 0)
    }

    pub fn upsert(&self, doc: &Document) -> Result<()> {
        if let Some(ref embedding) = doc.embedding {
            if let Some(dim) = self.config.dimension {
                if embedding.len() != dim {
                    return Err(VecStoreError::DimensionMismatch {
                        expected: dim,
                        actual: embedding.len(),
                    });
                }
            }
        }

        let metadata_json = serde_json::to_string(&doc.metadata)
            .map_err(|e| VecStoreError::Serialization(e.to_string()))?;

        let embedding_blob = doc
            .embedding
            .as_ref()
            .map(|v| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>());

        let dimension = doc.embedding.as_ref().map(|v| v.len() as i64);

        self.conn.execute(
            "INSERT INTO vec_documents (id, content, metadata, embedding, dimension, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
             ON CONFLICT(id) DO UPDATE SET
                content = excluded.content,
                metadata = excluded.metadata,
                embedding = excluded.embedding,
                dimension = excluded.dimension,
                updated_at = datetime('now')",
            params![
                doc.id,
                doc.content,
                metadata_json,
                embedding_blob,
                dimension
            ],
        )?;

        self.conn.execute(
            "DELETE FROM vec_documents_fts WHERE id = ?1",
            params![doc.id],
        )?;
        self.conn.execute(
            "INSERT INTO vec_documents_fts (id, content) VALUES (?1, ?2)",
            params![doc.id, doc.content],
        )?;

        Ok(())
    }

    pub fn batch_upsert(&mut self, docs: &[Document]) -> Result<()> {
        let tx = self.conn.transaction()?;
        for doc in docs {
            if let Some(ref embedding) = doc.embedding {
                if let Some(dim) = self.config.dimension {
                    if embedding.len() != dim {
                        return Err(VecStoreError::DimensionMismatch {
                            expected: dim,
                            actual: embedding.len(),
                        });
                    }
                }
            }

            let metadata_json = serde_json::to_string(&doc.metadata)
                .map_err(|e| VecStoreError::Serialization(e.to_string()))?;

            let embedding_blob = doc
                .embedding
                .as_ref()
                .map(|v| v.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>());
            let dimension = doc.embedding.as_ref().map(|v| v.len() as i64);

            tx.execute(
                "INSERT INTO vec_documents (id, content, metadata, embedding, dimension, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))
                 ON CONFLICT(id) DO UPDATE SET
                    content = excluded.content,
                    metadata = excluded.metadata,
                    embedding = excluded.embedding,
                    dimension = excluded.dimension,
                    updated_at = datetime('now')",
                params![doc.id, doc.content, metadata_json, embedding_blob, dimension],
            )?;

            tx.execute(
                "DELETE FROM vec_documents_fts WHERE id = ?1",
                params![doc.id],
            )?;
            tx.execute(
                "INSERT INTO vec_documents_fts (id, content) VALUES (?1, ?2)",
                params![doc.id, doc.content],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<Document>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, metadata, embedding, dimension FROM vec_documents WHERE id = ?1",
        )?;

        let result = stmt.query_row(params![id], |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let metadata_str: String = row.get(2)?;
            let embedding_blob: Option<Vec<u8>> = row.get(3)?;
            let dimension: Option<i64> = row.get(4)?;

            Ok((id, content, metadata_str, embedding_blob, dimension))
        });

        match result {
            Ok((id, content, metadata_str, embedding_blob, _dimension)) => {
                let metadata: serde_json::Value = serde_json::from_str(&metadata_str)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                let embedding = embedding_blob.map(|blob| {
                    blob.chunks_exact(4)
                        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                        .collect()
                });
                Ok(Some(Document {
                    id,
                    content,
                    metadata,
                    embedding,
                }))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let deleted = self
            .conn
            .execute("DELETE FROM vec_documents WHERE id = ?1", params![id])?;
        self.conn
            .execute("DELETE FROM vec_documents_fts WHERE id = ?1", params![id])?;
        Ok(deleted > 0)
    }

    pub fn text_search(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }

        let mut stmt = self.conn.prepare(
            "SELECT f.id, d.content, d.metadata, rank
             FROM vec_documents_fts f
             JOIN vec_documents d ON d.id = f.id
             WHERE vec_documents_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;

        let results = stmt
            .query_map(params![fts_query, limit as i64], |row| {
                let id: String = row.get(0)?;
                let content: String = row.get(1)?;
                let metadata_str: String = row.get(2)?;
                let rank: f64 = row.get(3)?;
                Ok((id, content, metadata_str, rank))
            })?
            .filter_map(|r| r.ok())
            .map(|(id, content, metadata_str, rank)| {
                let metadata: serde_json::Value = serde_json::from_str(&metadata_str)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                SearchResult {
                    document: Document {
                        id,
                        content,
                        metadata,
                        embedding: None,
                    },

                    score: -rank,
                }
            })
            .collect();

        Ok(results)
    }

    pub fn text_search_with_filter(
        &self,
        query: &str,
        filter: &MetadataFilter,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let fts_query = sanitize_fts_query(query);
        if fts_query.is_empty() {
            return Ok(vec![]);
        }

        let (filter_sql, filter_params) = filter.to_sql();

        let mut resolved_sql = filter_sql.clone();
        for (param_idx, _) in (3..).zip(filter_params.iter()) {
            resolved_sql = resolved_sql.replacen("{}", &format!("{param_idx}"), 1);
        }

        let sql = format!(
            "SELECT f.id, d.content, d.metadata, rank
             FROM vec_documents_fts f
             JOIN vec_documents d ON d.id = f.id
             WHERE vec_documents_fts MATCH ?1
               AND {resolved_sql}
             ORDER BY rank
             LIMIT ?2"
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let mut all_params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        all_params.push(Box::new(fts_query));
        all_params.push(Box::new(limit as i64));
        for p in &filter_params {
            all_params.push(Box::new(p.to_string()));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            all_params.iter().map(|b| b.as_ref()).collect();

        let results = stmt
            .query_map(param_refs.as_slice(), |row| {
                let id: String = row.get(0)?;
                let content: String = row.get(1)?;
                let metadata_str: String = row.get(2)?;
                let rank: f64 = row.get(3)?;
                Ok((id, content, metadata_str, rank))
            })?
            .filter_map(|r| r.ok())
            .map(|(id, content, metadata_str, rank)| {
                let metadata: serde_json::Value = serde_json::from_str(&metadata_str)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                SearchResult {
                    document: Document {
                        id,
                        content,
                        metadata,
                        embedding: None,
                    },
                    score: -rank,
                }
            })
            .collect();

        Ok(results)
    }

    pub fn vector_search(
        &self,
        query_embedding: &[f32],
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let dim = self.config.dimension.unwrap_or(query_embedding.len());
        if query_embedding.len() != dim {
            return Err(VecStoreError::DimensionMismatch {
                expected: dim,
                actual: query_embedding.len(),
            });
        }

        let mut stmt = self.conn.prepare(
            "SELECT id, content, metadata, embedding FROM vec_documents WHERE embedding IS NOT NULL",
        )?;

        let mut scored: Vec<SearchResult> = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let content: String = row.get(1)?;
                let metadata_str: String = row.get(2)?;
                let embedding_blob: Vec<u8> = row.get(3)?;
                Ok((id, content, metadata_str, embedding_blob))
            })?
            .filter_map(|r| r.ok())
            .filter_map(|(id, content, metadata_str, blob)| {
                let embedding: Vec<f32> = blob
                    .chunks_exact(4)
                    .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                    .collect();

                if embedding.len() != query_embedding.len() {
                    return None;
                }

                let score = cosine_similarity(query_embedding, &embedding);
                let metadata: serde_json::Value = serde_json::from_str(&metadata_str)
                    .unwrap_or(serde_json::Value::Object(Default::default()));

                Some(SearchResult {
                    document: Document {
                        id,
                        content,
                        metadata,
                        embedding: None,
                    },
                    score,
                })
            })
            .collect();

        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        scored.truncate(limit);

        Ok(scored)
    }

    pub fn count(&self) -> Result<usize> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM vec_documents", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    pub fn clear(&self) -> Result<()> {
        self.conn.execute_batch(
            "DELETE FROM vec_documents;
             DELETE FROM vec_documents_fts;",
        )?;
        Ok(())
    }

    pub fn list_recent(&self, limit: usize) -> Result<Vec<Document>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, content, metadata, embedding, dimension
             FROM vec_documents
             ORDER BY updated_at DESC
             LIMIT ?1",
        )?;

        let docs = stmt
            .query_map(params![limit as i64], |row| {
                let id: String = row.get(0)?;
                let content: String = row.get(1)?;
                let metadata_str: String = row.get(2)?;
                let embedding_blob: Option<Vec<u8>> = row.get(3)?;
                Ok((id, content, metadata_str, embedding_blob))
            })?
            .filter_map(|r| r.ok())
            .map(|(id, content, metadata_str, embedding_blob)| {
                let metadata: serde_json::Value = serde_json::from_str(&metadata_str)
                    .unwrap_or(serde_json::Value::Object(Default::default()));
                let embedding = embedding_blob.map(|blob| {
                    blob.chunks_exact(4)
                        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                        .collect()
                });
                Document {
                    id,
                    content,
                    metadata,
                    embedding,
                }
            })
            .collect();

        Ok(docs)
    }

    pub fn health_check(&self) -> Result<bool> {
        let _version = schema::get_schema_version(&self.conn)?;
        let _count = self.count()?;
        Ok(true)
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;
    for (ai, bi) in a.iter().zip(b.iter()) {
        let ai = *ai as f64;
        let bi = *bi as f64;
        dot += ai * bi;
        norm_a += ai * ai;
        norm_b += bi * bi;
    }
    let denom = norm_a.sqrt() * norm_b.sqrt();
    if denom < 1e-12 {
        0.0
    } else {
        dot / denom
    }
}

pub fn sanitize_fts_query(query: &str) -> String {
    let cleaned: String = query
        .chars()
        .filter(|c| !matches!(c, '"' | '*' | '+' | '-' | '(' | ')' | ':' | '^'))
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    trimmed
        .split_whitespace()
        .map(|word| format!("\"{word}\""))
        .collect::<Vec<_>>()
        .join(" ")
}
