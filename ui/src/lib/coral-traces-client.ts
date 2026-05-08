import { create } from '@bufbuild/protobuf'
import { createClient } from '@connectrpc/connect'
import { createGrpcWebTransport } from '@connectrpc/connect-web'

import {
  GetTraceRequestSchema,
  ListTracesRequestSchema,
  TraceService,
  type GetTraceResponse,
  type ListTracesResponse,
} from '@/generated/coral/v1/traces_pb'
import { ExplainSqlRequestSchema, QueryService, type QueryPlan } from '@/generated/coral/v1/query_pb'
import { WorkspaceSchema } from '@/generated/coral/v1/resources_pb'

function grpcWebBaseUrl(): string {
  return import.meta.env.VITE_CORAL_GRPC_WEB_URL ?? window.location.origin
}

const transport = createGrpcWebTransport({
  baseUrl: grpcWebBaseUrl(),
})

const traces = createClient(TraceService, transport)
const queries = createClient(QueryService, transport)
// Local trace summaries do not currently expose workspace metadata, so plan
// recreation uses the Coral default workspace. Queries from other workspaces may
// need a future TraceService field before the UI can recreate their plans.
const defaultWorkspace = create(WorkspaceSchema, { name: 'default' })

export async function listTraces(pageSize = 50, pageToken = ''): Promise<ListTracesResponse> {
  return traces.listTraces(create(ListTracesRequestSchema, { pageSize, pageToken }))
}

export async function getTrace(traceId: string): Promise<GetTraceResponse> {
  return traces.getTrace(create(GetTraceRequestSchema, { traceId }))
}

export async function planSql(sql: string): Promise<QueryPlan | undefined> {
  const response = await queries.explainSql(create(ExplainSqlRequestSchema, {
    workspace: defaultWorkspace,
    sql,
  }))
  return response.plan
}
