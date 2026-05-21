//! Implements the gRPC `SqlService`.

use arrow::datatypes::SchemaRef;
use arrow::ipc::writer::StreamWriter;
use arrow::record_batch::RecordBatch;
use coral_api::v1::sql_service_server::SqlService as SqlServiceApi;
use coral_api::v1::{
    ExecuteSqlRequest, ExecuteSqlResponse, ExplainSqlRequest, ExplainSqlResponse,
    ListRelationsRequest, ListRelationsResponse, PaginationResponse, QueryPlan as QueryPlanProto,
    SqlExecutionSummary as SqlExecutionSummaryProto,
};
use tonic::{Request, Response, Status};

use crate::bootstrap::core_status;
use crate::query::manager::QueryManager;
use crate::transport::{
    grpc_span, instrument_grpc, query_status, relation_summary_to_proto, relation_to_proto,
    workspace_name_from_proto,
};

#[derive(Clone)]
pub(crate) struct SqlService {
    queries: QueryManager,
}

impl SqlService {
    pub(crate) fn new(query_manager: QueryManager) -> Self {
        Self {
            queries: query_manager,
        }
    }
}

#[tonic::async_trait]
impl SqlServiceApi for SqlService {
    async fn list_relations(
        &self,
        request: Request<ListRelationsRequest>,
    ) -> Result<Response<ListRelationsResponse>, Status> {
        let span = grpc_span(&request);
        let queries = self.queries.clone();
        instrument_grpc(span, async move {
            let request = request.into_inner();
            let pagination = request.pagination.unwrap_or_default();
            let workspace_name = workspace_name_from_proto(request.workspace.as_ref())?;
            let schema_name = request.schema_name.trim();
            let schema_name = if schema_name.is_empty() {
                None
            } else {
                Some(schema_name)
            };
            let relation_name = request.relation_name.trim();
            let relation_name = if relation_name.is_empty() {
                None
            } else {
                Some(relation_name)
            };
            let relations = queries
                .list_relations(&workspace_name, schema_name, relation_name)
                .await
                .map_err(query_status)?;
            let total = relations.len();
            let offset = pagination.offset as usize;
            let limit = pagination.limit as usize;
            let page = paginate_relations(relations, offset, limit);
            let returned_count = page.len();
            let has_more = pagination.limit != 0 && offset.saturating_add(returned_count) < total;
            let (relations, relation_summaries) = if request.omit_columns {
                (
                    Vec::new(),
                    page.into_iter()
                        .map(|relation| relation_summary_to_proto(&workspace_name, relation))
                        .collect(),
                )
            } else {
                (
                    page.into_iter()
                        .map(|relation| relation_to_proto(&workspace_name, relation))
                        .collect(),
                    Vec::new(),
                )
            };
            Ok(Response::new(ListRelationsResponse {
                relations,
                relation_summaries,
                pagination: Some(PaginationResponse {
                    total_count: count_to_u32(total),
                    limit: pagination.limit,
                    offset: pagination.offset,
                    has_more,
                    next_offset: if has_more {
                        count_to_u32(offset.saturating_add(returned_count))
                    } else {
                        0
                    },
                }),
            }))
        })
        .await
    }

    async fn execute_sql(
        &self,
        request: Request<ExecuteSqlRequest>,
    ) -> Result<Response<ExecuteSqlResponse>, Status> {
        let span = grpc_span(&request);
        let queries = self.queries.clone();
        Box::pin(instrument_grpc(span, async move {
            let inner = request.into_inner();
            let workspace_name = workspace_name_from_proto(inner.workspace.as_ref())?;
            let execution = queries
                .execute_sql(&workspace_name, &inner.sql)
                .await
                .map_err(query_status)?;
            let response = ExecuteSqlResponse {
                arrow_ipc_stream: encode_arrow_ipc_stream(
                    execution.arrow_schema(),
                    execution.batches(),
                )
                .map_err(coral_engine::CoreError::from)
                .map_err(core_status)?,
                row_count: i64::try_from(execution.row_count()).unwrap_or(i64::MAX),
                summary: Some(sql_execution_summary_to_proto(execution.summary())),
            };
            Ok(Response::new(response))
        }))
        .await
    }

    async fn explain_sql(
        &self,
        request: Request<ExplainSqlRequest>,
    ) -> Result<Response<ExplainSqlResponse>, Status> {
        let span = grpc_span(&request);
        let queries = self.queries.clone();
        instrument_grpc(span, async move {
            let inner = request.into_inner();
            let workspace_name = workspace_name_from_proto(inner.workspace.as_ref())?;
            let plan = queries
                .explain_sql(&workspace_name, &inner.sql)
                .await
                .map_err(query_status)?;
            Ok(Response::new(ExplainSqlResponse {
                plan: Some(query_plan_to_proto(&plan)),
            }))
        })
        .await
    }
}

fn paginate_relations(
    relations: Vec<coral_engine::RelationInfo>,
    offset: usize,
    limit: usize,
) -> Vec<coral_engine::RelationInfo> {
    let iter = relations.into_iter().skip(offset);
    if limit == 0 {
        iter.collect()
    } else {
        iter.take(limit).collect()
    }
}

fn count_to_u32(count: usize) -> u32 {
    u32::try_from(count).unwrap_or(u32::MAX)
}

fn query_plan_to_proto(plan: &coral_engine::QueryPlan) -> QueryPlanProto {
    QueryPlanProto {
        unoptimized_logical_plan: plan.unoptimized_logical_plan().to_string(),
        optimized_logical_plan: plan.optimized_logical_plan().to_string(),
        physical_plan: plan.physical_plan().to_string(),
    }
}

fn sql_execution_summary_to_proto(
    summary: &coral_engine::SqlExecutionSummary,
) -> SqlExecutionSummaryProto {
    SqlExecutionSummaryProto {
        statement_kind: summary.statement_kind().to_string(),
        effect: summary.effect().to_string(),
        affected_row_count: summary.affected_row_count(),
    }
}

fn encode_arrow_ipc_stream(
    schema: &SchemaRef,
    batches: &[RecordBatch],
) -> Result<Vec<u8>, arrow::error::ArrowError> {
    let mut bytes = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut bytes, schema)?;
        for batch in batches {
            writer.write(batch)?;
        }
        writer.finish()?;
    }
    Ok(bytes)
}
