// Keep the browser bundle on Arrow's DOM entrypoint; the package also ships
// Node stream/CLI entrypoints that should not be resolved into the UI.
import { tableFromIPC, type Table } from "apache-arrow/Arrow.dom";
import { GrpcWebFetchTransport } from "@protobuf-ts/grpcweb-transport";
import { QueryServiceClient } from "./generated/coral/v1/query.client";
import type { ExecuteSqlResponse } from "./generated/coral/v1/query";
import { TraceServiceClient } from "./generated/coral/v1/traces.client";
import {
  TraceStatus,
  type TraceSpan,
  type TraceSummary,
} from "./generated/coral/v1/traces";
import "./styles.css";

const DEFAULT_SQL = "select * from coral.tables limit 20";
const TRACE_PAGE_SIZE = 40;
const TRACE_REFRESH_DELAY_MS = 450;

type QueryState = "idle" | "running" | "success" | "error";
type TraceState = "idle" | "loading" | "ready" | "error";

type QueryResult = {
  columns: string[];
  rows: Record<string, unknown>[];
  rowCount: number;
};

type JsonObject = Record<string, unknown>;

const transport = new GrpcWebFetchTransport({
  baseUrl: window.location.origin,
  format: "binary",
});

const queryClient = new QueryServiceClient(transport);
const traceClient = new TraceServiceClient(transport);

const app = document.querySelector<HTMLDivElement>("#app");

if (!app) {
  throw new Error("Missing #app root");
}

app.innerHTML = `
  <main class="shell">
    <section class="query-panel" aria-labelledby="query-title">
      <div class="panel-header">
        <div>
          <p class="eyebrow">Coral</p>
          <h1 id="query-title">SQL Console</h1>
        </div>
        <div class="status" data-state="idle" id="query-status">Ready</div>
      </div>

      <form id="query-form" class="query-form">
        <label for="sql-input">SQL</label>
        <textarea id="sql-input" spellcheck="false">${DEFAULT_SQL}</textarea>
        <div class="actions">
          <button id="execute-button" type="submit">Execute</button>
        </div>
      </form>
    </section>

    <section class="workbench" aria-label="Query output and traces">
      <section class="results-panel" aria-labelledby="result-title">
        <div class="result-header">
          <div>
            <p class="eyebrow">Result</p>
            <h2 id="result-title">Rows</h2>
          </div>
          <div class="row-count" id="row-count">0 rows</div>
        </div>
        <div id="error-message" class="error-message" hidden></div>
        <div id="empty-state" class="empty-state">No query has been executed.</div>
        <div id="table-wrap" class="table-wrap" hidden>
          <table>
            <thead id="result-head"></thead>
            <tbody id="result-body"></tbody>
          </table>
        </div>
      </section>

      <section class="traces-panel" aria-labelledby="traces-title">
        <div class="trace-header">
          <div>
            <p class="eyebrow">Observability</p>
            <h2 id="traces-title">Query Traces</h2>
          </div>
          <div class="trace-actions">
            <button id="refresh-traces-button" class="secondary-button" type="button">Refresh</button>
            <div class="status" data-state="idle" id="trace-status">Idle</div>
          </div>
        </div>
        <div class="trace-content">
          <div class="trace-list-column">
            <div id="trace-empty-state" class="empty-state">No query traces captured yet.</div>
            <div id="trace-list" class="trace-list" aria-label="Query trace stream"></div>
          </div>
          <article id="trace-detail" class="trace-detail" aria-label="Selected trace detail">
            <div class="empty-state">Select a trace to inspect its spans.</div>
          </article>
        </div>
      </section>
    </section>
  </main>
`;

const form = requiredElement<HTMLFormElement>("#query-form");
const sqlInput = requiredElement<HTMLTextAreaElement>("#sql-input");
const executeButton = requiredElement<HTMLButtonElement>("#execute-button");
const statusElement = requiredElement<HTMLDivElement>("#query-status");
const rowCountElement = requiredElement<HTMLDivElement>("#row-count");
const errorElement = requiredElement<HTMLDivElement>("#error-message");
const emptyElement = requiredElement<HTMLDivElement>("#empty-state");
const tableWrap = requiredElement<HTMLDivElement>("#table-wrap");
const tableHead = requiredElement<HTMLTableSectionElement>("#result-head");
const tableBody = requiredElement<HTMLTableSectionElement>("#result-body");
const traceStatusElement = requiredElement<HTMLDivElement>("#trace-status");
const refreshTracesButton = requiredElement<HTMLButtonElement>(
  "#refresh-traces-button",
);
const traceEmptyElement = requiredElement<HTMLDivElement>("#trace-empty-state");
const traceListElement = requiredElement<HTMLDivElement>("#trace-list");
const traceDetailElement = requiredElement<HTMLElement>("#trace-detail");

let traceSummaries: TraceSummary[] = [];
let selectedTraceId = "";
let pendingTraceRefresh: number | undefined;
let traceLoadRequestId = 0;

form.addEventListener("submit", (event) => {
  event.preventDefault();
  void executeSql(sqlInput.value);
});

refreshTracesButton.addEventListener("click", () => {
  void loadTraces({ keepSelection: true });
});

void loadTraces({ keepSelection: false });

function requiredElement<T extends Element>(selector: string): T {
  const element = document.querySelector<T>(selector);
  if (!element) {
    throw new Error(`Missing required element: ${selector}`);
  }
  return element;
}

async function executeSql(sql: string): Promise<void> {
  const statement = sql.trim();
  if (!statement) {
    renderError("Enter a SQL statement.");
    return;
  }

  setQueryState("running", "Running");
  executeButton.disabled = true;

  try {
    const response = await executeSqlRequest(statement);
    const result = decodeQueryResult(response);
    renderResult(result);
    setQueryState("success", "Complete");
  } catch (error) {
    renderError(error instanceof Error ? error.message : String(error));
    setQueryState("error", "Error");
  } finally {
    executeButton.disabled = false;
    scheduleTraceRefresh();
  }
}

async function executeSqlRequest(sql: string): Promise<ExecuteSqlResponse> {
  const { response } = await queryClient.executeSql({
    workspace: { name: "default" },
    sql,
  });
  return response;
}

function decodeQueryResult(response: ExecuteSqlResponse): QueryResult {
  const table = tableFromIPC(response.arrowIpcStream);
  const columns = table.schema.fields.map((field) => field.name);
  const rows = tableRows(table, columns);
  return {
    columns,
    rows,
    rowCount: response.rowCount,
  };
}

function tableRows(
  table: Table,
  columns: string[],
): Record<string, unknown>[] {
  return table.toArray().map((row) => {
    const json = row.toJSON() as Record<string, unknown>;
    return Object.fromEntries(columns.map((column) => [column, json[column]]));
  });
}

function renderResult(result: QueryResult): void {
  errorElement.hidden = true;
  errorElement.textContent = "";
  rowCountElement.textContent = formatRowCount(result.rowCount);

  tableHead.replaceChildren();
  tableBody.replaceChildren();

  if (result.columns.length === 0) {
    tableWrap.hidden = true;
    emptyElement.hidden = false;
    emptyElement.textContent = "The query returned no columns.";
    return;
  }

  const headerRow = document.createElement("tr");
  for (const column of result.columns) {
    const th = document.createElement("th");
    th.scope = "col";
    th.textContent = column;
    headerRow.append(th);
  }
  tableHead.append(headerRow);

  for (const row of result.rows) {
    const tableRow = document.createElement("tr");
    for (const column of result.columns) {
      const cell = document.createElement("td");
      const value = row[column];
      cell.textContent = formatCellValue(value);
      if (value === null || value === undefined) {
        cell.classList.add("null-cell");
      }
      tableRow.append(cell);
    }
    tableBody.append(tableRow);
  }

  if (result.rows.length === 0) {
    const tableRow = document.createElement("tr");
    const cell = document.createElement("td");
    cell.colSpan = result.columns.length;
    cell.textContent = "No rows returned.";
    cell.className = "empty-table-cell";
    tableRow.append(cell);
    tableBody.append(tableRow);
  }

  emptyElement.hidden = true;
  tableWrap.hidden = false;
}

function renderError(message: string): void {
  tableWrap.hidden = true;
  emptyElement.hidden = true;
  rowCountElement.textContent = "0 rows";
  errorElement.hidden = false;
  errorElement.textContent = message;
}

function setQueryState(state: QueryState, label: string): void {
  statusElement.dataset.state = state;
  statusElement.textContent = label;
}

function scheduleTraceRefresh(): void {
  if (pendingTraceRefresh !== undefined) {
    window.clearTimeout(pendingTraceRefresh);
  }
  pendingTraceRefresh = window.setTimeout(() => {
    pendingTraceRefresh = undefined;
    void loadTraces({ keepSelection: false });
  }, TRACE_REFRESH_DELAY_MS);
}

async function loadTraces({
  keepSelection,
}: {
  keepSelection: boolean;
}): Promise<void> {
  const requestId = ++traceLoadRequestId;
  setTraceState("loading", "Loading");
  refreshTracesButton.disabled = true;

  try {
    const { response } = await traceClient.listTraces({
      pageSize: TRACE_PAGE_SIZE,
      pageToken: "",
    });
    if (requestId !== traceLoadRequestId) {
      return;
    }

    traceSummaries = response.traces;
    const nextSelection =
      keepSelection &&
      traceSummaries.some((trace) => trace.traceId === selectedTraceId)
        ? selectedTraceId
        : traceSummaries[0]?.traceId ?? "";

    selectedTraceId = nextSelection;
    renderTraceList();

    if (nextSelection) {
      void selectTrace(nextSelection);
    } else {
      renderTraceEmpty("No query traces captured yet.");
    }

    setTraceState(
      "ready",
      traceSummaries.length === 0
        ? "No traces"
        : `${traceSummaries.length.toLocaleString()} traces`,
    );
  } catch (error) {
    if (requestId !== traceLoadRequestId) {
      return;
    }
    renderTraceError(error instanceof Error ? error.message : String(error));
    setTraceState("error", "Error");
  } finally {
    if (requestId === traceLoadRequestId) {
      refreshTracesButton.disabled = false;
    }
  }
}

function renderTraceList(): void {
  traceListElement.replaceChildren();
  traceEmptyElement.hidden = traceSummaries.length > 0;

  for (const trace of traceSummaries) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = "trace-row";
    button.setAttribute(
      "aria-selected",
      trace.traceId === selectedTraceId ? "true" : "false",
    );

    const top = document.createElement("span");
    top.className = "trace-row-top";

    const title = document.createElement("span");
    title.className = "trace-query";
    title.textContent = trace.query || trace.name || "Query trace";
    top.append(title, statusBadge(trace.status));

    const meta = document.createElement("span");
    meta.className = "trace-meta";
    meta.textContent = traceMetaText(trace);

    const id = document.createElement("span");
    id.className = "trace-id";
    id.textContent = shortTraceId(trace.traceId);

    button.append(top, meta, id);
    button.addEventListener("click", () => {
      void selectTrace(trace.traceId);
    });
    traceListElement.append(button);
  }
}

async function selectTrace(traceId: string): Promise<void> {
  selectedTraceId = traceId;
  renderTraceList();
  renderTraceDetailLoading();

  try {
    const { response } = await traceClient.getTrace({ traceId });
    if (selectedTraceId !== traceId) {
      return;
    }
    renderTraceDetail(response.summary, response.spans);
  } catch (error) {
    if (selectedTraceId !== traceId) {
      return;
    }
    renderTraceError(error instanceof Error ? error.message : String(error));
  }
}

function renderTraceDetailLoading(): void {
  traceDetailElement.replaceChildren(emptyMessage("Loading trace detail."));
}

function renderTraceEmpty(message: string): void {
  traceDetailElement.replaceChildren(emptyMessage(message));
}

function renderTraceError(message: string): void {
  const error = document.createElement("div");
  error.className = "error-message";
  error.textContent = message;
  traceDetailElement.replaceChildren(error);
}

function renderTraceDetail(
  summary: TraceSummary | undefined,
  spans: TraceSpan[],
): void {
  if (!summary) {
    renderTraceEmpty("Trace detail did not include a summary.");
    return;
  }

  const header = document.createElement("div");
  header.className = "trace-detail-header";

  const titleGroup = document.createElement("div");
  const eyebrow = document.createElement("p");
  eyebrow.className = "eyebrow";
  eyebrow.textContent = shortTraceId(summary.traceId);
  const title = document.createElement("h3");
  title.textContent = summary.query || summary.name || "Query trace";
  titleGroup.append(eyebrow, title);
  header.append(titleGroup, statusBadge(summary.status));

  const query = document.createElement("pre");
  query.className = "query-snippet";
  query.textContent = summary.query || summary.name || "No query text recorded.";

  const meta = document.createElement("dl");
  meta.className = "detail-meta";
  meta.append(
    metaItem("Started", formatTimestamp(summary.startTimeUnixNanos)),
    metaItem("Duration", formatDuration(summary.durationNanos)),
    metaItem("Spans", summary.spanCount.toLocaleString()),
    metaItem("Rows", formatOptionalRowCount(summary)),
  );

  const sources = sourcesFromSpans(spans);
  const sourceList = document.createElement("div");
  sourceList.className = "source-list";
  sourceList.append(sectionLabel("Sources"));
  if (sources.length === 0) {
    const empty = document.createElement("span");
    empty.className = "muted";
    empty.textContent = "No source calls recorded.";
    sourceList.append(empty);
  } else {
    for (const source of sources) {
      const sourceChip = document.createElement("span");
      sourceChip.className = "attribute-chip";
      sourceChip.textContent = source;
      sourceList.append(sourceChip);
    }
  }

  const timeline = document.createElement("div");
  timeline.className = "span-timeline";
  timeline.append(sectionLabel("Timeline"));
  for (const span of spans) {
    timeline.append(spanRow(span, spans));
  }

  traceDetailElement.replaceChildren(header, query, meta, sourceList, timeline);
}

function spanRow(span: TraceSpan, spans: TraceSpan[]): HTMLElement {
  const start = minNanos(spans.map((item) => item.startTimeUnixNanos));
  const end = maxNanos(spans.map((item) => item.endTimeUnixNanos));
  const spanStart = parseNanos(span.startTimeUnixNanos);
  const total = maxBigInt(1n, end - start);
  const offsetPercent = clamp(
    ratioPercent(spanStart - start, total),
    0,
    100,
  );
  const widthPercent = clamp(
    ratioPercent(parseNanos(span.durationNanos), total),
    1,
    100,
  );
  const attributes = parseJsonObject(span.attributesJson);

  const row = document.createElement("article");
  row.className = `span-row span-row-${statusClass(span.status)}`;

  const header = document.createElement("div");
  header.className = "span-row-header";
  const title = document.createElement("div");
  title.className = "span-title";
  title.textContent = span.name || "span";
  const meta = document.createElement("div");
  meta.className = "span-meta";
  meta.textContent = `${spanCategory(span, attributes)} | ${formatDuration(
    span.durationNanos,
  )} | ${formatTimestamp(span.startTimeUnixNanos)}`;
  header.append(title, meta);

  const track = document.createElement("div");
  track.className = "span-track";
  const bar = document.createElement("div");
  bar.className = "span-bar";
  bar.style.marginLeft = `${offsetPercent}%`;
  bar.style.width = `${widthPercent}%`;
  track.append(bar);

  const chips = document.createElement("div");
  chips.className = "attribute-chips";
  for (const chip of spanChips(span, attributes)) {
    chips.append(chip);
  }

  const jsonDetails = document.createElement("details");
  jsonDetails.className = "span-json";
  const summary = document.createElement("summary");
  summary.textContent = "Attributes";
  const pre = document.createElement("pre");
  pre.textContent = prettyJson(span.attributesJson);
  jsonDetails.append(summary, pre);

  row.append(header, track);
  if (chips.childElementCount > 0) {
    row.append(chips);
  }
  row.append(jsonDetails);
  return row;
}

function spanChips(span: TraceSpan, attributes: JsonObject): HTMLElement[] {
  const keys = [
    "workspace",
    "coral.source",
    "coral.table",
    "http.request.method",
    "http.response.status_code",
    "url.full",
    "row_count",
    "status",
  ];
  const chips = keys
    .map((key) => {
      const value = attributes[key];
      return value === undefined ? undefined : attributeChip(key, value);
    })
    .filter((chip): chip is HTMLElement => chip !== undefined);

  if (span.statusMessage) {
    chips.push(attributeChip("error", span.statusMessage));
  }
  return chips;
}

function attributeChip(key: string, value: unknown): HTMLElement {
  const chip = document.createElement("span");
  chip.className = "attribute-chip";
  chip.textContent = `${key}: ${attributeValue(value)}`;
  return chip;
}

function metaItem(label: string, value: string): HTMLElement {
  const item = document.createElement("div");
  const dt = document.createElement("dt");
  dt.textContent = label;
  const dd = document.createElement("dd");
  dd.textContent = value;
  item.append(dt, dd);
  return item;
}

function sectionLabel(text: string): HTMLElement {
  const label = document.createElement("div");
  label.className = "section-label";
  label.textContent = text;
  return label;
}

function emptyMessage(message: string): HTMLElement {
  const empty = document.createElement("div");
  empty.className = "empty-state";
  empty.textContent = message;
  return empty;
}

function setTraceState(state: TraceState, label: string): void {
  traceStatusElement.dataset.state = state;
  traceStatusElement.textContent = label;
}

function traceMetaText(trace: TraceSummary): string {
  const parts = [
    formatTimestamp(trace.startTimeUnixNanos),
    formatDuration(trace.durationNanos),
    `${trace.spanCount.toLocaleString()} spans`,
  ];
  if (trace.rowCountRecorded) {
    parts.push(formatRowCount(trace.rowCount));
  }
  return parts.join(" | ");
}

function sourcesFromSpans(spans: TraceSpan[]): string[] {
  const sources = new Set<string>();
  for (const span of spans) {
    const attributes = parseJsonObject(span.attributesJson);
    const source = stringAttribute(attributes, "coral.source");
    const table = stringAttribute(attributes, "coral.table");
    if (source && table) {
      sources.add(`${source}.${table}`);
    } else if (source) {
      sources.add(source);
    }
  }
  return [...sources].sort((left, right) => left.localeCompare(right));
}

function spanCategory(span: TraceSpan, attributes: JsonObject): string {
  if (span.name === "coral.query") {
    return "query";
  }
  if (stringAttribute(attributes, "coral.source")) {
    return "retrieval";
  }
  if (span.name.startsWith("http.")) {
    return "api call";
  }
  if (span.name.toLowerCase().includes("datafusion")) {
    return "engine";
  }
  return span.kind || "system";
}

function statusBadge(status: TraceStatus): HTMLElement {
  const badge = document.createElement("span");
  badge.className = `status-badge status-badge-${statusClass(status)}`;
  badge.textContent = formatStatus(status);
  return badge;
}

function statusClass(status: TraceStatus): string {
  switch (status) {
    case TraceStatus.OK:
      return "ok";
    case TraceStatus.ERROR:
      return "error";
    default:
      return "unknown";
  }
}

function formatStatus(status: TraceStatus): string {
  switch (status) {
    case TraceStatus.OK:
      return "ok";
    case TraceStatus.ERROR:
      return "error";
    default:
      return "unknown";
  }
}

function formatRowCount(count: number): string {
  return `${count.toLocaleString()} ${count === 1 ? "row" : "rows"}`;
}

function formatOptionalRowCount(summary: TraceSummary): string {
  return summary.rowCountRecorded ? formatRowCount(summary.rowCount) : "not recorded";
}

function formatCellValue(value: unknown): string {
  if (value === null || value === undefined) {
    return "NULL";
  }
  if (value instanceof Date) {
    return value.toISOString();
  }
  if (typeof value === "bigint") {
    return value.toString();
  }
  if (typeof value === "object") {
    return JSON.stringify(value);
  }
  return String(value);
}

function formatDuration(nanosText: string): string {
  const nanos = parseNanos(nanosText);
  if (nanos <= 0n) {
    return "0 ms";
  }
  const millis = safeNumber(nanos) / 1_000_000;
  if (millis < 1) {
    const micros = safeNumber(nanos / 1_000n);
    return `${Math.max(1, Math.round(micros)).toLocaleString()} us`;
  }
  if (millis < 1000) {
    return `${millis.toFixed(millis < 10 ? 2 : 1)} ms`;
  }
  return `${(millis / 1000).toFixed(2)} s`;
}

function formatTimestamp(nanosText: string): string {
  const nanos = parseNanos(nanosText);
  if (nanos <= 0n) {
    return "unknown";
  }
  return new Date(safeNumber(nanos / 1_000_000n)).toLocaleString([], {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
    second: "2-digit",
  });
}

function shortTraceId(traceId: string): string {
  if (traceId.length <= 16) {
    return traceId;
  }
  return `${traceId.slice(0, 8)}...${traceId.slice(-8)}`;
}

function parseJsonObject(jsonText: string): JsonObject {
  try {
    const value = JSON.parse(jsonText) as unknown;
    if (value && typeof value === "object" && !Array.isArray(value)) {
      return value as JsonObject;
    }
  } catch {
    return {};
  }
  return {};
}

function prettyJson(jsonText: string): string {
  try {
    return JSON.stringify(JSON.parse(jsonText), null, 2);
  } catch {
    return jsonText || "{}";
  }
}

function stringAttribute(attributes: JsonObject, key: string): string {
  const value = attributes[key];
  return typeof value === "string" ? value : "";
}

function attributeValue(value: unknown): string {
  if (value === null || value === undefined) {
    return "null";
  }
  if (typeof value === "object") {
    return JSON.stringify(value);
  }
  return String(value);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(max, Math.max(min, value));
}

function parseNanos(value: string): bigint {
  try {
    return BigInt(value || "0");
  } catch {
    return 0n;
  }
}

function minNanos(values: string[]): bigint {
  return values.map(parseNanos).reduce((min, value) => {
    return value < min ? value : min;
  }, parseNanos(values[0] ?? "0"));
}

function maxNanos(values: string[]): bigint {
  return values.map(parseNanos).reduce((max, value) => {
    return value > max ? value : max;
  }, parseNanos(values[0] ?? "0"));
}

function maxBigInt(left: bigint, right: bigint): bigint {
  return left > right ? left : right;
}

function ratioPercent(value: bigint, total: bigint): number {
  if (value <= 0n || total <= 0n) {
    return 0;
  }
  return Number((value * 10_000n) / total) / 100;
}

function safeNumber(value: bigint): number {
  const max = BigInt(Number.MAX_SAFE_INTEGER);
  if (value > max) {
    return Number.MAX_SAFE_INTEGER;
  }
  if (value < -max) {
    return -Number.MAX_SAFE_INTEGER;
  }
  return Number(value);
}
