import { useEffect, useMemo, useState } from 'react'
import classnames from 'classnames'

import { Icon } from '@/wax/components/icon'
import { TextInput } from '@/wax/components/inputs/text'
import { Skeleton } from '@/wax/components/skeleton'
import { Typography } from '@/wax/components/typography'

import { PageHeader } from '@/components/page-header'
import { ErrorBanner } from '@/components/error-banner'
import { providerIcon } from '@/lib/provider-icons'
import { useRouter } from '@/lib/router'
import {
  discoverBundled,
  discoverCommunity,
  listInstalledSources,
  type CatalogEntry,
  type InstalledSource,
} from '@/lib/sources'

import * as styles from './sources-index.css'

type Facet = 'all' | 'core' | 'community' | 'installed'

export function SourcesIndex() {
  const { navigate } = useRouter()
  const [installed, setInstalled] = useState<InstalledSource[] | null>(null)
  const [bundled, setBundled] = useState<CatalogEntry[] | null>(null)
  const [community, setCommunity] = useState<CatalogEntry[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [showSkeleton, setShowSkeleton] = useState(false)

  useEffect(() => {
    let cancelled = false
    const skel = window.setTimeout(() => {
      if (!cancelled && installed === null) setShowSkeleton(true)
    }, 120)

    async function load() {
      try {
        const [installedRes, bundledRes, communityRes] = await Promise.all([
          listInstalledSources(),
          discoverBundled(),
          discoverCommunity(),
        ])
        if (cancelled) return
        installedRes.sort((a, b) => a.name.localeCompare(b.name))
        setInstalled(installedRes)
        setBundled(bundledRes)
        setCommunity(communityRes)
      } catch (e) {
        if (cancelled) return
        setError(e instanceof Error ? e.message : String(e))
      }
    }

    void load()
    return () => {
      cancelled = true
      window.clearTimeout(skel)
    }
  }, [installed])

  const catalog = useMemo<CatalogEntry[]>(
    () => [...(bundled ?? []), ...(community ?? [])],
    [bundled, community],
  )

  const installedNames = useMemo(
    () => new Set((installed ?? []).map((s) => s.name)),
    [installed],
  )

  return (
    <div className={styles.root}>
      <PageHeader title="Sources" subtitle="Data sources available in this workspace." />

      <div className={styles.body}>
        {error ? (
          <ErrorBanner
            title="Couldn't load sources"
            message={error}
            onRetry={() => window.location.reload()}
          />
        ) : null}

        <ConnectedSection
          loading={installed === null && !error}
          showSkeleton={showSkeleton}
          sources={installed ?? []}
          onOpen={(name) => navigate({ route: { kind: 'source-detail', name } })}
        />

        <LibrarySection
          loading={catalog.length === 0 && bundled === null && community === null && !error}
          entries={catalog}
          installedNames={installedNames}
          onInstall={(entry) =>
            navigate({
              route: { kind: 'source-install', name: entry.name, origin: entry.origin },
            })
          }
          onOpenInstalled={(name) => navigate({ route: { kind: 'source-detail', name } })}
        />
      </div>
    </div>
  )
}

function ConnectedSection({
  loading,
  showSkeleton,
  sources,
  onOpen,
}: {
  loading: boolean
  showSkeleton: boolean
  sources: InstalledSource[]
  onOpen: (name: string) => void
}) {
  return (
    <section className={styles.section}>
      <SectionHead title="Connected" count={sources.length} />
      {loading ? (
        showSkeleton ? <SkeletonGrid /> : null
      ) : sources.length === 0 ? (
        <EmptyConnected />
      ) : (
        <div className={styles.grid}>
          {sources.map((source) => (
            <ConnectedCard key={source.name} source={source} onOpen={() => onOpen(source.name)} />
          ))}
        </div>
      )}
    </section>
  )
}

function ConnectedCard({ source, onOpen }: { source: InstalledSource; onOpen: () => void }) {
  const icon = providerIcon(source.name)
  return (
    <div className={styles.cardWrap}>
      <button type="button" className={styles.card} onClick={onOpen}>
        <div className={styles.cardHeader}>
          <div className={styles.iconBox}>
            {icon ? (
              <img src={icon} alt="" className={styles.providerIcon} />
            ) : (
              <Icon name="Plug" size="20" color="secondary" />
            )}
          </div>
          <div className={styles.tagStack}>
            <span className={styles.originTag}>{originLabel(source.origin)}</span>
          </div>
        </div>
        <div className={styles.cardBody}>
          <Typography.BodyLargeStrong as="span">{source.name}</Typography.BodyLargeStrong>
          <Typography.BodySmall variant="tertiary">v{source.version || '—'}</Typography.BodySmall>
        </div>
      </button>
    </div>
  )
}

function EmptyConnected() {
  return (
    <div className={styles.empty}>
      <div className={styles.emptyPlus}>
        <Icon name="Plus" size="22" color="tertiary" />
      </div>
      <Typography.Body variant="primary">No sources yet</Typography.Body>
      <Typography.BodySmall variant="tertiary">
        Pick a source from the library below to get started.
      </Typography.BodySmall>
    </div>
  )
}

function LibrarySection({
  loading,
  entries,
  installedNames,
  onInstall,
  onOpenInstalled,
}: {
  loading: boolean
  entries: CatalogEntry[]
  installedNames: Set<string>
  onInstall: (entry: CatalogEntry) => void
  onOpenInstalled: (name: string) => void
}) {
  const [facet, setFacet] = useState<Facet>('all')
  const [search, setSearch] = useState('')

  const counts = useMemo(() => {
    const c = { all: entries.length, core: 0, community: 0, installed: 0 }
    for (const e of entries) {
      if (e.origin === 'bundled') c.core += 1
      else c.community += 1
      if (e.installed || installedNames.has(e.name)) c.installed += 1
    }
    return c
  }, [entries, installedNames])

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase()
    return entries.filter((e) => {
      if (facet === 'core' && e.origin !== 'bundled') return false
      if (facet === 'community' && e.origin !== 'community') return false
      if (facet === 'installed' && !(e.installed || installedNames.has(e.name))) return false
      if (q && !e.name.toLowerCase().includes(q) && !e.description.toLowerCase().includes(q)) {
        return false
      }
      return true
    })
  }, [entries, facet, search, installedNames])

  return (
    <section className={styles.section}>
      <SectionHead title="Library" count={entries.length} secondary="Sources you can install." />

      <div className={styles.libraryToolbar}>
        <div className={styles.libraryFilters}>
          <FacetChip label="All" active={facet === 'all'} count={counts.all} onClick={() => setFacet('all')} />
          <FacetChip label="Core" active={facet === 'core'} count={counts.core} onClick={() => setFacet('core')} />
          <FacetChip
            label="Community"
            active={facet === 'community'}
            count={counts.community}
            onClick={() => setFacet('community')}
          />
          <span className={styles.facetSep} />
          <FacetChip
            label="Installed"
            active={facet === 'installed'}
            count={counts.installed}
            onClick={() => setFacet('installed')}
          />
        </div>
        <div className={styles.librarySearch}>
          <TextInput value={search} onChange={setSearch} placeholder="Search the library…" icon="Search" />
        </div>
      </div>

      {loading ? (
        <div className={styles.libraryGrid}>
          {Array.from({ length: 8 }).map((_, i) => (
            <div key={i} className={styles.tile} style={{ cursor: 'default' }}>
              <Skeleton width={32} height={32} borderRadius={8} />
              <Skeleton width={100} height={14} borderRadius={4} />
              <Skeleton width={140} height={12} borderRadius={4} />
            </div>
          ))}
        </div>
      ) : filtered.length === 0 ? (
        <div className={styles.thinNotice}>
          <Typography.BodySmall variant="tertiary">No sources match.</Typography.BodySmall>
        </div>
      ) : (
        <div className={styles.libraryGrid}>
          {filtered.map((entry) => (
            <LibraryTile
              key={`${entry.origin}:${entry.name}`}
              entry={entry}
              installed={entry.installed || installedNames.has(entry.name)}
              onClick={() => {
                if (entry.installed || installedNames.has(entry.name)) onOpenInstalled(entry.name)
                else onInstall(entry)
              }}
            />
          ))}
        </div>
      )}
    </section>
  )
}

function FacetChip({
  label,
  count,
  active,
  onClick,
}: {
  label: string
  count: number
  active: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      className={classnames(styles.facetChip, { [styles.facetChipActive]: active })}
      onClick={onClick}
    >
      {label}
      <span className={styles.facetCount}>{count}</span>
    </button>
  )
}

function LibraryTile({
  entry,
  installed,
  onClick,
}: {
  entry: CatalogEntry
  installed: boolean
  onClick: () => void
}) {
  const icon = providerIcon(entry.name)
  return (
    <button type="button" className={styles.tile} onClick={onClick}>
      <div className={styles.tileHeader}>
        <div className={styles.tileIcon}>
          {icon ? (
            <img src={icon} alt="" className={styles.tileIconImg} />
          ) : (
            <Icon name="Plug" size="18" color="secondary" />
          )}
        </div>
        <span className={styles.smallPill}>{entry.origin === 'bundled' ? 'Core' : 'Community'}</span>
      </div>
      <div className={styles.tileBody}>
        <Typography.BodyStrong as="span">{entry.name}</Typography.BodyStrong>
        <Typography.BodySmall variant="tertiary" className={styles.tileDesc}>
          {entry.description}
        </Typography.BodySmall>
      </div>
      <div className={styles.tileFooter}>
        {installed ? (
          <span className={styles.installedHint}>
            <Icon name="Check" size="14" color="success" /> Connected
          </span>
        ) : (
          <span className={styles.tileAddHint}>
            <Icon name="Plus" size="14" color="secondary" /> Install
          </span>
        )}
      </div>
    </button>
  )
}

function SectionHead({
  title,
  count,
  secondary,
}: {
  title: string
  count?: number
  secondary?: string
}) {
  return (
    <div className={styles.sectionHead}>
      <div className={styles.sectionHeadLeft}>
        <Typography.HeadingXSmall as="h2">{title}</Typography.HeadingXSmall>
        {typeof count === 'number' ? <span className={styles.sectionCount}>{count}</span> : null}
        {secondary ? (
          <Typography.BodySmall variant="tertiary" className={styles.sectionSecondary}>
            · {secondary}
          </Typography.BodySmall>
        ) : null}
      </div>
    </div>
  )
}

function SkeletonGrid() {
  return (
    <div className={styles.grid}>
      {Array.from({ length: 4 }).map((_, i) => (
        <div key={i} className={styles.card} style={{ cursor: 'default' }}>
          <div className={styles.cardHeader}>
            <Skeleton width={36} height={36} borderRadius={8} />
            <Skeleton width={56} height={14} borderRadius={4} />
          </div>
          <div className={styles.cardBody}>
            <Skeleton width={120} height={20} borderRadius={4} />
            <Skeleton width={80} height={14} borderRadius={4} />
          </div>
        </div>
      ))}
    </div>
  )
}

function originLabel(origin: InstalledSource['origin']): string {
  if (origin === 'bundled') return 'Core'
  if (origin === 'community') return 'Community'
  if (origin === 'imported') return 'Imported'
  return '—'
}
