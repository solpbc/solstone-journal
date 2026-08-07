// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Map, Value, json};
use solstone_core_generate::{GenerateResponse, RefusalReason, SessionCompletion};
use solstone_core_journal_io::{AtomicWriteOptions, install_file};
use solstone_core_processing_record::vocab;

use crate::WinnowConfig;
use crate::decode::{IdentityTransform, QualifiedFrame, process_video_with_transform};
use crate::notify;
use crate::request;
use crate::selection::{self, CategorizedFrame, CategoryOverride, SelectionError};
use crate::session::{DescribeSessionFactory, SystemSessionFactory};

pub const EXIT_PROVIDER_BLOCKED: i32 = 69;
const MAX_ATTEMPTS: u64 = 5;

#[derive(Debug)]
pub enum RunError {
    Blocked,
    Internal(String),
}

struct Pending {
    frame: QualifiedFrame,
    attempt: u64,
}

struct CategorizedRow {
    frame_id: u64,
    timestamp: f64,
    analysis: Value,
}

struct Promotion<'a> {
    output: &'a Path,
    rows: &'a Path,
    video: &'a Path,
    input_size: u64,
    decoded: &'a crate::DescribeResult,
    model: Option<String>,
    state: &'a str,
    reason: &'a str,
}

struct RowTemp {
    path: PathBuf,
    file: File,
}
impl RowTemp {
    fn new(parent: &Path) -> Result<Self, RunError> {
        for number in 0..1000_u32 {
            let path = parent.join(format!(
                ".describe-{}-{number}.jsonl.tmp",
                std::process::id()
            ));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok(Self { path, file }),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(RunError::Internal(error.to_string())),
            }
        }
        Err(RunError::Internal(
            "could not allocate describe temp file".to_owned(),
        ))
    }
    fn row(&mut self, row: &Value) -> Result<(), RunError> {
        writeln!(self.file, "{row}").map_err(|error| RunError::Internal(error.to_string()))?;
        self.file
            .flush()
            .map_err(|error| RunError::Internal(error.to_string()))
    }
}
impl Drop for RowTemp {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub struct DescribeOptions<'a> {
    pub video: &'a Path,
    pub journal: &'a Path,
    pub jobs: usize,
    pub config: WinnowConfig,
    pub redact_rules: Vec<String>,
    pub max_extractions: u32,
    pub category_overrides: BTreeMap<String, CategoryOverride>,
}

pub fn run(options: DescribeOptions<'_>) -> Result<(), RunError> {
    run_with_factory(options, &SystemSessionFactory)
}

pub fn run_with_factory(
    options: DescribeOptions<'_>,
    factory: &dyn DescribeSessionFactory,
) -> Result<(), RunError> {
    if options.jobs == 0 {
        return Err(RunError::Internal("--jobs must be positive".to_owned()));
    }
    let output = options.video.with_extension("jsonl");
    let parent = output
        .parent()
        .ok_or_else(|| RunError::Internal("video has no parent".to_owned()))?;
    let input_size = fs::metadata(options.video)
        .map_err(|error| RunError::Internal(error.to_string()))?
        .len();
    let mut transform = IdentityTransform;
    let mut decoded = process_video_with_transform(options.video, &mut transform, options.config);
    let mut rows = RowTemp::new(parent)?;
    if decoded.qualified_frames.is_empty() {
        let (state, reason) = if decoded.decode_failed {
            (vocab::STATE_FAILED, vocab::REASON_CORRUPT_INPUT)
        } else {
            (vocab::STATE_EMPTY, vocab::REASON_NO_DECODABLE_FRAMES)
        };
        return promote(Promotion {
            output: &output,
            rows: &rows.path,
            video: options.video,
            input_size,
            decoded: &decoded,
            model: None,
            state,
            reason,
        });
    }
    let work_key = options
        .video
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or("describe");
    let session = factory
        .spawn(options.jobs)
        .map_err(|_| blocked(options.journal, work_key, None, None, None))?;
    let instruction = request::system_instruction(&options.redact_rules);
    let mut waiting: VecDeque<Pending> = std::mem::take(&mut decoded.qualified_frames)
        .into_iter()
        .map(|frame| Pending { frame, attempt: 0 })
        .collect();
    let mut outstanding: HashMap<String, Pending> = HashMap::new();
    let mut categorized = Vec::new();
    let mut failures = false;
    let mut model = None;
    let window = options.jobs.saturating_mul(2);
    while !waiting.is_empty() || !outstanding.is_empty() {
        while outstanding.len() < window {
            let Some(pending) = waiting.pop_front() else {
                break;
            };
            let req = request::request(
                pending.frame.frame_id,
                pending.attempt,
                &pending.frame.png,
                &instruction,
            );
            let id = req.id.clone().expect("describe request ids are present");
            if session.submit(req).is_err() {
                return Err(blocked(options.journal, work_key, None, None, None));
            }
            outstanding.insert(id, pending);
        }
        let completion = session
            .recv_timeout(Duration::from_secs(120))
            .map_err(|_| blocked(options.journal, work_key, None, None, None))?;
        let SessionCompletion::Response(response) = completion else {
            return Err(blocked(options.journal, work_key, None, None, None));
        };
        let id = match &response {
            GenerateResponse::Generated(g) => g.id.clone(),
            GenerateResponse::Refused(r) => r.id.clone(),
        }
        .ok_or_else(|| RunError::Internal("session response omitted id".to_owned()))?;
        let pending = outstanding
            .remove(&id)
            .ok_or_else(|| RunError::Internal("uncorrelated response".to_owned()))?;
        match response {
            GenerateResponse::Generated(generated) => {
                model.get_or_insert(generated.model.clone());
                let analysis = serde_json::from_str::<Value>(&generated.text)
                    .unwrap_or_else(|_| json!({"error":"Invalid JSON response"}));
                let error = analysis.get("error").is_some();
                failures |= error;
                rows.row(&json!({"frame_id":pending.frame.frame_id,"timestamp":pending.frame.timestamp,"analysis":analysis,"enhanced":false,"finish_reason":generated.finish_reason,"requests":[{"type":"describe","model":generated.model,"attempt":pending.attempt,"retries":pending.attempt}]}))?;
                categorized.push(CategorizedRow {
                    frame_id: pending.frame.frame_id,
                    timestamp: pending.frame.timestamp,
                    analysis,
                });
            }
            GenerateResponse::Refused(refusal) => {
                let code = refusal.reason_code.as_ref().map(|value| value.as_wire());
                if refusal.reason == RefusalReason::NoEngineConfigured || refusal.blocking {
                    let _ = session.close();
                    return Err(blocked(
                        options.journal,
                        work_key,
                        code,
                        refusal.provider.as_deref(),
                        Some("observe.describe.frame"),
                    ));
                }
                if refusal.retryable && pending.attempt + 1 < MAX_ATTEMPTS {
                    waiting.push_back(Pending {
                        frame: pending.frame,
                        attempt: pending.attempt + 1,
                    });
                } else {
                    failures = true;
                    rows.row(&json!({"frame_id":pending.frame.frame_id,"timestamp":pending.frame.timestamp,"error":refusal.detail,"enhanced":false,"finish_reason":"unknown","requests":[{"type":"describe","attempt":pending.attempt,"retries":pending.attempt,"reason_code":code}]}))?;
                    categorized.push(CategorizedRow {
                        frame_id: pending.frame.frame_id,
                        timestamp: pending.frame.timestamp,
                        analysis: json!({"error": refusal.detail}),
                    });
                }
            }
        }
    }
    let mut selection_frames = categorized
        .iter()
        .map(|row| CategorizedFrame {
            frame_id: row.frame_id,
            timestamp: row.timestamp,
            analysis: row.analysis.clone(),
        })
        .collect::<Vec<_>>();
    selection_frames.sort_unstable_by_key(|frame| frame.frame_id);
    match selection::select(
        session.as_ref(),
        &selection_frames,
        options.max_extractions,
        &options.category_overrides,
    ) {
        Ok(_) => {}
        Err(SelectionError::Blocked {
            reason_code,
            provider,
        }) => {
            let _ = session.close();
            return Err(blocked(
                options.journal,
                work_key,
                reason_code.as_deref(),
                provider.as_deref(),
                Some("observe.extract.selection"),
            ));
        }
        Err(SelectionError::Session) => {
            let _ = session.close();
            return Err(blocked(options.journal, work_key, None, None, None));
        }
    }
    let _ = session.close();
    let (state, reason) = if decoded.decode_failed {
        (vocab::STATE_FAILED, vocab::REASON_CORRUPT_INPUT)
    } else if failures {
        (vocab::STATE_FAILED, vocab::REASON_ANALYSIS_FAILED)
    } else {
        (vocab::STATE_ANALYZED, vocab::REASON_OK)
    };
    promote(Promotion {
        output: &output,
        rows: &rows.path,
        video: options.video,
        input_size,
        decoded: &decoded,
        model,
        state,
        reason,
    })
}

fn blocked(
    journal: &Path,
    work_key: &str,
    code: Option<&str>,
    provider: Option<&str>,
    context: Option<&str>,
) -> RunError {
    notify::blocked(journal, work_key, code, provider, context);
    RunError::Blocked
}

fn promote(promotion: Promotion<'_>) -> Result<(), RunError> {
    let parent = promotion
        .output
        .parent()
        .ok_or_else(|| RunError::Internal("output has no parent".to_owned()))?;
    let final_path = parent.join(format!(".describe-final-{}.jsonl.tmp", std::process::id()));
    let result = (|| {
        let mut header = Map::new();
        header.insert(
            "raw".to_owned(),
            json!(
                promotion
                    .video
                    .file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or_default()
            ),
        );
        if let Ok(observer) = std::env::var("OBSERVER_NAME")
            && !observer.is_empty()
        {
            header.insert("observer".to_owned(), json!(observer));
        }
        if let Ok(meta) = std::env::var("SEGMENT_META") {
            match serde_json::from_str::<Value>(&meta) {
                Ok(Value::Object(values)) => header.extend(values),
                Ok(_) => {
                    return Err(RunError::Internal(
                        "SEGMENT_META must be an object".to_owned(),
                    ));
                }
                Err(_) => {}
            }
        }
        header.insert(
            "first_hash".to_owned(),
            json!(promotion.decoded.first_hash.map(|v| format!("{v:016x}"))),
        );
        header.insert(
            "last_hash".to_owned(),
            json!(promotion.decoded.last_hash.map(|v| format!("{v:016x}"))),
        );
        header.insert(
            "qualified_count".to_owned(),
            json!(promotion.decoded.qualified_count),
        );
        if let Some(model) = promotion.model {
            header.insert("_solstone_thinking".to_owned(), json!({"model":model}));
        }
        let mut record = json!({"schema":vocab::SCHEMA,"state":promotion.state,"reason_code":promotion.reason,"handler":vocab::HANDLER_DESCRIBE,"attempted_at":chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),"input_size":promotion.input_size});
        if promotion.state == vocab::STATE_FAILED {
            record["attempts"] = json!(1);
        }
        header.insert("_solstone_processing".to_owned(), record);
        let mut final_file =
            File::create(&final_path).map_err(|error| RunError::Internal(error.to_string()))?;
        writeln!(final_file, "{}", Value::Object(header))
            .map_err(|error| RunError::Internal(error.to_string()))?;
        for line in BufReader::new(
            File::open(promotion.rows).map_err(|error| RunError::Internal(error.to_string()))?,
        )
        .lines()
        {
            writeln!(
                final_file,
                "{}",
                line.map_err(|error| RunError::Internal(error.to_string()))?
            )
            .map_err(|error| RunError::Internal(error.to_string()))?;
        }
        drop(final_file);
        install_file(&final_path, promotion.output, AtomicWriteOptions::default())
            .map_err(|error| RunError::Internal(error.to_string()))
    })();
    if result.is_err() {
        let _ = fs::remove_file(&final_path);
    }
    result
}
