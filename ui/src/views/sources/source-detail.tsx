import { useCallback, useEffect, useState } from 'react'

import type { Source, ValidateSourceResponse } from '@/generated/coral/v1/sources_pb'

import { Container as ButtonContainer } from '@/wax/components/button/container'
import { Icon as ButtonIcon } from '@/wax/components/button/icon'
import { Text as ButtonText } from '@/wax/components/button/text'
import * as Dialog from '@/wax/components/dialog'
import { Icon } from '@/wax/components/icon'
import { Typography } from '@/wax/components/typography'

import { showToast } from '@/components/toast'
import { providerIcon } from '@/lib/provider-icons'
import {
  deleteSource,
  getInstalledSource,
  originLabel,
  validateSource,
  type SourceOriginLabel,
} from '@/lib/sources'

import * as styles from './source-detail.css'

type ValidationState =
  | { kind: 'idle' }
  | { kind: 'busy' }
  | { kind: 'ok'; tableCount: number; functionCount: number }
  | { kind: 'failed'; message: string }

export function SourceDetailDialog({
  name,
  open,
  onOpenChange,
  onRemoved,
}: {
  name: string | null
  open: boolean
  onOpenChange: (open: boolean) => void
  onRemoved: (name: string) => void
}) {
  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Backdrop />
        <Dialog.Popup size="l">
          {name ? (
            <SourceDetailDialogContent
              key={name}
              name={name}
              onClose={() => onOpenChange(false)}
              onRemoved={onRemoved}
            />
          ) : null}
        </Dialog.Popup>
      </Dialog.Portal>
    </Dialog.Root>
  )
}

function SourceDetailDialogContent({
  name,
  onClose,
  onRemoved,
}: {
  name: string
  onClose: () => void
  onRemoved: (name: string) => void
}) {
  const [source, setSource] = useState<Source | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [validation, setValidation] = useState<ValidationState>({ kind: 'idle' })
  const [confirmingRemove, setConfirmingRemove] = useState(false)
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
      setConfirmingRemove(false)
      onRemoved(name)
    } catch (e) {
      showToast('error', e instanceof Error ? e.message : String(e))
      setDeleting(false)
    }
  }, [name, onRemoved])

  const icon = providerIcon(name)
  const origin = source ? originLabel(source.origin) : null

  if (confirmingRemove) {
    return (
      <>
        <Dialog.Title>Remove {name}?</Dialog.Title>
        <Dialog.Description>
          This deletes the source configuration and stored credentials from this workspace. You can
          reinstall later, but you'll need to re-supply any secrets.
        </Dialog.Description>
        <Dialog.Actions>
          <ButtonContainer
            variant="secondary"
            size="32"
            onClick={() => setConfirmingRemove(false)}
            disabled={deleting}
          >
            <ButtonText>Cancel</ButtonText>
          </ButtonContainer>
          <ButtonContainer
            variant="primary"
            size="32"
            onClick={() => void onDelete()}
            disabled={deleting}
          >
            {deleting ? <ButtonIcon name="Loader" /> : null}
            <ButtonText>{deleting ? 'Removing…' : 'Remove'}</ButtonText>
          </ButtonContainer>
        </Dialog.Actions>
      </>
    )
  }

  return (
    <>
      <div className={styles.header}>
        <div className={styles.headerLogo}>
          {icon ? (
            <img src={icon} alt="" className={styles.headerLogoImg} />
          ) : (
            <Icon name="Plug" size="22" color="secondary" />
          )}
        </div>
        <div className={styles.headerText}>
          <Dialog.Title className={styles.headerTitleRow}>
            <Typography.HeadingMedium as="span" className={styles.headerTitle}>
              {name}
            </Typography.HeadingMedium>
            {origin ? <span className={styles.headerPill}>{originBadgeLabel(origin)}</span> : null}
          </Dialog.Title>
          <Dialog.Description render={<div />}>
            <Typography.BodySmall variant="secondary">
              {source?.version ? `v${source.version}` : 'Connected source'}
            </Typography.BodySmall>
          </Dialog.Description>
        </div>
      </div>

      {loadError ? (
        <div className={styles.alertError}>
          <Icon name="CircleAlert" size="14" color="inherit" />
          <Typography.BodySmall>{loadError}</Typography.BodySmall>
        </div>
      ) : null}

      {!source && !loadError ? (
        <Typography.BodySmall variant="tertiary">Loading…</Typography.BodySmall>
      ) : !source ? null : (
        <>
          <Bindings source={source} />
          <Validation state={validation} onValidate={onValidate} />
        </>
      )}

      <Dialog.Actions>
        <ButtonContainer variant="bare" size="32" onClick={() => setConfirmingRemove(true)}>
          <ButtonText>Remove</ButtonText>
        </ButtonContainer>
        <ButtonContainer variant="primary" size="32" onClick={onClose}>
          <ButtonText>Close</ButtonText>
        </ButtonContainer>
      </Dialog.Actions>
    </>
  )
}

function Bindings({ source }: { source: Source }) {
  return (
    <section className={styles.section}>
      <Typography.HeadingXSmall as="h3">Configuration</Typography.HeadingXSmall>
      {source.variables.length === 0 && source.secrets.length === 0 ? (
        <Typography.BodySmall variant="tertiary">No bindings recorded.</Typography.BodySmall>
      ) : (
        <div className={styles.bindingList}>
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
      )}
    </section>
  )
}

function Validation({ state, onValidate }: { state: ValidationState; onValidate: () => void }) {
  return (
    <section className={styles.section}>
      <div className={styles.sectionHead}>
        <Typography.HeadingXSmall as="h3">Connection</Typography.HeadingXSmall>
        <ButtonContainer
          variant="secondary"
          size="32"
          onClick={onValidate}
          disabled={state.kind === 'busy'}
        >
          <ButtonIcon name={state.kind === 'busy' ? 'Loader' : 'RefreshCw'} />
          <ButtonText>{state.kind === 'busy' ? 'Validating…' : 'Validate'}</ButtonText>
        </ButtonContainer>
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
        <div className={styles.alertError}>
          <Icon name="CircleAlert" size="14" color="inherit" />
          <Typography.BodySmall>{state.message}</Typography.BodySmall>
        </div>
      )}
    </section>
  )
}

function originBadgeLabel(origin: SourceOriginLabel): string {
  if (origin === 'bundled') return 'Core'
  if (origin === 'imported') return 'Imported'
  return '—'
}
