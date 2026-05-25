import { create } from '@bufbuild/protobuf'

import {
  CreateBundledSourceRequestSchema,
  CreateBundledSourceWithOAuthRequestSchema,
  DeleteSourceRequestSchema,
  DiscoverCommunitySourcesRequestSchema,
  DiscoverSourcesRequestSchema,
  GetCommunitySourceInfoRequestSchema,
  GetSourceInfoRequestSchema,
  ImportSourceRequestSchema,
  ListSourcesRequestSchema,
  SourceOrigin,
  ValidateSourceRequestSchema,
  type CreateBundledSourceWithOAuthResponse,
  type ImportSourceResponse,
  type OAuthCredentialRetrieval,
  type Source,
  type SourceInfo,
} from '@/generated/coral/v1/sources_pb'

import { sourceClient, WORKSPACE } from './coral-clients'

export type SourceOriginLabel = 'bundled' | 'imported' | 'community' | 'unknown'

export interface InstalledSource {
  name: string
  version: string
  origin: SourceOriginLabel
}

export interface CatalogEntry {
  name: string
  description: string
  version: string
  installed: boolean
  origin: 'bundled' | 'community'
  hint?: string
}

export interface ResolvedSourceInfo {
  info: SourceInfo
  /** Raw manifest YAML for community sources; null for bundled. */
  manifestYaml: string | null
}

export interface InstallInput {
  key: string
  value: string
  secret: boolean
}

function originLabel(origin: SourceOrigin): SourceOriginLabel {
  if (origin === SourceOrigin.BUNDLED) return 'bundled'
  if (origin === SourceOrigin.IMPORTED) return 'imported'
  if (origin === SourceOrigin.COMMUNITY) return 'community'
  return 'unknown'
}

function toInstalled(s: Source): InstalledSource {
  return { name: s.name, version: s.version, origin: originLabel(s.origin) }
}

export async function listInstalledSources(): Promise<InstalledSource[]> {
  const resp = await sourceClient.listSources(
    create(ListSourcesRequestSchema, { workspace: WORKSPACE }),
  )
  return resp.sources.map(toInstalled)
}

function toCatalogEntry(s: SourceInfo, origin: 'bundled' | 'community'): CatalogEntry {
  return {
    name: s.name,
    description: s.description,
    version: s.version,
    installed: s.installed,
    origin,
  }
}

export async function discoverBundled(): Promise<CatalogEntry[]> {
  const resp = await sourceClient.discoverSources(
    create(DiscoverSourcesRequestSchema, { workspace: WORKSPACE }),
  )
  return resp.sources.map((s) => toCatalogEntry(s, 'bundled'))
}

export async function discoverCommunity(): Promise<CatalogEntry[]> {
  const resp = await sourceClient.discoverCommunitySources(
    create(DiscoverCommunitySourcesRequestSchema, { workspace: WORKSPACE }),
  )
  return resp.sources.map((s) => toCatalogEntry(s, 'community'))
}

export async function getBundledSourceInfo(name: string): Promise<ResolvedSourceInfo> {
  const resp = await sourceClient.getSourceInfo(
    create(GetSourceInfoRequestSchema, { workspace: WORKSPACE, name }),
  )
  if (!resp.sourceInfo) {
    throw new Error(`source '${name}' has no info`)
  }
  return { info: resp.sourceInfo, manifestYaml: null }
}

export async function getCommunitySourceInfo(name: string): Promise<ResolvedSourceInfo> {
  const resp = await sourceClient.getCommunitySourceInfo(
    create(GetCommunitySourceInfoRequestSchema, { workspace: WORKSPACE, name }),
  )
  if (!resp.sourceInfo) {
    throw new Error(`community source '${name}' has no info`)
  }
  return { info: resp.sourceInfo, manifestYaml: resp.manifestYaml }
}

export async function deleteSource(name: string): Promise<void> {
  await sourceClient.deleteSource(
    create(DeleteSourceRequestSchema, { workspace: WORKSPACE, name }),
  )
}

export async function validateSource(name: string) {
  return sourceClient.validateSource(
    create(ValidateSourceRequestSchema, { workspace: WORKSPACE, name }),
  )
}

function splitBindings(inputs: InstallInput[]) {
  const variables = inputs.filter((i) => !i.secret).map((i) => ({ key: i.key, value: i.value }))
  const secrets = inputs.filter((i) => i.secret).map((i) => ({ key: i.key, value: i.value }))
  return { variables, secrets }
}

export async function createBundledSource(
  name: string,
  inputs: InstallInput[],
): Promise<Source> {
  const { variables, secrets } = splitBindings(inputs)
  const resp = await sourceClient.createBundledSource(
    create(CreateBundledSourceRequestSchema, {
      workspace: WORKSPACE,
      name,
      variables,
      secrets,
    }),
  )
  if (!resp.source) throw new Error(`createBundledSource returned no source`)
  return resp.source
}

/** Install a community source by handing the resolved YAML back to ImportSource. */
export async function importCommunitySource(
  manifestYaml: string,
  inputs: InstallInput[],
): Promise<Source> {
  const { variables, secrets } = splitBindings(inputs)
  const stream = sourceClient.importSource(
    create(ImportSourceRequestSchema, {
      workspace: WORKSPACE,
      manifestYaml,
      variables,
      secrets,
    }),
  )
  for await (const response of stream) {
    if (response.event.case === 'source') return response.event.value
  }
  throw new Error(`importSource stream ended without a source event`)
}

export interface OAuthFlowCallbacks {
  onAuthorization?: (event: { inputKey: string; authorizationUrl: string; expiresInSeconds: bigint }) => void
  onCompleted?: (event: { inputKey: string; metadata: Map<string, string> }) => void
}

/** Run the bundled-source OAuth install stream and deliver progress events. */
export async function createBundledSourceWithOAuth(
  name: string,
  inputs: InstallInput[],
  oauthRetrievals: OAuthCredentialRetrieval[],
  callbacks: OAuthFlowCallbacks = {},
): Promise<Source> {
  const { variables, secrets } = splitBindings(inputs)
  const stream = sourceClient.createBundledSourceWithOAuth(
    create(CreateBundledSourceWithOAuthRequestSchema, {
      workspace: WORKSPACE,
      name,
      variables,
      secrets,
      oauthCredentialRetrievals: oauthRetrievals,
    }),
  )
  return handleOAuthStream(stream, callbacks)
}

/** Run the community-source OAuth install stream (via ImportSource). */
export async function importCommunitySourceWithOAuth(
  manifestYaml: string,
  inputs: InstallInput[],
  oauthRetrievals: OAuthCredentialRetrieval[],
  callbacks: OAuthFlowCallbacks = {},
): Promise<Source> {
  const { variables, secrets } = splitBindings(inputs)
  const stream = sourceClient.importSource(
    create(ImportSourceRequestSchema, {
      workspace: WORKSPACE,
      manifestYaml,
      variables,
      secrets,
      oauthCredentialRetrievals: oauthRetrievals,
    }),
  )
  return handleOAuthStream(stream, callbacks)
}

async function handleOAuthStream(
  stream: AsyncIterable<CreateBundledSourceWithOAuthResponse | ImportSourceResponse>,
  callbacks: OAuthFlowCallbacks,
): Promise<Source> {
  for await (const response of stream) {
    const event = response.event
    if (event.case === 'source') return event.value
    if (event.case === 'oauthAuthorization') {
      callbacks.onAuthorization?.({
        inputKey: event.value.inputKey,
        authorizationUrl: event.value.authorizationUrl,
        expiresInSeconds: event.value.expiresInSeconds,
      })
    } else if (event.case === 'oauthCompleted') {
      const metadata = new Map<string, string>()
      for (const item of event.value.metadata) metadata.set(item.key, item.value)
      callbacks.onCompleted?.({ inputKey: event.value.inputKey, metadata })
    }
  }
  throw new Error(`install stream ended without a source event`)
}
