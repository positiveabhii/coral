import { create } from '@bufbuild/protobuf'
import { useEffect, useMemo, useState } from 'react'

import {
  OAuthCredentialRetrievalSchema,
  type OAuthAuthorizationCodeCredentialMethod,
  type SourceCredentialMethod,
  type SourceInputSpec,
} from '@/generated/coral/v1/sources_pb'

import * as Button from '@/wax/components/button'
import { Icon } from '@/wax/components/icon'
import { Typography } from '@/wax/components/typography'

import { PageHeader } from '@/components/page-header'
import { ErrorBanner } from '@/components/error-banner'
import { showToast } from '@/components/toast'
import { providerIcon } from '@/lib/provider-icons'
import { useRouter } from '@/lib/router'
import {
  createBundledSource,
  createBundledSourceWithOAuth,
  getBundledSourceInfo,
  getCommunitySourceInfo,
  importCommunitySource,
  importCommunitySourceWithOAuth,
  type InstallInput,
  type ResolvedSourceInfo,
} from '@/lib/sources'

import * as styles from './source-install.css'

type InstallProgress =
  | { kind: 'idle' }
  | { kind: 'busy' }
  | { kind: 'awaiting-oauth'; inputKey: string; authorizationUrl: string }
  | { kind: 'oauth-completed'; inputKey: string }

export function SourceInstall({
  name,
  origin,
}: {
  name: string
  origin: 'bundled' | 'community'
}) {
  const { navigate } = useRouter()
  const [resolved, setResolved] = useState<ResolvedSourceInfo | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [values, setValues] = useState<Record<string, string>>({})
  const [revealed, setRevealed] = useState<Set<string>>(new Set())
  const [methodChoices, setMethodChoices] = useState<Record<string, number>>({})
  const [progress, setProgress] = useState<InstallProgress>({ kind: 'idle' })
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    const fetcher = origin === 'community' ? getCommunitySourceInfo : getBundledSourceInfo
    fetcher(name)
      .then((info) => !cancelled && setResolved(info))
      .catch((e) => !cancelled && setLoadError(e instanceof Error ? e.message : String(e)))
    return () => {
      cancelled = true
    }
  }, [name, origin])

  const inputs: SourceInputSpec[] = resolved?.info.inputs ?? []
  const icon = providerIcon(name)
  const busy = progress.kind !== 'idle' && progress.kind !== 'oauth-completed'

  const canSubmit = useMemo(() => {
    if (!resolved) return false
    return resolved.info.inputs.every((input) => {
      if (!input.required) return true
      const choice = methodChoices[input.key] ?? 0
      if (input.input.case === 'variable') {
        const def = input.input.value.defaultValue
        return (values[input.key] ?? def).trim().length > 0
      }
      if (input.input.case === 'secret') {
        const method = input.input.value.credential?.methods[choice]
        if (!method) {
          return (values[input.key] ?? '').trim().length > 0
        }
        if (method.method.case === 'sourceConfig') {
          return (values[input.key] ?? '').trim().length > 0
        }
        if (method.method.case === 'oauthAuthorizationCode') {
          return oauthMethodReady(method.method.value, values)
        }
      }
      return true
    })
  }, [resolved, values, methodChoices])

  // For each secret, the user's chosen method (defaulting to index 0).
  const effectiveChoice = (input: SourceInputSpec): number =>
    methodChoices[input.key] ?? 0

  async function submit() {
    if (!resolved) return
    setError(null)
    setProgress({ kind: 'busy' })

    try {
      const bindings: InstallInput[] = []
      const retrievalProtos = []

      for (const input of inputs) {
        if (input.input.case === 'variable') {
          const value = (values[input.key] ?? input.input.value.defaultValue ?? '').trim()
          if (value.length > 0) bindings.push({ key: input.key, value, secret: false })
          continue
        }
        if (input.input.case !== 'secret') continue

        const method = input.input.value.credential?.methods[effectiveChoice(input)]
        if (!method || method.method.case === 'sourceConfig') {
          const value = (values[input.key] ?? '').trim()
          if (value.length > 0) bindings.push({ key: input.key, value, secret: true })
          continue
        }
        if (method.method.case === 'oauthAuthorizationCode') {
          const credentialInputs = oauthCredentialInputs(method.method.value, values)
          retrievalProtos.push(
            create(OAuthCredentialRetrievalSchema, {
              inputKey: input.key,
              methodIndex: effectiveChoice(input),
              credentialInputs,
            }),
          )
        }
      }

      const callbacks = {
        onAuthorization: (event: { inputKey: string; authorizationUrl: string }) => {
          setProgress({
            kind: 'awaiting-oauth',
            inputKey: event.inputKey,
            authorizationUrl: event.authorizationUrl,
          })
          // Try to open in a new tab; popup-blocked windows are handled by
          // the visible fallback link in the UI.
          window.open(event.authorizationUrl, '_blank', 'noopener,noreferrer')
        },
        onCompleted: (event: { inputKey: string }) => {
          setProgress({ kind: 'oauth-completed', inputKey: event.inputKey })
        },
      }

      if (origin === 'bundled') {
        if (retrievalProtos.length === 0) {
          await createBundledSource(name, bindings)
        } else {
          await createBundledSourceWithOAuth(name, bindings, retrievalProtos, callbacks)
        }
      } else {
        if (!resolved.manifestYaml) {
          throw new Error('community install requires a resolved manifest YAML')
        }
        if (retrievalProtos.length === 0) {
          await importCommunitySource(resolved.manifestYaml, bindings)
        } else {
          await importCommunitySourceWithOAuth(
            resolved.manifestYaml,
            bindings,
            retrievalProtos,
            callbacks,
          )
        }
      }

      showToast('success', `Installed ${name}`)
      navigate({ route: { kind: 'source-detail', name } })
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
      setProgress({ kind: 'idle' })
    }
  }

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
            <Typography.HeadingMedium as="h1">Install {name}</Typography.HeadingMedium>
            <span className={styles.coreBadge}>{origin === 'bundled' ? 'Core' : 'Community'}</span>
          </div>
        }
        subtitle={
          resolved?.info.description ??
          (origin === 'bundled' ? 'Officially supported by Coral.' : 'Community source.')
        }
      >
        <Button.Container
          variant="secondary"
          size="32"
          onClick={() => navigate({ route: { kind: 'sources' } })}
          disabled={busy}
        >
          <Button.Text>Cancel</Button.Text>
        </Button.Container>
        <Button.Container
          variant="primary"
          size="32"
          onClick={() => void submit()}
          disabled={busy || !canSubmit}
        >
          <Button.Icon name={busy ? 'Loader' : 'Plus'} />
          <Button.Text>{busyLabel(progress, name)}</Button.Text>
        </Button.Container>
      </PageHeader>

      <div className={styles.body}>
        {loadError ? <ErrorBanner title="Couldn't load source" message={loadError} /> : null}

        <div className={styles.card}>
          {resolved === null && !loadError ? (
            <Typography.BodySmall variant="tertiary">Loading…</Typography.BodySmall>
          ) : !resolved ? null : inputs.length === 0 ? (
            <Typography.BodySmall variant="tertiary">
              No configuration needed — click Install to add the source.
            </Typography.BodySmall>
          ) : (
            <div className={styles.fields}>
              {inputs.map((input) => (
                <InputRow
                  key={input.key}
                  input={input}
                  methodIndex={effectiveChoice(input)}
                  values={values}
                  revealed={revealed}
                  disabled={busy}
                  onValueChange={(key, value) => setValues((p) => ({ ...p, [key]: value }))}
                  onMethodChange={(key, index) => setMethodChoices((p) => ({ ...p, [key]: index }))}
                  onToggleReveal={(key) =>
                    setRevealed((p) => {
                      const next = new Set(p)
                      if (next.has(key)) next.delete(key)
                      else next.add(key)
                      return next
                    })
                  }
                />
              ))}
            </div>
          )}

          {progress.kind === 'awaiting-oauth' ? (
            <OAuthProgress
              authorizationUrl={progress.authorizationUrl}
              inputKey={progress.inputKey}
            />
          ) : null}

          {progress.kind === 'oauth-completed' ? (
            <div className={styles.oauthBox}>
              <Icon name="CircleCheck" size="16" color="success" />
              <Typography.BodySmall variant="primary">
                {progress.inputKey} authorized. Finishing install…
              </Typography.BodySmall>
            </div>
          ) : null}

          {error ? (
            <div className={styles.errorBox}>
              <Icon name="CircleAlert" size="16" color="error" />
              <Typography.BodySmall variant="primary">{error}</Typography.BodySmall>
            </div>
          ) : null}
        </div>
      </div>
    </div>
  )
}

function InputRow({
  input,
  methodIndex,
  values,
  revealed,
  disabled,
  onValueChange,
  onMethodChange,
  onToggleReveal,
}: {
  input: SourceInputSpec
  methodIndex: number
  values: Record<string, string>
  revealed: Set<string>
  disabled: boolean
  onValueChange: (key: string, value: string) => void
  onMethodChange: (key: string, index: number) => void
  onToggleReveal: (key: string) => void
}) {
  if (input.input.case === 'variable') {
    const def = input.input.value.defaultValue
    const value = values[input.key] ?? def
    return (
      <Field input={input} hint={input.hint}>
        <input
          type="text"
          value={value}
          disabled={disabled}
          onChange={(e) => onValueChange(input.key, e.target.value)}
          placeholder={def || ''}
          className={styles.input}
        />
      </Field>
    )
  }

  if (input.input.case !== 'secret') return null

  const credential = input.input.value.credential
  const methods = credential?.methods ?? []
  const selected = methods[methodIndex]

  return (
    <Field input={input} hint={input.hint}>
      {methods.length > 1 ? (
        <div className={styles.methodTabs}>
          {methods.map((m, i) => (
            <button
              key={i}
              type="button"
              className={styles.methodTab}
              data-active={i === methodIndex ? 'true' : 'false'}
              disabled={disabled}
              onClick={() => onMethodChange(input.key, i)}
            >
              {methodLabel(m, i)}
            </button>
          ))}
        </div>
      ) : null}

      {!selected || selected.method.case === 'sourceConfig' ? (
        <SecretPasteField
          inputKey={input.key}
          value={values[input.key] ?? ''}
          revealed={revealed.has(input.key)}
          disabled={disabled}
          onChange={(v) => onValueChange(input.key, v)}
          onToggleReveal={() => onToggleReveal(input.key)}
        />
      ) : selected.method.case === 'oauthAuthorizationCode' ? (
        <OAuthFields
          oauth={selected.method.value}
          values={values}
          disabled={disabled}
          onValueChange={onValueChange}
        />
      ) : null}
    </Field>
  )
}

function Field({
  input,
  hint,
  children,
}: {
  input: SourceInputSpec
  hint?: string
  children: React.ReactNode
}) {
  const isSecret = input.input.case === 'secret'
  return (
    <div className={styles.field}>
      <label className={styles.fieldLabel}>
        <span className={styles.fieldKey}>{input.key}</span>
        {input.required ? <span className={styles.required}>required</span> : null}
        {isSecret ? <span className={styles.secretTag}>secret</span> : null}
      </label>
      {children}
      {hint ? (
        <Typography.BodySmall variant="tertiary" className={styles.fieldHint}>
          {hint}
        </Typography.BodySmall>
      ) : null}
    </div>
  )
}

function SecretPasteField({
  inputKey,
  value,
  revealed,
  disabled,
  onChange,
  onToggleReveal,
}: {
  inputKey: string
  value: string
  revealed: boolean
  disabled: boolean
  onChange: (v: string) => void
  onToggleReveal: () => void
}) {
  return (
    <div className={styles.inputRow}>
      <input
        type={revealed ? 'text' : 'password'}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(e.target.value)}
        placeholder="••••••••"
        className={styles.input}
        aria-label={inputKey}
      />
      <button
        type="button"
        className={styles.eyeBtn}
        aria-label={revealed ? 'Hide value' : 'Show value'}
        onClick={onToggleReveal}
        disabled={disabled}
      >
        <Icon name={revealed ? 'X' : 'Search'} size="16" color="secondary" />
      </button>
    </div>
  )
}

function OAuthFields({
  oauth,
  values,
  disabled,
  onValueChange,
}: {
  oauth: OAuthAuthorizationCodeCredentialMethod
  values: Record<string, string>
  disabled: boolean
  onValueChange: (key: string, value: string) => void
}) {
  const requiredInputs = oauthRequiredInputs(oauth)
  return (
    <div className={styles.oauthFields}>
      {requiredInputs.length === 0 ? (
        <Typography.BodySmall variant="tertiary">
          Click Install to open your browser and complete sign-in.
        </Typography.BodySmall>
      ) : (
        <>
          <Typography.BodySmall variant="tertiary">
            Provide these to start the OAuth flow:
          </Typography.BodySmall>
          {requiredInputs.map(({ key, label, secret }) => (
            <div key={key} className={styles.field}>
              <label className={styles.fieldLabel}>
                <span className={styles.fieldKey}>{label ?? key}</span>
              </label>
              <input
                type={secret ? 'password' : 'text'}
                value={values[key] ?? ''}
                disabled={disabled}
                onChange={(e) => onValueChange(key, e.target.value)}
                className={styles.input}
                aria-label={key}
              />
            </div>
          ))}
        </>
      )}
    </div>
  )
}

function OAuthProgress({
  authorizationUrl,
  inputKey,
}: {
  authorizationUrl: string
  inputKey: string
}) {
  return (
    <div className={styles.oauthBox}>
      <Icon name="Loader" size="16" color="secondary" />
      <div>
        <Typography.BodySmall variant="primary">
          Waiting for {inputKey} authorization in your browser…
        </Typography.BodySmall>
        <Typography.BodySmall variant="tertiary">
          If the new tab didn't open,{' '}
          <a href={authorizationUrl} target="_blank" rel="noopener noreferrer">
            click here to open it
          </a>
          .
        </Typography.BodySmall>
      </div>
    </div>
  )
}

function methodLabel(method: SourceCredentialMethod, index: number): string {
  if (method.label) return method.label
  if (method.method.case === 'sourceConfig') return 'Paste'
  if (method.method.case === 'oauthAuthorizationCode') return 'OAuth'
  return `Method ${index + 1}`
}

function oauthRequiredInputs(
  oauth: OAuthAuthorizationCodeCredentialMethod,
): { key: string; label?: string; secret: boolean }[] {
  const out: { key: string; label?: string; secret: boolean }[] = []
  const id = oauth.client?.id
  if (id?.input && !id.defaultValue) {
    out.push({ key: id.input, secret: false })
  }
  const secret = oauth.client?.secret
  if (secret?.input) {
    out.push({ key: secret.input, secret: true })
  }
  return out
}

function oauthMethodReady(
  oauth: OAuthAuthorizationCodeCredentialMethod,
  values: Record<string, string>,
): boolean {
  return oauthRequiredInputs(oauth).every(({ key }) => (values[key] ?? '').trim().length > 0)
}

function oauthCredentialInputs(
  oauth: OAuthAuthorizationCodeCredentialMethod,
  values: Record<string, string>,
): { key: string; value: string }[] {
  return oauthRequiredInputs(oauth)
    .map(({ key }) => ({ key, value: (values[key] ?? '').trim() }))
    .filter((entry) => entry.value.length > 0)
}

function busyLabel(progress: InstallProgress, name: string): string {
  if (progress.kind === 'busy') return 'Installing…'
  if (progress.kind === 'awaiting-oauth') return 'Awaiting OAuth…'
  if (progress.kind === 'oauth-completed') return 'Finishing…'
  return `Install ${name}`
}
