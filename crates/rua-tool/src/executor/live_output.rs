use std::path::{Path, PathBuf};
use std::sync::Weak;

use async_trait::async_trait;
use rua_task::{
    TaskLiveOutputBatch, TaskLiveOutputCursor, TaskLiveOutputError, TaskLiveOutputRecord,
    TaskLiveOutputSource, TaskRunnerSession,
};
use tokio::io::{AsyncBufReadExt, AsyncSeekExt, AsyncWriteExt};
use tokio::sync::Mutex;

#[derive(Default)]
struct PlainText(String);

impl vte::Perform for PlainText {
    fn print(&mut self, c: char) {
        self.0.push(c);
    }
    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | b'\t' => self.0.push(byte as char),
            b'\r' => {}
            _ => {}
        }
    }
}

pub(crate) fn plain_text(bytes: &[u8]) -> String {
    let mut text = PlainText::default();
    vte::Parser::new().advance(&mut text, bytes);
    text.0
}

struct Capture {
    file: tokio::fs::File,
    parsers: [vte::Parser; 2],
    len: u64,
    sealed: u8,
    error: Option<String>,
}

pub(crate) struct ShellLiveOutputSource {
    path: PathBuf,
    capture: Mutex<Capture>,
    session: Weak<TaskRunnerSession>,
}

impl ShellLiveOutputSource {
    pub(crate) async fn reopen(path: PathBuf) -> std::io::Result<Self> {
        let file = tokio::fs::File::open(&path).await?;
        let len = file.metadata().await?.len();
        Ok(Self {
            path,
            capture: Mutex::new(Capture {
                file,
                parsers: [vte::Parser::new(), vte::Parser::new()],
                len,
                sealed: 3,
                error: None,
            }),
            session: Weak::new(),
        })
    }

    pub(super) async fn new(
        path: PathBuf,
        session: Weak<TaskRunnerSession>,
    ) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await?;
        Ok(Self {
            path,
            capture: Mutex::new(Capture {
                file,
                parsers: [vte::Parser::new(), vte::Parser::new()],
                len: 0,
                sealed: 0,
                error: None,
            }),
            session,
        })
    }

    fn notify(&self) {
        if let Some(session) = self.session.upgrade() {
            session.notify_live_output();
        }
    }
}

#[async_trait]
impl crate::raw_capture::RawCaptureObserver for ShellLiveOutputSource {
    async fn appended(&self, source: &str, bytes: &[u8]) {
        let mut capture = self.capture.lock().await;
        if capture.error.is_some() {
            return;
        }
        let mut plain = PlainText::default();
        capture.parsers[usize::from(source == "stderr")].advance(&mut plain, bytes);
        if plain.0.is_empty() {
            return;
        }
        let record = TaskLiveOutputRecord {
            source: source.into(),
            text: plain.0,
        };
        let mut bytes = serde_json::to_vec(&record).expect("output record is serializable");
        bytes.push(b'\n');
        match capture.file.write_all(&bytes).await {
            Ok(()) => match capture.file.flush().await {
                Ok(()) => capture.len += bytes.len() as u64,
                Err(error) => capture.error = Some(error.to_string()),
            },
            Err(error) => capture.error = Some(error.to_string()),
        }
        drop(capture);
        self.notify();
    }

    async fn sealed(&self, source: &str) {
        let mut capture = self.capture.lock().await;
        capture.sealed |= if source == "stderr" { 2 } else { 1 };
        if let Err(error) = capture.file.sync_all().await {
            capture.error = Some(error.to_string());
        }
        drop(capture);
        self.notify();
    }
}

async fn record_at(
    path: &Path,
    position: u64,
) -> Result<(TaskLiveOutputRecord, u64), TaskLiveOutputError> {
    let mut file = tokio::fs::File::open(path).await.map_err(io_error)?;
    if position > 0 {
        file.seek(std::io::SeekFrom::Start(position - 1))
            .await
            .map_err(io_error)?;
        let mut byte = [0];
        tokio::io::AsyncReadExt::read_exact(&mut file, &mut byte)
            .await
            .map_err(io_error)?;
        if byte[0] != b'\n' {
            return Err(TaskLiveOutputError::new(
                "Cursor is not an output record boundary",
            ));
        }
    }
    file.seek(std::io::SeekFrom::Start(position))
        .await
        .map_err(io_error)?;
    let mut line = String::new();
    tokio::io::BufReader::new(file)
        .read_line(&mut line)
        .await
        .map_err(io_error)?;
    let record = serde_json::from_str(&line)
        .map_err(|e| TaskLiveOutputError::new(format!("Invalid output record: {e}")))?;
    Ok((record, position + line.len() as u64))
}

fn io_error(error: std::io::Error) -> TaskLiveOutputError {
    TaskLiveOutputError::new(error.to_string())
}

#[async_trait]
impl TaskLiveOutputSource for ShellLiveOutputSource {
    async fn end_cursor(&self) -> Result<TaskLiveOutputCursor, TaskLiveOutputError> {
        let mut cursor = TaskLiveOutputCursor::default();
        cursor
            .positions
            .insert("record".into(), self.capture.lock().await.len);
        Ok(cursor)
    }

    async fn read_records(
        &self,
        cursor: &TaskLiveOutputCursor,
        max_records: usize,
        max_bytes: usize,
    ) -> Result<TaskLiveOutputBatch, TaskLiveOutputError> {
        let (len, sealed) = {
            let capture = self.capture.lock().await;
            if let Some(error) = &capture.error {
                return Err(TaskLiveOutputError::new(error.clone()));
            }
            (capture.len, capture.sealed == 3)
        };
        if !cursor.pending.is_empty()
            || cursor
                .positions
                .keys()
                .any(|key| key != "record" && key != "offset")
        {
            return Err(TaskLiveOutputError::new("Invalid output cursor fields"));
        }
        let mut position = cursor.positions.get("record").copied().unwrap_or(0);
        let mut offset = cursor.positions.get("offset").copied().unwrap_or(0) as usize;
        if position > len || (position == len && offset != 0) {
            return Err(TaskLiveOutputError::new(
                "Output cursor exceeds stream length",
            ));
        }
        let mut records = Vec::new();
        let mut used = 0;
        while position < len && records.len() < max_records && used < max_bytes {
            let (record, next) = record_at(&self.path, position).await?;
            if offset > record.text.len() || !record.text.is_char_boundary(offset) {
                return Err(TaskLiveOutputError::new("Invalid UTF-8 output offset"));
            }
            let mut end = offset
                .saturating_add(max_bytes - used)
                .min(record.text.len());
            while !record.text.is_char_boundary(end) {
                end -= 1;
            }
            if end == offset && offset < record.text.len() {
                if records.is_empty() {
                    return Err(TaskLiveOutputError::new(
                        "Output budget is too small for the next character; use at least 4",
                    ));
                }
                break;
            }
            let text = record.text[offset..end].to_string();
            used += text.len();
            records.push(TaskLiveOutputRecord {
                source: record.source,
                text,
            });
            if end == record.text.len() {
                position = next;
                offset = 0;
            } else {
                offset = end;
            }
        }
        let mut next_cursor = TaskLiveOutputCursor::default();
        next_cursor.positions.insert("record".into(), position);
        if offset > 0 {
            next_cursor.positions.insert("offset".into(), offset as u64);
        }
        Ok(TaskLiveOutputBatch {
            records,
            next_cursor,
            has_more: position < len,
            sealed: sealed && position == len,
        })
    }

    async fn seal(&self) {
        let mut capture = self.capture.lock().await;
        capture.sealed = 3;
        if let Err(error) = capture.file.sync_all().await {
            capture.error = Some(error.to_string());
        }
        drop(capture);
        self.notify();
    }
}
