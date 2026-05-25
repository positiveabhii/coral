// Maps a Coral source name (or arbitrary key) to a provider icon URL under
// /images/providers/. Returns null when there's no matching asset and the
// caller should fall back to a generic icon.

const PROVIDER_ICONS: Record<string, string> = {
  datadog: '/images/providers/datadog.svg',
  github: '/images/providers/github.svg',
  gitlab: '/images/providers/gitlab.svg',
  grafana: '/images/providers/grafana.svg',
  launchdarkly: '/images/providers/launchdarkly.svg',
  linear: '/images/providers/linear.svg',
  openai: '/images/providers/openai.svg',
  pagerduty: '/images/providers/pagerduty.png',
  posthog: '/images/providers/posthog.png',
  sentry: '/images/providers/sentry.svg',
  slack: '/images/providers/slack.png',
}

export function providerIcon(key: string): string | null {
  return PROVIDER_ICONS[key.toLowerCase()] ?? null
}
