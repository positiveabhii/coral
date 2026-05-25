import { Shell } from '@/components/shell'
import { ToastHost } from '@/components/toast'
import { useRouter } from '@/lib/router'
import { TracesPage } from '@/views/TracesPage'
import { SourcesIndex } from '@/views/sources/sources-index'
import { useThemeClassOnBody } from '@/wax/theme/theme-provider'
import '@/app.css'

export function App() {
  useThemeClassOnBody()
  const { location } = useRouter()

  return (
    <Shell>
      {renderRoute(location.route.kind)}
      <ToastHost />
    </Shell>
  )
}

function renderRoute(kind: ReturnType<typeof useRouter>['location']['route']['kind']) {
  if (kind === 'sources' || kind === 'source-install' || kind === 'source-detail') {
    return <SourcesIndex />
  }
  return <TracesPage />
}
