import { Shell } from '@/components/shell'
import { ToastHost } from '@/components/toast'
import { useRouter } from '@/lib/router'
import { TracesPage } from '@/views/TracesPage'
import { SourceDetail } from '@/views/sources/source-detail'
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
  if (route.kind === 'source-detail') {
    return <SourceDetail name={route.name} />
  }
  if (route.kind === 'sources') {
    return <SourcesIndex />
  }
  return <TracesPage />
}
