import { useCallback, useEffect, useMemo, useState } from 'react'

import {
  type Source,
  type SourceCredentialMethod,
  type SourceInputSpec,
} from '@/generated/coral/v1/sources_pb'

import { Container as ButtonContainer } from '@/wax/components/button/container'
import { Icon as ButtonIcon } from '@/wax/components/button/icon'
import { Text as ButtonText } from '@/wax/components/button/text'
import * as Dialog from '@/wax/components/dialog'
import { Icon } from '@/wax/components/icon'
import { TextInput } from '@/wax/components/inputs/text'
import { Typography } from '@/wax/components/typography'

import { showToast } from '@/components/toast'
import { providerIcon } from '@/lib/provider-icons'
import {
  createBundledSource,
  deleteSource,
  getBundledSourceInfo,
  getInstalledSource,
  originLabel,
  type InstallInput,
  type SourceOriginLabel,
} from '@/lib/sources'

import * as styles from './source-detail.css'

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
  const [inputs, setInputs] = useState<SourceInputSpec[] | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [confirmingRemove, setConfirmingRemove] = useState(false)
  const [deleting, setDeleting] = useState(false)

  const refresh = useCallback(async () => {
    try {
      const [installed, info] = await Promise.all([
        getInstalledSource(name),
        getBundledSourceInfo(name).catch(() => null),
      ])
      setSource(installed)
      setInputs(info?.info.inputs ?? [])
    } catch (e) {
      setLoadError(e instanceof Error ? e.message : String(e))
    }
  }, [name])

  useEffect(() => {
    void refresh()
  }, [refresh])

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
        <Bindings source={source} inputs={inputs ?? []} onSaved={refresh} />
      )}

      <Dialog.Actions>
        <ButtonContainer variant="bare" size="32" onClick={() => setConfirmingRemove(true)}>
          <ButtonText>Remove</ButtonText>
        </ButtonContainer>
        <ButtonContainer variant="primary" size="32" onClick={onClose}>
          <ButtonText>Close</ButtonText>
        </ButtonContainer>
      </Dialog.Actions>

      <Dialog.Root open={confirmingRemove} onOpenChange={setConfirmingRemove}>
        <Dialog.Portal>
          <Dialog.Backdrop />
          <Dialog.Popup size="m">
            <Dialog.Title>Remove {name}?</Dialog.Title>
            <Dialog.Description>
              This deletes the source configuration and stored credentials from this workspace. You
              can reinstall later, but you'll need to re-supply any secrets.
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
          </Dialog.Popup>
        </Dialog.Portal>
      </Dialog.Root>
    </>
  )
}

function Bindings({
  source,
  inputs,
  onSaved,
}: {
  source: Source
  inputs: SourceInputSpec[]
  onSaved: () => Promise<void>
}) {
  const inputsByKey = useMemo(() => {
    const out = new Map<string, SourceInputSpec>()
    for (const input of inputs) out.set(input.key, input)
    return out
  }, [inputs])

  if (source.variables.length === 0 && source.secrets.length === 0) {
    return (
      <section className={styles.section}>
        <Typography.HeadingXSmall as="h3">Configuration</Typography.HeadingXSmall>
        <Typography.BodySmall variant="tertiary">No bindings recorded.</Typography.BodySmall>
      </section>
    )
  }

  return (
    <section className={styles.section}>
      <Typography.HeadingXSmall as="h3">Configuration</Typography.HeadingXSmall>
      <div className={styles.bindingList}>
        {source.variables.map((v) => (
          <BindingRow
            key={`var:${v.key}`}
            sourceName={source.name}
            kind="variable"
            keyName={v.key}
            currentValue={v.value}
            source={source}
            onSaved={onSaved}
          />
        ))}
        {source.secrets.map((s) => (
          <BindingRow
            key={`sec:${s.key}`}
            sourceName={source.name}
            kind="secret"
            keyName={s.key}
            currentValue={null}
            source={source}
            inputSpec={inputsByKey.get(s.key)}
            onSaved={onSaved}
          />
        ))}
      </div>
    </section>
  )
}

function BindingRow({
  sourceName,
  kind,
  keyName,
  currentValue,
  source,
  inputSpec,
  onSaved,
}: {
  sourceName: string
  kind: 'variable' | 'secret'
  keyName: string
  currentValue: string | null
  source: Source
  inputSpec?: SourceInputSpec
  onSaved: () => Promise<void>
}) {
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState('')
  const [saving, setSaving] = useState(false)

  const oauthBacked = kind === 'secret' && isOAuthSecret(inputSpec)

  function startEdit() {
    setDraft(kind === 'variable' ? (currentValue ?? '') : '')
    setEditing(true)
  }

  async function save() {
    setSaving(true)
    try {
      const trimmed = draft.trim()
      const bindings: InstallInput[] = []
      for (const v of source.variables) {
        const value = kind === 'variable' && v.key === keyName ? trimmed : v.value
        bindings.push({ key: v.key, value, secret: false })
      }
      if (kind === 'secret') {
        if (trimmed.length === 0) {
          showToast('error', `Enter a new value for ${keyName}`)
          setSaving(false)
          return
        }
        bindings.push({ key: keyName, value: trimmed, secret: true })
      }
      await createBundledSource(sourceName, bindings)
      showToast('success', `Updated ${keyName}`)
      setEditing(false)
      setDraft('')
      await onSaved()
    } catch (e) {
      showToast('error', e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  if (!editing) {
    return (
      <div className={styles.keyValue}>
        <span className={styles.keyLabel}>{keyName}</span>
        <span className={styles.keyValueText}>
          {kind === 'variable' ? currentValue || '—' : '•••••••• (secret)'}
        </span>
        {oauthBacked ? (
          <Typography.BodySmall variant="tertiary">Managed by OAuth</Typography.BodySmall>
        ) : (
          <button
            type="button"
            className={styles.editButton}
            onClick={startEdit}
            aria-label={`Edit ${keyName}`}
          >
            <Icon name="Pencil" size="14" color="secondary" />
          </button>
        )}
      </div>
    )
  }

  return (
    <div className={styles.keyValueEdit}>
      <span className={styles.keyLabel}>{keyName}</span>
      <TextInput
        type={kind === 'secret' ? 'password' : 'text'}
        value={draft}
        onChange={setDraft}
        placeholder={kind === 'secret' ? 'Enter new value' : keyName}
        disabled={saving}
      />
      <div className={styles.editActions}>
        <ButtonContainer
          variant="bare"
          size="32"
          onClick={() => {
            setEditing(false)
            setDraft('')
          }}
          disabled={saving}
        >
          <ButtonText>Cancel</ButtonText>
        </ButtonContainer>
        <ButtonContainer variant="primary" size="32" onClick={() => void save()} disabled={saving}>
          {saving ? <ButtonIcon name="Loader" /> : null}
          <ButtonText>{saving ? 'Saving…' : 'Save'}</ButtonText>
        </ButtonContainer>
      </div>
    </div>
  )
}

function isOAuthSecret(input: SourceInputSpec | undefined): boolean {
  if (!input || input.input.case !== 'secret') return false
  const methods: SourceCredentialMethod[] = input.input.value.credential?.methods ?? []
  return methods.some((m) => m.method.case === 'oauth')
}

function originBadgeLabel(origin: SourceOriginLabel): string {
  if (origin === 'bundled') return 'Core'
  if (origin === 'imported') return 'Imported'
  return '—'
}
