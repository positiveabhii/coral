import { useEffect, useMemo, useState } from 'react'

import { Icon } from '@/wax/components/icon'
import { Typography } from '@/wax/components/typography'

import { ErrorBanner } from '@/components/error-banner'
import { providerIcon } from '@/lib/provider-icons'
import { useRouter } from '@/lib/router'
import { categoriseSources, type CategoryDef } from '@/lib/source-categories'
import {
  discoverBundled,
  discoverCommunity,
  listInstalledSources,
  type CatalogEntry,
  type InstalledSource,
} from '@/lib/sources'

import * as styles from './sources-index.css'

interface IndexEntry extends CatalogEntry {
  installedVersion?: string
}

export function SourcesIndex() {
  const { navigate } = useRouter()
  const [bundled, setBundled] = useState<CatalogEntry[] | null>(null)
  const [community, setCommunity] = useState<CatalogEntry[] | null>(null)
  const [installed, setInstalled] = useState<InstalledSource[] | null>(null)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    async function load() {
      try {
        const [installedRes, bundledRes, communityRes] = await Promise.all([
          listInstalledSources(),
          discoverBundled(),
          discoverCommunity(),
        ])
        if (cancelled) return
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
    }
  }, [])

  const loading = installed === null && bundled === null && community === null && !error

  const entries = useMemo<IndexEntry[]>(() => {
    const installedByName = new Map((installed ?? []).map((s) => [s.name, s]))
    const merged: IndexEntry[] = [...(bundled ?? []), ...(community ?? [])].map((entry) => ({
      ...entry,
      installed: entry.installed || installedByName.has(entry.name),
      installedVersion: installedByName.get(entry.name)?.version,
    }))
    merged.sort((a, b) => a.name.localeCompare(b.name))
    return merged
  }, [bundled, community, installed])

  const sections = useMemo(() => categoriseSources(entries), [entries])

  return (
    <div className={styles.root}>
      <div className={styles.container}>
        <div className={styles.header}>
          <Typography.HeadingLarge as="h1">Sources</Typography.HeadingLarge>
          <Typography.Body variant="secondary">
            Connect external systems to query their data from Coral. Click a source to install or
            inspect it.
          </Typography.Body>
        </div>

        {error ? (
          <ErrorBanner
            title="Couldn't load sources"
            message={error}
            onRetry={() => window.location.reload()}
          />
        ) : null}

        {loading ? (
          <div className={styles.loadingState}>
            <Icon name="Loader" size="16" color="tertiary" className={styles.spinAnimation} />
            <Typography.BodySmall variant="tertiary">Loading sources…</Typography.BodySmall>
          </div>
        ) : null}

        {!loading && !error && entries.length === 0 ? (
          <div className={styles.emptyState}>
            <Icon name="Plug" size="24" color="tertiary" />
            <Typography.Body variant="secondary">
              No sources available. Check the Coral build for a populated catalog.
            </Typography.Body>
          </div>
        ) : null}

        {sections.map((section) => (
          <CategorySection
            key={section.category.key}
            category={section.category}
            entries={section.entries}
            onPick={(entry) => {
              if (entry.installed) {
                navigate({ route: { kind: 'source-detail', name: entry.name } })
              } else {
                navigate({
                  route: { kind: 'source-install', name: entry.name, origin: entry.origin },
                })
              }
            }}
          />
        ))}
      </div>
    </div>
  )
}

function CategorySection({
  category,
  entries,
  onPick,
}: {
  category: CategoryDef
  entries: IndexEntry[]
  onPick: (entry: IndexEntry) => void
}) {
  return (
    <div className={styles.categorySection}>
      <Typography.HeadingXSmall as="h2">{category.label}</Typography.HeadingXSmall>
      <div className={styles.cardGrid}>
        {entries.map((entry) => (
          <SourceCard key={`${entry.origin}:${entry.name}`} entry={entry} onClick={() => onPick(entry)} />
        ))}
      </div>
    </div>
  )
}

function SourceCard({ entry, onClick }: { entry: IndexEntry; onClick: () => void }) {
  const icon = providerIcon(entry.name)
  return (
    <button type="button" onClick={onClick} className={styles.card}>
      <div className={styles.cardHeader}>
        <div className={styles.cardLogo}>
          {icon ? (
            <img alt="" src={icon} className={styles.cardLogoImg} />
          ) : (
            <Icon name="Plug" size="18" color="tertiary" />
          )}
        </div>
        <Typography.BodyLargeStrong as="span" className={styles.cardTitle}>
          {entry.name}
        </Typography.BodyLargeStrong>
        {entry.installed ? (
          <span className={styles.statusPill}>
            <Icon color="success" name="CircleCheck" size="14" />
            Connected
          </span>
        ) : (
          <span className={styles.originPill} data-origin={entry.origin}>
            {entry.origin === 'bundled' ? 'Core' : 'Community'}
          </span>
        )}
      </div>
      {entry.description ? (
        <Typography.Body variant="tertiary" className={styles.cardDescription}>
          {entry.description}
        </Typography.Body>
      ) : null}
      <div className={styles.cardFooter}>
        <Typography.BodySmall variant="tertiary">
          v{entry.installedVersion ?? entry.version}
        </Typography.BodySmall>
      </div>
    </button>
  )
}
