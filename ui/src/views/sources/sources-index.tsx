import { useCallback, useEffect, useMemo, useState } from 'react'

import { Icon } from '@/wax/components/icon'
import { TextInput } from '@/wax/components/inputs/text'
import { Tooltip } from '@/wax/components/tooltip'
import { Typography } from '@/wax/components/typography'

import { ErrorBanner } from '@/components/error-banner'
import { providerIcon } from '@/lib/provider-icons'
import {
  discoverBundled,
  listInstalledSources,
  type CatalogEntry,
  type InstalledSource,
} from '@/lib/sources'

import { SourceDetailDialog } from './source-detail'
import { SourceInstallDialog } from './source-install'
import * as styles from './sources-index.css'

interface IndexEntry extends CatalogEntry {
  installedVersion?: string
}

export function SourcesIndex() {
  const [bundled, setBundled] = useState<CatalogEntry[] | null>(null)
  const [installed, setInstalled] = useState<InstalledSource[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [search, setSearch] = useState('')
  const [installingName, setInstallingName] = useState<string | null>(null)
  const [detailName, setDetailName] = useState<string | null>(null)

  const refresh = useCallback(async () => {
    try {
      const [installedRes, bundledRes] = await Promise.all([
        listInstalledSources(),
        discoverBundled(),
      ])
      setInstalled(installedRes)
      setBundled(bundledRes)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const loading = installed === null && bundled === null && !error

  const allEntries = useMemo<IndexEntry[]>(() => {
    const installedByName = new Map((installed ?? []).map((s) => [s.name, s]))
    const merged: IndexEntry[] = (bundled ?? []).map((entry) => ({
      ...entry,
      installed: entry.installed || installedByName.has(entry.name),
      installedVersion: installedByName.get(entry.name)?.version,
    }))
    merged.sort((a, b) => a.name.localeCompare(b.name))
    return merged
  }, [bundled, installed])

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase()
    if (!q) return allEntries
    return allEntries.filter(
      (entry) =>
        entry.name.toLowerCase().includes(q) || entry.description.toLowerCase().includes(q),
    )
  }, [allEntries, search])

  const connected = useMemo(() => filtered.filter((entry) => entry.installed), [filtered])
  const available = useMemo(() => filtered.filter((entry) => !entry.installed), [filtered])

  const onPick = (entry: IndexEntry) => {
    if (entry.installed) {
      setDetailName(entry.name)
    } else {
      setInstallingName(entry.name)
    }
  }

  const onInstalled = useCallback(
    (name: string) => {
      setInstallingName(null)
      void refresh()
      setDetailName(name)
    },
    [refresh],
  )

  const onRemoved = useCallback(() => {
    setDetailName(null)
    void refresh()
  }, [refresh])

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

        <div className={styles.searchBar}>
          <TextInput
            value={search}
            onChange={setSearch}
            placeholder="Search sources…"
            icon="Search"
          />
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

        {!loading && !error && allEntries.length === 0 ? (
          <div className={styles.emptyState}>
            <Icon name="Plug" size="24" color="tertiary" />
            <Typography.Body variant="secondary">
              No sources available. Check the Coral build for a populated catalog.
            </Typography.Body>
          </div>
        ) : null}

        {connected.length > 0 ? (
          <Section title="Connected" count={connected.length}>
            <div className={styles.cardGrid}>
              {connected.map((entry) => (
                <SourceCard
                  key={`${entry.origin}:${entry.name}`}
                  entry={entry}
                  onClick={() => onPick(entry)}
                />
              ))}
            </div>
          </Section>
        ) : null}

        {available.length > 0 ? (
          <Section title="Available" count={available.length}>
            <div className={styles.cardGrid}>
              {available.map((entry) => (
                <SourceCard
                  key={`${entry.origin}:${entry.name}`}
                  entry={entry}
                  onClick={() => onPick(entry)}
                />
              ))}
            </div>
          </Section>
        ) : !loading && !error && allEntries.length > 0 ? (
          <Typography.BodySmall variant="tertiary">
            No sources match your search.
          </Typography.BodySmall>
        ) : null}
      </div>

      <SourceInstallDialog
        name={installingName}
        open={installingName !== null}
        onOpenChange={(open) => {
          if (!open) setInstallingName(null)
        }}
        onInstalled={onInstalled}
      />

      <SourceDetailDialog
        name={detailName}
        open={detailName !== null}
        onOpenChange={(open) => {
          if (!open) setDetailName(null)
        }}
        onRemoved={onRemoved}
      />
    </div>
  )
}

function Section({
  title,
  count,
  children,
}: {
  title: string
  count: number
  children: React.ReactNode
}) {
  return (
    <div className={styles.categorySection}>
      <div className={styles.sectionHead}>
        <Typography.HeadingXSmall as="h2">{title}</Typography.HeadingXSmall>
        <span className={styles.sectionCount}>{count}</span>
      </div>
      {children}
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
        <span className={styles.originPill}>Core</span>
        {entry.installed ? (
          <Tooltip content="Connected">
            <span className={styles.connectedIcon} aria-label="Connected">
              <Icon color="success" name="CircleCheck" size="16" />
            </span>
          </Tooltip>
        ) : null}
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
