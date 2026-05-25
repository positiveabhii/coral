// Topical categories for the sources catalog, modeled on the same layout
// used by adp/web/src/lib/plugin-categories.ts. The mapping is best-effort —
// uncategorized sources land in the "other" bucket.

export interface CategoryDef {
  key: string
  label: string
}

export const CATEGORY_ORDER: CategoryDef[] = [
  { key: 'observability', label: 'Observability' },
  { key: 'developer-tools', label: 'Developer Tools' },
  { key: 'communication', label: 'Communication' },
  { key: 'project-management', label: 'Project Management' },
  { key: 'analytics', label: 'Analytics' },
  { key: 'ai-ml', label: 'AI & ML' },
  { key: 'databases', label: 'Databases' },
  { key: 'auth-identity', label: 'Auth & Identity' },
  { key: 'payments-commerce', label: 'Payments & Commerce' },
  { key: 'infra', label: 'Infrastructure & Automation' },
  { key: 'content', label: 'Content & Marketing' },
]

const OTHER_CATEGORY: CategoryDef = { key: 'other', label: 'Other' }

const SOURCE_CATEGORY: Record<string, string> = {
  // Observability
  datadog: 'observability',
  grafana: 'observability',
  sentry: 'observability',
  prometheus: 'observability',
  axiom: 'observability',
  honeycomb: 'observability',
  signoz: 'observability',
  influxdb: 'observability',
  coralogix: 'observability',
  splunk: 'observability',
  betterstack: 'observability',
  statusgator: 'observability',

  // Developer Tools
  github: 'developer-tools',
  gitlab: 'developer-tools',
  bitbucket: 'developer-tools',
  codecov: 'developer-tools',
  jenkins: 'developer-tools',
  travis_ci: 'developer-tools',
  vercel: 'developer-tools',
  netlify: 'developer-tools',
  fly: 'developer-tools',
  cloudflare: 'developer-tools',
  digitalocean: 'developer-tools',
  postman: 'developer-tools',
  crates_io: 'developer-tools',
  osv: 'developer-tools',

  // Communication
  slack: 'communication',
  mailgun: 'communication',
  sendgrid: 'communication',
  resend: 'communication',
  postmark: 'communication',
  zulip: 'communication',
  loops: 'communication',

  // Project Management
  linear: 'project-management',
  cal: 'project-management',
  todoist: 'project-management',
  launchdarkly: 'project-management',
  opsgenie: 'project-management',

  // Analytics
  amplitude: 'analytics',
  posthog: 'analytics',
  plausible: 'analytics',
  mixpanel: 'analytics',
  umami_cloud: 'analytics',
  beehiiv: 'analytics',
  langfuse: 'analytics',
  langsmith: 'analytics',

  // AI / ML
  openai: 'ai-ml',
  anthropic: 'ai-ml',
  groq_ai: 'ai-ml',
  huggingface: 'ai-ml',
  pinecone: 'ai-ml',
  weaviate: 'ai-ml',
  qdrant_cloud: 'ai-ml',
  milvus: 'ai-ml',
  clear_ml: 'ai-ml',

  // Databases
  clickhouse: 'databases',
  clickhouse_mcp: 'databases',
  neondb: 'databases',
  neo4j: 'databases',
  turso: 'databases',
  upstash: 'databases',
  databricks: 'databases',
  datahub: 'databases',
  dbt_cloud: 'databases',
  kafka: 'databases',
  rabbitmq: 'databases',
  elasticsearch: 'databases',
  trino: 'databases',

  // Auth & Identity
  auth0: 'auth-identity',
  clerk: 'auth-identity',
  keycloak: 'auth-identity',
  okta: 'auth-identity',

  // Payments & Commerce
  stripe: 'payments-commerce',
  shopify: 'payments-commerce',
  woocommerce: 'payments-commerce',
  dub: 'payments-commerce',
  notion: 'project-management',

  // Infrastructure & Automation
  k8s: 'infra',
  airflow: 'infra',
  n8n: 'infra',
  kestra: 'infra',
  prefect: 'infra',
  novu: 'infra',
  tailscale: 'infra',

  // Content & Marketing
  ghost: 'content',
  hubspot: 'content',
  figma: 'content',
  hn: 'content',
  remotive: 'content',
  google_drive: 'content',
  mcp: 'developer-tools',
}

export function getCategoryForSource(name: string): string {
  return SOURCE_CATEGORY[name.toLowerCase()] ?? 'other'
}

export function categoriseSources<T extends { name: string }>(
  entries: T[],
): { category: CategoryDef; entries: T[] }[] {
  const groups: Record<string, T[]> = {}
  for (const entry of entries) {
    const cat = getCategoryForSource(entry.name)
    if (!groups[cat]) groups[cat] = []
    groups[cat].push(entry)
  }
  const sections = CATEGORY_ORDER.filter((cat) => groups[cat.key]?.length).map((cat) => ({
    category: cat,
    entries: groups[cat.key] ?? [],
  }))
  if (groups[OTHER_CATEGORY.key]?.length) {
    sections.push({ category: OTHER_CATEGORY, entries: groups[OTHER_CATEGORY.key] ?? [] })
  }
  return sections
}
