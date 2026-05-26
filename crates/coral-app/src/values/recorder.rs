use arrow::json::writer::{JsonArray, WriterBuilder};
use arrow::record_batch::RecordBatch;
use chrono::{SecondsFormat, Utc};
use serde_json::Value;
use std::collections::BTreeMap;
use tokio::sync::mpsc;
use tokio::task;

use crate::state::AppStateLayout;
use crate::values::extract::{CandidateValue, collect_row_values};
use crate::values::store::{ValueMemoryError, ValueMemoryStore, ValueRollup};
use crate::values::surface::infer_observed_surface;
use crate::workspaces::WorkspaceName;

const VALUE_MEMORY_QUEUE_CAPACITY: usize = 32;

#[derive(Debug, thiserror::Error)]
pub(crate) enum ValueMemoryRecordError {
    #[error("value memory writer queue is full")]
    QueueFull,
    #[error("value memory writer has stopped")]
    QueueClosed,
    #[error(transparent)]
    Arrow(#[from] arrow::error::ArrowError),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Store(#[from] ValueMemoryError),
}

#[derive(Clone)]
pub(crate) struct ValueMemoryRecorder {
    sender: mpsc::Sender<ValueMemoryJob>,
}

impl ValueMemoryRecorder {
    pub(crate) fn new(layout: AppStateLayout) -> Self {
        let (sender, receiver) = mpsc::channel(VALUE_MEMORY_QUEUE_CAPACITY);
        tokio::spawn(run_value_memory_worker(layout, receiver));
        Self { sender }
    }

    pub(crate) fn record_result(
        &self,
        workspace_name: &WorkspaceName,
        sql: &str,
        batches: &[RecordBatch],
    ) -> Result<(), ValueMemoryRecordError> {
        if batches.is_empty() {
            return Ok(());
        }
        let job = ValueMemoryJob {
            workspace_name: workspace_name.clone(),
            sql: sql.to_string(),
            batches: batches.to_vec(),
            observed_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        };
        self.sender.try_send(job).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => ValueMemoryRecordError::QueueFull,
            mpsc::error::TrySendError::Closed(_) => ValueMemoryRecordError::QueueClosed,
        })
    }
}

struct ValueMemoryJob {
    workspace_name: WorkspaceName,
    sql: String,
    batches: Vec<RecordBatch>,
    observed_at: String,
}

async fn run_value_memory_worker(
    layout: AppStateLayout,
    mut receiver: mpsc::Receiver<ValueMemoryJob>,
) {
    while let Some(job) = receiver.recv().await {
        let layout = layout.clone();
        match task::spawn_blocking(move || process_value_memory_job(layout, job)).await {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                tracing::warn!(
                    detail = %error,
                    "failed to index query result values"
                );
            }
            Err(error) => {
                tracing::warn!(
                    detail = %error,
                    "value memory writer task failed"
                );
            }
        }
    }
}

fn process_value_memory_job(
    layout: AppStateLayout,
    job: ValueMemoryJob,
) -> Result<(), ValueMemoryRecordError> {
    let Some(surface) = infer_observed_surface(&job.sql) else {
        tracing::debug!("skipping value memory indexing for query without one inferred surface");
        return Ok(());
    };
    if surface.schema_name == "coral" {
        tracing::debug!("skipping value memory indexing for Coral metadata query");
        return Ok(());
    }
    let mut observed = BTreeMap::<CandidateValue, u64>::new();
    for batch in &job.batches {
        for row in batch_to_json_rows(batch)? {
            for value in collect_row_values(&row) {
                *observed.entry(value).or_insert(0) += 1;
            }
        }
    }

    let rollups = observed
        .into_iter()
        .map(|(value, seen_count)| ValueRollup {
            workspace_name: job.workspace_name.as_str().to_string(),
            schema_name: surface.schema_name.clone(),
            table_name: surface.table_name.clone(),
            column_path: value.column_path,
            value: value.value,
            value_truncated: value.value_truncated,
            search_text: value.search_text,
            value_hash: value.value_hash,
            rank: value.kind.rank(),
            seen_count,
            observed_at: job.observed_at.clone(),
        })
        .collect::<Vec<_>>();
    let store = ValueMemoryStore::new(layout.value_memory_file(&job.workspace_name));
    store.upsert_rollups(rollups)?;
    Ok(())
}

fn batch_to_json_rows(batch: &RecordBatch) -> Result<Vec<Value>, ValueMemoryRecordError> {
    let mut bytes = Vec::new();
    {
        let mut writer = WriterBuilder::new()
            .with_explicit_nulls(false)
            .build::<_, JsonArray>(&mut bytes);
        writer.write(batch)?;
        writer.finish()?;
    }
    serde_json::from_slice(&bytes).map_err(Into::into)
}
