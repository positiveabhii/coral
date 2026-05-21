//! Lazy `DataFusion` execution plan for manifest-driven HTTP writes.

use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

use datafusion::arrow::array::{
    Array, BooleanArray, Float32Array, Float64Array, Int32Array, Int64Array, RecordBatch,
    StringArray, UInt32Array, UInt64Array,
};
use datafusion::arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use datafusion::error::{DataFusionError, Result};
use datafusion::execution::TaskContext;
use datafusion::physical_expr::EquivalenceProperties;
use datafusion::physical_plan::execution_plan::{Boundedness, EmissionType};
use datafusion::physical_plan::stream::RecordBatchStreamAdapter;
use datafusion::physical_plan::{
    DisplayAs, DisplayFormatType, ExecutionPlan, Partitioning, PlanProperties,
    SendableRecordBatchStream,
};
use futures::stream;
use serde_json::{Value, json};
use tracing::{Instrument, field};

use crate::backends::http::HttpSourceClient;
use crate::backends::http::target::HttpWriteTarget;
use coral_spec::backends::http::HttpRelationWriteOperationSpec;

#[derive(Clone)]
enum HttpWriteInput {
    Insert { input: Arc<dyn ExecutionPlan> },
    Single { values: HashMap<String, Value> },
}

/// Execution-plan node that performs provider writes only when executed.
pub(crate) struct HttpWriteExec {
    backend: HttpSourceClient,
    source_schema: String,
    relation_name: String,
    operation: Arc<HttpRelationWriteOperationSpec>,
    target: Arc<HttpWriteTarget>,
    key_values: HashMap<String, String>,
    input: HttpWriteInput,
    output_schema: SchemaRef,
    props: Arc<PlanProperties>,
}

impl fmt::Debug for HttpWriteExec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpWriteExec")
            .field("source", &self.source_schema)
            .field("relation", &self.relation_name)
            .field("operation", &self.operation.operation.as_str())
            .finish_non_exhaustive()
    }
}

impl HttpWriteExec {
    pub(crate) fn insert(
        backend: HttpSourceClient,
        source_schema: String,
        relation_name: String,
        operation: HttpRelationWriteOperationSpec,
        target: HttpWriteTarget,
        input: Arc<dyn ExecutionPlan>,
    ) -> Self {
        Self::new(
            backend,
            source_schema,
            relation_name,
            operation,
            target,
            HashMap::new(),
            HttpWriteInput::Insert { input },
        )
    }

    pub(crate) fn single(
        backend: HttpSourceClient,
        source_schema: String,
        relation_name: String,
        operation: HttpRelationWriteOperationSpec,
        target: HttpWriteTarget,
        key_values: HashMap<String, String>,
        values: HashMap<String, Value>,
    ) -> Self {
        Self::new(
            backend,
            source_schema,
            relation_name,
            operation,
            target,
            key_values,
            HttpWriteInput::Single { values },
        )
    }

    fn new(
        backend: HttpSourceClient,
        source_schema: String,
        relation_name: String,
        operation: HttpRelationWriteOperationSpec,
        target: HttpWriteTarget,
        key_values: HashMap<String, String>,
        input: HttpWriteInput,
    ) -> Self {
        let output_schema = dml_count_schema();
        let props = Arc::new(PlanProperties::new(
            EquivalenceProperties::new(output_schema.clone()),
            Partitioning::UnknownPartitioning(1),
            EmissionType::Incremental,
            Boundedness::Bounded,
        ));
        Self {
            backend,
            source_schema,
            relation_name,
            operation: Arc::new(operation),
            target: Arc::new(target),
            key_values,
            input,
            output_schema,
            props,
        }
    }
}

impl DisplayAs for HttpWriteExec {
    fn fmt_as(&self, _format: DisplayFormatType, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "HttpWriteExec: source={}, relation={}, operation={}",
            self.source_schema,
            self.relation_name,
            self.operation.operation.as_str()
        )
    }
}

impl ExecutionPlan for HttpWriteExec {
    fn name(&self) -> &'static str {
        "HttpWriteExec"
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.output_schema.clone()
    }

    fn properties(&self) -> &Arc<PlanProperties> {
        &self.props
    }

    fn partition_statistics(
        &self,
        _partition: Option<usize>,
    ) -> Result<datafusion::common::Statistics> {
        Ok(datafusion::common::Statistics::new_unknown(&self.schema()))
    }

    fn children(&self) -> Vec<&Arc<dyn ExecutionPlan>> {
        match &self.input {
            HttpWriteInput::Insert { input } => vec![input],
            HttpWriteInput::Single { .. } => vec![],
        }
    }

    fn with_new_children(
        self: Arc<Self>,
        mut children: Vec<Arc<dyn ExecutionPlan>>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        match &self.input {
            HttpWriteInput::Insert { .. } => {
                if children.len() != 1 {
                    return Err(DataFusionError::Plan(format!(
                        "HttpWriteExec insert expects one child, got {}",
                        children.len()
                    )));
                }
                let input = children.remove(0);
                Ok(Arc::new(Self::insert(
                    self.backend.clone(),
                    self.source_schema.clone(),
                    self.relation_name.clone(),
                    (*self.operation).clone(),
                    (*self.target).clone(),
                    input,
                )))
            }
            HttpWriteInput::Single { .. } => Ok(self),
        }
    }

    fn execute(
        &self,
        _partition: usize,
        context: Arc<TaskContext>,
    ) -> Result<SendableRecordBatchStream> {
        let backend = self.backend.clone();
        let target = self.target.clone();
        let operation = self.operation.clone();
        let key_values = self.key_values.clone();
        let input = self.input.clone();
        let output_schema = self.output_schema.clone();
        let stream_schema = output_schema.clone();
        let operation_name = self.operation.operation.as_str();
        let effect = match self.operation.operation {
            coral_spec::backends::http::HttpRelationWriteOperation::Insert
            | coral_spec::backends::http::HttpRelationWriteOperation::Update => "write",
            coral_spec::backends::http::HttpRelationWriteOperation::Delete
            | coral_spec::backends::http::HttpRelationWriteOperation::Truncate => "destructive",
        };
        let write_span = tracing::info_span!(
            target: "coral_engine::http",
            "http.write",
            coral.source = self.source_schema.as_str(),
            coral.sql.target.kind = "relation",
            coral.sql.target.name = self.relation_name.as_str(),
            coral.sql.operation = operation_name,
            coral.sql.effect = effect,
            coral.sql.affected_count = field::Empty,
        );
        let record_span = write_span.clone();

        let stream = stream::once(
            async move {
                let count = match input {
                    HttpWriteInput::Insert { input } => {
                        execute_insert(backend, target, operation, input, context).await?
                    }
                    HttpWriteInput::Single { values } => {
                        backend.write(&target, &key_values, &values).await?;
                        1
                    }
                };
                record_span.record(
                    "coral.sql.affected_count",
                    i64::try_from(count).unwrap_or(i64::MAX),
                );
                count_batch(stream_schema, count)
            }
            .instrument(write_span),
        );

        Ok(Box::pin(RecordBatchStreamAdapter::new(
            output_schema,
            stream,
        )))
    }
}

async fn execute_insert(
    backend: HttpSourceClient,
    target: Arc<HttpWriteTarget>,
    operation: Arc<HttpRelationWriteOperationSpec>,
    input: Arc<dyn ExecutionPlan>,
    context: Arc<TaskContext>,
) -> Result<u64> {
    let batches = datafusion::physical_plan::collect(input, context).await?;
    let mut count = 0_u64;
    for batch in &batches {
        for row in 0..batch.num_rows() {
            let values = write_values_from_row(batch, row, operation.as_ref())?;
            backend.write(&target, &HashMap::new(), &values).await?;
            count = count.saturating_add(1);
        }
    }
    Ok(count)
}

fn write_values_from_row(
    batch: &RecordBatch,
    row: usize,
    operation: &HttpRelationWriteOperationSpec,
) -> Result<HashMap<String, Value>> {
    let writable = operation
        .input_columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    let required = operation
        .input_columns
        .iter()
        .filter(|column| !column.nullable)
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    let mut values = HashMap::new();

    for (index, field) in batch.schema().fields().iter().enumerate() {
        let value = array_json_value(batch.column(index), row)?;
        if writable.contains(field.name().as_str()) {
            if let Some(value) = value {
                values.insert(field.name().clone(), value);
            }
        } else if value.is_some() {
            return Err(DataFusionError::Plan(format!(
                "INSERT cannot write non-writable column '{}'",
                field.name()
            )));
        }
    }

    for required in required {
        if !values.contains_key(required) {
            return Err(DataFusionError::Plan(format!(
                "INSERT missing required writable column '{required}'"
            )));
        }
    }

    Ok(values)
}

fn array_json_value(array: &Arc<dyn Array>, row: usize) -> Result<Option<Value>> {
    if array.is_null(row) {
        return Ok(None);
    }
    let value = match array.data_type() {
        DataType::Utf8 => json!(downcast_array::<StringArray>(array)?.value(row)),
        DataType::Int64 => json!(downcast_array::<Int64Array>(array)?.value(row)),
        DataType::Int32 => json!(downcast_array::<Int32Array>(array)?.value(row)),
        DataType::UInt64 => json!(downcast_array::<UInt64Array>(array)?.value(row)),
        DataType::UInt32 => json!(downcast_array::<UInt32Array>(array)?.value(row)),
        DataType::Float64 => json!(downcast_array::<Float64Array>(array)?.value(row)),
        DataType::Float32 => json!(downcast_array::<Float32Array>(array)?.value(row)),
        DataType::Boolean => json!(downcast_array::<BooleanArray>(array)?.value(row)),
        other => {
            return Err(DataFusionError::Plan(format!(
                "INSERT cannot serialize Arrow type {other}"
            )));
        }
    };
    Ok(Some(value))
}

fn downcast_array<T: 'static>(array: &Arc<dyn Array>) -> Result<&T> {
    array.as_any().downcast_ref::<T>().ok_or_else(|| {
        DataFusionError::Execution(format!(
            "failed to downcast Arrow array with type {}",
            array.data_type()
        ))
    })
}

fn dml_count_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![Field::new(
        "count",
        DataType::UInt64,
        false,
    )]))
}

fn count_batch(schema: SchemaRef, count: u64) -> Result<RecordBatch> {
    RecordBatch::try_new(schema, vec![Arc::new(UInt64Array::from(vec![count]))])
        .map_err(|error| DataFusionError::ArrowError(Box::new(error), None))
}
