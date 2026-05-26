import { useCallback, useEffect, useState } from 'react'

import type { Source } from '@/generated/coral/v1/sources_pb'
import type { ValidateSourceResponse } from '@/generated/coral/v1/sources_pb'

import * as Button from '@/wax/components/button'
import { Icon } from '@/wax/components/icon'
import { Typography } from '@/wax/components/typography'

import { ErrorBanner } from '@/components/error-banner'
import { PageHeader } from '@/components/page-header'
import { showToast } from '@/components/toast'
import { providerIcon } from '@/lib/provider-icons'
import { useRouter } from '@/lib/router'
import {
  deleteSource,
  getInstalledSource,
  validateSource,
  type SourceOriginLabel,
} from '@/lib/sources'

import * as styles from './source-detail.css'

type ValidationState =
  | { kind: 'idle' }
  | { kind: 'busy' }
  | { kind: 'ok'; tableCount: number; functionCount: number }
  | { kind: 'failed'; message: string }

export function SourceDetail({ name }: { name: string }) {
  const { navigate } = useRouter()
  const [source, setSource] = useState<Source | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [validation, setValidation] = useState<ValidationState>({ kind: 'idle' })
  const [confirmDelete, setConfirmDelete] = useState(false)
  const [deleting, setDeleting] = useState(false)

  useEffect(() => {
    let cancelled = false
    getInstalledSource(name)
      .then((s) => !cancelled && setSource(s))
      .catch((e) => !cancelled && setLoadError(e instanceof Error ? e.message : String(e)))
    return () => {
      cancelled = true
    }
  }, [name])

  const onValidate = useCallback(async () => {
    setValidation({ kind: 'busy' })
    try {
      const result: ValidateSourceResponse = await validateSource(name)
      const failed = result.queryTests.find((q) => q.outcome.case === 'failure')
      if (failed && failed.outcome.case === 'failure') {
        setValidation({
          kind: 'failed',
          message: failed.outcome.value.errorMessage || 'Validation failed',
        })
      } else {
        setValidation({
          kind: 'ok',
          tableCount: result.tables.length,
          functionCount: result.tableFunctions.length,
        })
      }
    } catch (e) {
      setValidation({
        kind: 'failed',
        message: e instanceof Error ? e.message : String(e),
      })
    }
  }, [name])

  const onDelete = useCallback(async () => {
    setDeleting(true)
    try {
      await deleteSource(name)
      showToast('success', `Removed ${name}`)
      navigate({ route: { kind: 'sources' } })
    } catch (e) {
      showToast('error', e instanceof Error ? e.message : String(e))
      setDeleting(false)
    }
  }, [name, navigate])

  const icon = providerIcon(name)
  const origin = source ? originLabel(source.origin) : null

  return (
    <div className={styles.root}>
      <PageHeader
        title={
          <div className={styles.titleRow}>
            <div className={styles.titleIcon}>
              {icon ? (
                <img src={icon} alt="" className={styles.titleIconImg} />
              ) : (
                <Icon name="Plug" size="22" color="secondary" />
              )}
            </div>
            <Typography.HeadingMedium as="h1">{name}</Typography.HeadingMedium>
            {origin ? <span className={styles.originBadge}>{originBadgeLabel(origin)}</span> : null}
          </div>
        }
        subtitle={source?.version ? `v${source.version}` : undefined}
      >
        <Button.Container
          variant="secondary"
          size="32"
          onClick={() => navigate({ route: { kind: 'sources' } })}
        >
          <Button.Text>Back to sources</Button.Text>
        </Button.Container>
      </PageHeader>

      <div className={styles.body}>
        {loadError ? (
          <ErrorBanner
            title="Couldn't load source"
            message={loadError}
            onRetry={() => window.location.reload()}
          />
        ) : null}

        {!source && !loadError ? (
          <Typography.BodySmall variant="tertiary">Loading…</Typography.BodySmall>
        ) : !source ? null : (
          <>
            <div className={styles.grid}>
              <Bindings source={source} />
              <Validation state={validation} onValidate={onValidate} />
            </div>

            <DeleteCard
              name={name}
              confirm={confirmDelete}
              deleting={deleting}
              onArm={() => setConfirmDelete(true)}
              onCancel={() => setConfirmDelete(false)}
              onConfirm={() => void onDelete()}
            />
          </>
        )}
      </div>
    </div>
  )
}

function Bindings({ source }: { source: Source }) {
  return (
    <div className={styles.card}>
      <div className={styles.cardTitle}>
        <Typography.HeadingXSmall as="h2">Configuration</Typography.HeadingXSmall>
      </div>
      <div className={styles.cardList}>
        {source.variables.length === 0 && source.secrets.length === 0 ? (
          <Typography.BodySmall variant="tertiary">No bindings recorded.</Typography.BodySmall>
        ) : null}
        {source.variables.map((v) => (
          <div key={`var:${v.key}`} className={styles.keyValue}>
            <span className={styles.keyLabel}>{v.key}</span>
            <span className={styles.keyValueText}>{v.value || '—'}</span>
          </div>
        ))}
        {source.secrets.map((s) => (
          <div key={`sec:${s.key}`} className={styles.keyValue}>
            <span className={styles.keyLabel}>{s.key}</span>
            <span className={styles.keyValueText}>•••••••• (secret)</span>
          </div>
        ))}
      </div>
    </div>
  )
}

function Validation({
  state,
  onValidate,
}: {
  state: ValidationState
  onValidate: () => void
}) {
  return (
    <div className={styles.card}>
      <div className={styles.cardTitle}>
        <Typography.HeadingXSmall as="h2">Connection</Typography.HeadingXSmall>
        <Button.Container variant="secondary" size="32" onClick={onValidate} disabled={state.kind === 'busy'}>
          <Button.Icon name={state.kind === 'busy' ? 'Loader' : 'RefreshCw'} />
          <Button.Text>{state.kind === 'busy' ? 'Validating…' : 'Validate'}</Button.Text>
        </Button.Container>
      </div>
      {state.kind === 'idle' ? (
        <Typography.BodySmall variant="tertiary">
          Run the source's authored test queries to confirm it's reachable.
        </Typography.BodySmall>
      ) : state.kind === 'busy' ? (
        <Typography.BodySmall variant="tertiary">Running test queries…</Typography.BodySmall>
      ) : state.kind === 'ok' ? (
        <div className={styles.validateBox}>
          <div className={styles.validateRow}>
            <Icon name="CircleCheck" size="16" color="success" />
            <Typography.BodySmall variant="primary">Ready</Typography.BodySmall>
          </div>
          <Typography.BodySmall variant="tertiary">
            {state.tableCount} table{state.tableCount === 1 ? '' : 's'}
            {state.functionCount > 0
              ? ` · ${state.functionCount} function${state.functionCount === 1 ? '' : 's'}`
              : null}
          </Typography.BodySmall>
        </div>
      ) : (
        <div className={styles.errorBox}>
          <Icon name="CircleAlert" size="16" color="error" />
          <Typography.BodySmall variant="primary">{state.message}</Typography.BodySmall>
        </div>
      )}
    </div>
  )
}

function DeleteCard({
  name,
  confirm,
  deleting,
  onArm,
  onCancel,
  onConfirm,
}: {
  name: string
  confirm: boolean
  deleting: boolean
  onArm: () => void
  onCancel: () => void
  onConfirm: () => void
}) {
  if (!confirm) {
    return (
      <div className={styles.card}>
        <div className={styles.cardTitle}>
          <Typography.HeadingXSmall as="h2">Remove</Typography.HeadingXSmall>
          <Button.Container variant="secondary" size="32" onClick={onArm}>
            <Button.Icon name="X" />
            <Button.Text>Remove source</Button.Text>
          </Button.Container>
        </div>
        <Typography.BodySmall variant="tertiary">
          Deletes the source configuration and stored credentials from this workspace.
        </Typography.BodySmall>
      </div>
    )
  }
  return (
    <div className={styles.deleteCard}>
      <Typography.BodyStrong as="span">Remove {name}?</Typography.BodyStrong>
      <Typography.BodySmall variant="primary">
        This deletes the source and its stored credentials. You can reinstall later, but you'll need to
        re-supply any secrets.
      </Typography.BodySmall>
      <div style={{ display: 'flex', gap: 8 }}>
        <Button.Container variant="secondary" size="32" onClick={onCancel} disabled={deleting}>
          <Button.Text>Cancel</Button.Text>
        </Button.Container>
        <Button.Container variant="primary" size="32" onClick={onConfirm} disabled={deleting}>
          <Button.Icon name={deleting ? 'Loader' : 'X'} />
          <Button.Text>{deleting ? 'Removing…' : 'Remove'}</Button.Text>
        </Button.Container>
      </div>
    </div>
  )
}

function originLabel(origin: number): SourceOriginLabel {
  // Mirrors the conversion in lib/sources but keeps this component self-contained.
  if (origin === 1) return 'bundled'
  if (origin === 2) return 'imported'
  return 'unknown'
}

function originBadgeLabel(origin: SourceOriginLabel): string {
  if (origin === 'bundled') return 'Core'
  if (origin === 'imported') return 'Imported'
  return '—'
}
