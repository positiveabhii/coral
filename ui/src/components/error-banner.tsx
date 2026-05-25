import { Icon } from '@/wax/components/icon'
import { Typography } from '@/wax/components/typography'

import * as styles from './error-banner.css'

interface ErrorBannerProps {
  title?: string
  message: string
  onRetry?: () => void
}

export function ErrorBanner({ title, message, onRetry }: ErrorBannerProps) {
  return (
    <div className={styles.banner} role="alert">
      <Icon name="CircleAlert" size="18" color="error" />
      <div className={styles.text}>
        {title ? <Typography.BodySmallStrong>{title}</Typography.BodySmallStrong> : null}
        <Typography.BodySmall variant="secondary">{message}</Typography.BodySmall>
      </div>
      {onRetry ? (
        <button type="button" className={styles.retry} onClick={onRetry}>
          <Icon name="RefreshCw" size="14" color="secondary" />
          Retry
        </button>
      ) : null}
    </div>
  )
}
