import { Icon, type IconName } from '@/wax/components/icon'
import { CoralIcon } from '@/wax/components/icon/custom-icons/coral'
import { useRouter, type Route } from '@/lib/router'

import * as styles from './navbar.css'

const NAV_ITEMS: { icon: IconName; label: string; target: Route; matches: Route['kind'][] }[] = [
  {
    icon: 'Activity',
    label: 'Traces',
    target: { kind: 'traces' },
    matches: ['traces'],
  },
  {
    icon: 'Plug',
    label: 'Sources',
    target: { kind: 'sources' },
    matches: ['sources'],
  },
]

export function Navbar() {
  const { location, navigate } = useRouter()
  return (
    <nav className={styles.navbar} aria-label="Coral">
      <div className={styles.header}>
        <div className={styles.brandButton}>
          <CoralIcon size={22} />
        </div>
      </div>
      <div className={styles.nav} aria-label="Primary navigation">
        {NAV_ITEMS.map((item) => {
          const active = item.matches.includes(location.route.kind)
          return (
            <button
              aria-current={active ? 'page' : undefined}
              aria-label={item.label}
              className={styles.navButton}
              data-active={active ? 'true' : 'false'}
              key={item.label}
              onClick={() => navigate({ route: item.target })}
              type="button"
            >
              <Icon name={item.icon} size="20" color="inherit" />
            </button>
          )
        })}
      </div>
    </nav>
  )
}
