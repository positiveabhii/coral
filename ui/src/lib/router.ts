import { useCallback, useSyncExternalStore } from 'react'

export type Route =
  | { kind: 'traces' }
  | { kind: 'sources' }
  | { kind: 'source-detail'; name: string }

export interface ParsedLocation {
  route: Route
}

function parseHash(): ParsedLocation {
  const raw = window.location.hash.replace(/^#\/?/, '')
  const segments = raw.split('?')[0].split('/').filter(Boolean)

  if (segments[0] === 'sources') {
    if (segments[1] === 'detail' && segments[2]) {
      return { route: { kind: 'source-detail', name: decodeURIComponent(segments[2]) } }
    }
    return { route: { kind: 'sources' } }
  }

  if (segments[0] === 'traces' || segments.length === 0) {
    return { route: { kind: 'traces' } }
  }

  return { route: { kind: 'traces' } }
}

function serialise(parsed: ParsedLocation): string {
  const r = parsed.route
  if (r.kind === 'traces') return '#/traces'
  if (r.kind === 'sources') return '#/sources'
  return `#/sources/detail/${encodeURIComponent(r.name)}`
}

let cachedLocation: ParsedLocation = parseHash()
const listeners = new Set<() => void>()

function onHashChange() {
  cachedLocation = parseHash()
  listeners.forEach((l) => l())
}

function subscribe(listener: () => void) {
  if (listeners.size === 0) window.addEventListener('hashchange', onHashChange)
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
    if (listeners.size === 0) window.removeEventListener('hashchange', onHashChange)
  }
}

function getSnapshot(): ParsedLocation {
  return cachedLocation
}

export function useRouter() {
  const location = useSyncExternalStore(subscribe, getSnapshot, getSnapshot)
  const navigate = useCallback((next: ParsedLocation) => {
    const hash = serialise(next)
    if (window.location.hash !== hash) {
      window.location.hash = hash
    }
  }, [])
  return { location, navigate }
}
