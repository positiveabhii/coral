import { Shell } from '@/components/shell'
import { ToastHost } from '@/components/toast'
import { useRouter } from '@/lib/router'
import { TracesPage } from '@/views/TracesPage'
import { SourceInstall } from '@/views/sources/source-install'
import { SourcesIndex } from '@/views/sources/sources-index'
import { useThemeClassOnBody } from '@/wax/theme/theme-provider'
import '@/app.css'

export function App() {
  useThemeClassOnBody()
  const { location } = useRouter()

  return (
    <Shell>
      {renderRoute(location.route)}
      <ToastHost />
    </Shell>
  )
}

function renderRoute(route: ReturnType<typeof useRouter>['location']['route']) {
  if (route.kind === 'source-install') {
    return <SourceInstall name={route.name} origin={route.origin} />
  }
  if (route.kind === 'sources' || route.kind === 'source-detail') {
    // Detail surface lands in M6; for now the index handles the click-through.
    return <SourcesIndex />
  }
  return <TracesPage />
}
