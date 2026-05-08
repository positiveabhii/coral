import { useEffect, useRef, useState } from 'react'

import { Icon } from '@/wax/components/icon'
import { Typography } from '@/wax/components/typography'
import { planSql } from '@/lib/coral-traces-client'
import type { GetTraceResponse } from '@/generated/coral/v1/traces_pb'
import type { QueryPlan } from '@/generated/coral/v1/query_pb'
import { theme } from '@/wax/theme/theme.css'

import * as s from '../traces-page.css'
import type { ExtraDetailTab } from './trace-detail'

type TraceQueryPlan = { plan?: QueryPlan; error?: string }
type PlanLoadState = { loading: boolean; result: TraceQueryPlan | null }

function useTraceQueryPlan(sql: string | undefined, enabled: boolean): PlanLoadState {
  const [queryPlan, setQueryPlan] = useState<TraceQueryPlan | null>(null)
  const [loading, setLoading] = useState(false)
  const requestedSqlRef = useRef<string | null>(null)

  useEffect(() => {
    requestedSqlRef.current = null
    setQueryPlan(null)
    setLoading(false)
  }, [sql])

  useEffect(() => {
    if (!sql || !enabled || requestedSqlRef.current === sql) return

    let stale = false
    requestedSqlRef.current = sql
    setLoading(true)
    planSql(sql)
      .then((plan) => { if (!stale) setQueryPlan(plan ? { plan } : {}) })
      .catch((err) => { if (!stale) setQueryPlan({ error: err instanceof Error ? err.message : String(err) }) })
      .finally(() => { if (!stale) setLoading(false) })
    return () => { stale = true }
  }, [enabled, sql])

  return { loading, result: queryPlan }
}

interface PlanNode {
  id: string
  label: string
  schema?: string
}

interface PlanEdge {
  from: string
  to: string
}

interface PlanGraph {
  title: string
  nodes: PlanNode[]
  edges: PlanEdge[]
}

interface PlanRenderNode {
  node: PlanNode
  children: PlanRenderNode[]
  isReference?: boolean
  isCycle?: boolean
}

function linesFromPlan(plan: string): string[] {
  return plan.split('\n').filter((line) => line.trim())
}

function depthForPlanLine(line: string): number {
  const marker = line.search(/[A-Za-z]/)
  return Math.max(0, marker < 0 ? 0 : Math.floor(marker / 2))
}

function cleanPlanLabel(line: string): string {
  return line.replace(/^[+|\-\s]*/, '').replace(/^[└├│─\s]*/, '').trim()
}

function schemaFromLabel(label: string): string | undefined {
  const match = label.match(/schema=\[([^\]]+)\]/i) ?? label.match(/Schema:\s*\[?([^\]]+)\]?/i)
  return match?.[1]
}

function parseDotPlan(dot: string): PlanGraph[] {
  const graphs: PlanGraph[] = []
  const subgraphRe = /subgraph\s+\w+\s*\{([^}]*graph\s*\[label="([^"]+)"\][^}]*)}/gs
  let match: RegExpExecArray | null

  while ((match = subgraphRe.exec(dot)) !== null) {
    const graph = dotGraphFromBody(match[1], match[2])
    if (graph.nodes.length > 0) graphs.push(graph)
  }

  if (graphs.length === 0) {
    const graph = dotGraphFromBody(dot, 'Plan')
    if (graph.nodes.length > 0) graphs.push(graph)
  }

  return graphs
}

function dotGraphFromBody(body: string, title: string): PlanGraph {
  const nodes: PlanNode[] = []
  const edges: PlanEdge[] = []
  const nodeRe = /(\d+)\s*\[.*?label="([^"]+)".*?\]/g
  let nodeMatch: RegExpExecArray | null
  while ((nodeMatch = nodeRe.exec(body)) !== null) {
    const labelParts = nodeMatch[2].split('\\n')
    nodes.push({ id: nodeMatch[1], label: labelParts[0], schema: labelParts[1] })
  }

  const edgeRe = /(\d+)\s*->\s*(\d+)/g
  let edgeMatch: RegExpExecArray | null
  while ((edgeMatch = edgeRe.exec(body)) !== null) {
    edges.push({ from: edgeMatch[1], to: edgeMatch[2] })
  }

  return { title, nodes, edges }
}

function buildPlanRenderTree(graph: PlanGraph): PlanRenderNode[] {
  const childMap = new Map<string, string[]>()
  const parentSet = new Set<string>()
  for (const edge of graph.edges) {
    if (!childMap.has(edge.from)) childMap.set(edge.from, [])
    childMap.get(edge.from)?.push(edge.to)
    parentSet.add(edge.to)
  }

  const nodeMap = new Map(graph.nodes.map((node) => [node.id, node]))
  const expanded = new Set<string>()

  function build(id: string, ancestors: Set<string>): PlanRenderNode | null {
    const node = nodeMap.get(id)
    if (!node) return null
    if (ancestors.has(id)) return { node, children: [], isCycle: true }
    if (expanded.has(id)) return { node, children: [], isReference: true }

    expanded.add(id)
    const path = new Set(ancestors)
    path.add(id)
    const children = (childMap.get(id) ?? [])
      .map((childId) => build(childId, path))
      .filter((child): child is PlanRenderNode => child !== null)
    return { node, children }
  }

  const roots = graph.nodes.filter((node) => !parentSet.has(node.id))
  if (roots.length === 0 && graph.nodes.length > 0) roots.push(graph.nodes[0])
  return roots.map((root) => build(root.id, new Set())).filter((node): node is PlanRenderNode => node !== null)
}

function isDotPlan(plan: string): boolean {
  return /(?:digraph|subgraph)\s+\w*\s*\{|\d+\s*->\s*\d+/.test(plan)
}

function parseIndentedPlan(plan: string): PlanRenderNode[] {
  const roots: PlanRenderNode[] = []
  const stack: { depth: number; node: PlanRenderNode }[] = []

  linesFromPlan(plan).forEach((line, index) => {
    const label = cleanPlanLabel(line)
    if (!label) return
    const renderNode: PlanRenderNode = {
      node: { id: String(index), label, schema: schemaFromLabel(label) },
      children: [],
    }
    const depth = depthForPlanLine(line)
    while (stack.length > 0 && stack[stack.length - 1].depth >= depth) stack.pop()
    const parent = stack[stack.length - 1]
    if (parent) parent.node.children.push(renderNode)
    else roots.push(renderNode)
    stack.push({ depth, node: renderNode })
  })

  return roots
}

function nodeAccent(label: string): string {
  if (label.startsWith('TableScan') || label.startsWith('DataSource')) return theme.content.success
  if (label.startsWith('Projection')) return theme.content.info
  if (label.startsWith('Filter') || label.startsWith('Hash')) return theme.content.warning
  if (label.startsWith('Limit') || label.startsWith('Sort') || label.startsWith('TopK')) return theme.content.link
  return theme.content.tertiary
}

function shortenSchema(raw: string): string {
  const cols = raw.split(',').map((column) => column.trim().split(':')[0].split('.').pop()?.trim() ?? column.trim())
  if (cols.length <= 6) return cols.join(', ')
  return `${cols.slice(0, 6).join(', ')} +${cols.length - 6} more`
}

function PlanTreeNode({ renderNode, depth }: { renderNode: PlanRenderNode; depth: number }) {
  const accent = nodeAccent(renderNode.node.label)
  const labelParts = renderNode.node.label.split(':')
  const op = labelParts[0].trim()
  const detail = labelParts.slice(1).join(':').trim()
  const hasChildren = renderNode.children.length > 0

  if (renderNode.isReference || renderNode.isCycle) {
    const marker = renderNode.isCycle ? '↺' : '↳'
    const markerDetail = renderNode.isCycle ? '(cycle)' : '(shared subplan)'
    return (
      <div className={s.planTreeRow} style={{ paddingInlineStart: depth * 28, opacity: 0.55 }}>
        {depth > 0 && <div className={s.planTreeArrow} />}
        <div className={s.planTreeNode}>
          <div className={s.planTreeAccent} style={{ backgroundColor: accent }} />
          <div className={s.planTreeContent}>
            <span className={s.planTreeOp} style={{ color: accent }}>{marker} {op}</span>
            <span className={s.planTreeDetail}>{markerDetail}</span>
          </div>
        </div>
      </div>
    )
  }

  return (
    <div>
      <div className={s.planTreeRow} style={{ paddingInlineStart: depth * 28 }}>
        {depth > 0 && <div className={s.planTreeArrow} />}
        <div className={s.planTreeNode}>
          <div className={s.planTreeAccent} style={{ backgroundColor: accent }} />
          <div className={s.planTreeContent}>
            <span className={s.planTreeOp} style={{ color: accent }}>{op}</span>
            {detail && <span className={s.planTreeDetail}>{detail}</span>}
            {renderNode.node.schema && <span className={s.planTreeSchema}>{shortenSchema(renderNode.node.schema)}</span>}
          </div>
        </div>
      </div>
      {hasChildren && (
        <div className={s.planTreeChildren} style={{ marginInlineStart: depth * 28 + 14 }}>
          {renderNode.children.map((child, index) => <PlanTreeNode key={`${child.node.id}-${index}`} renderNode={child} depth={depth + 1} />)}
        </div>
      )}
    </div>
  )
}

function ExecutionPlanView({ label, plan }: { label: string; plan: string }) {
  const trees = isDotPlan(plan)
    ? parseDotPlan(plan).flatMap((graph) => buildPlanRenderTree(graph))
    : parseIndentedPlan(plan)
  return (
    <div className={s.planSection}>
      <div className={s.planHeader}><div className={s.planLabel}><Typography.Body>{label}</Typography.Body></div></div>
      {trees.length > 0 ? (
        <div className={s.planTreeContainer}>{trees.map((tree, index) => <PlanTreeNode key={`${tree.node.id}-${index}`} renderNode={tree} depth={0} />)}</div>
      ) : (
        <pre className={s.planRaw}>{plan}</pre>
      )}
    </div>
  )
}

function PlanLoadingPanel() {
  return (
    <div className={s.emptyPanel}>
      <Icon name="Loader" className={s.spinner} color="tertiary" />
      <Typography.Body variant="tertiary">Recreating query plan…</Typography.Body>
    </div>
  )
}

function PlanPanel({ loading, queryPlan, title, value }: { loading: boolean; queryPlan?: TraceQueryPlan | null; title: string; value: string }) {
  if (loading) return <PlanLoadingPanel />
  if (!queryPlan) return <div className={s.emptyPanel}><Typography.Body variant="tertiary">No query plan is available for this trace.</Typography.Body></div>
  if (queryPlan.error) return <div className={s.errorBox}><Typography.Body variant="error">{queryPlan.error}</Typography.Body></div>
  return <ExecutionPlanView label={title} plan={value || 'Plan was recreated but empty.'} />
}

function LogicalPlanPanel({ loading, queryPlan }: { loading: boolean; queryPlan?: TraceQueryPlan | null }) {
  if (loading) return <PlanLoadingPanel />
  if (!queryPlan) return <div className={s.emptyPanel}><Typography.Body variant="tertiary">No query plan is available for this trace.</Typography.Body></div>
  if (queryPlan.error) return <div className={s.errorBox}><Typography.Body variant="error">{queryPlan.error}</Typography.Body></div>
  const plan = queryPlan.plan
  if (!plan) return <div className={s.emptyPanel}><Typography.Body variant="tertiary">No query plan is available for this trace.</Typography.Body></div>
  return (
    <div className={s.planStack}>
      <ExecutionPlanView label="Logical Plan" plan={plan.unoptimizedLogicalPlan || 'Unoptimized logical plan was recreated but empty.'} />
      <ExecutionPlanView label="Optimized Logical Plan" plan={plan.optimizedLogicalPlan || 'Optimized logical plan was recreated but empty.'} />
    </div>
  )
}

export function useTracePlanTabs(detail: GetTraceResponse | null, activeTab: string): ExtraDetailTab[] {
  const sql = detail?.summary?.query.trim()
  const shouldLoadPlan = activeTab === 'logical' || activeTab === 'physical'
  const { loading, result: queryPlan } = useTraceQueryPlan(sql, shouldLoadPlan)

  if (!sql) return []

  return [
    { id: 'logical', label: 'Logical Plan', content: <LogicalPlanPanel loading={loading} queryPlan={queryPlan} /> },
    { id: 'physical', label: 'Physical Plan', content: <PlanPanel loading={loading} queryPlan={queryPlan} title="Physical Plan" value={queryPlan?.plan?.physicalPlan ?? ''} /> },
  ]
}
