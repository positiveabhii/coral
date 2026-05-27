import { useEffect, useState } from 'react'

import { Icon } from '@/wax/components/icon'
import { Typography } from '@/wax/components/typography'

import * as styles from './toast.css'

export interface ToastMessage {
  id: number
  kind: 'success' | 'error' | 'info'
  body: string
}

type Listener = (toasts: ToastMessage[]) => void

let toasts: ToastMessage[] = []
let nextId = 1
const listeners = new Set<Listener>()

function emit() {
  for (const l of listeners) l(toasts)
}

export function showToast(kind: ToastMessage['kind'], body: string) {
  const id = nextId++
  toasts = [...toasts, { id, kind, body }]
  emit()
  window.setTimeout(() => {
    toasts = toasts.filter((t) => t.id !== id)
    emit()
  }, 4500)
}

export function ToastHost() {
  const [items, setItems] = useState<ToastMessage[]>(toasts)
  useEffect(() => {
    const l: Listener = (next) => setItems(next)
    listeners.add(l)
    return () => {
      listeners.delete(l)
    }
  }, [])

  return (
    <div className={styles.host} aria-live="polite">
      {items.map((t) => (
        <div key={t.id} className={styles.toast({ kind: t.kind })} role="status">
          <Icon
            name={
              t.kind === 'success'
                ? 'CircleCheck'
                : t.kind === 'error'
                  ? 'CircleAlert'
                  : 'CircleAlert'
            }
            size="16"
            color={t.kind === 'success' ? 'success' : t.kind === 'error' ? 'error' : 'info'}
          />
          <Typography.BodySmall variant="primary">{t.body}</Typography.BodySmall>
        </div>
      ))}
    </div>
  )
}
