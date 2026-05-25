import { keyframes, style } from '@vanilla-extract/css'
import { recipe } from '@vanilla-extract/recipes'

import { theme, zIndex } from '@/wax/theme/theme.css'

const slideUp = keyframes({
  from: { opacity: 0, transform: 'translateY(8px)' },
  to: { opacity: 1, transform: 'translateY(0)' },
})

export const host = style({
  bottom: 24,
  display: 'flex',
  flexDirection: 'column',
  gap: 8,
  left: '50%',
  pointerEvents: 'none',
  position: 'fixed',
  transform: 'translateX(-50%)',
  zIndex: zIndex.notification,
})

export const toast = recipe({
  base: {
    alignItems: 'center',
    animation: `${slideUp} 160ms ease-out`,
    background: theme.surface.floating,
    border: `1px solid ${theme.stroke.floating}`,
    borderRadius: 10,
    boxShadow: theme.elevation.e3,
    display: 'flex',
    gap: 10,
    maxWidth: 480,
    paddingBlock: 10,
    paddingInline: 14,
    pointerEvents: 'auto',
  },
  variants: {
    kind: {
      success: {},
      error: {},
      info: {},
    },
  },
})
