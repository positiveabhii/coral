import { style } from '@vanilla-extract/css'

import { theme } from '@/wax/theme/theme.css'

export const root = style({
  display: 'flex',
  flexDirection: 'column',
  height: '100%',
  minHeight: 0,
})

export const backBtn = style({
  alignItems: 'center',
  background: 'transparent',
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 8,
  color: theme.content.secondary,
  cursor: 'pointer',
  display: 'flex',
  height: 32,
  justifyContent: 'center',
  width: 32,
  ':hover': { background: theme.surface.onMainContent },
})

export const titleRow = style({
  alignItems: 'center',
  display: 'flex',
  gap: 10,
})

export const titleIcon = style({
  alignItems: 'center',
  background: theme.surface.onMainContent,
  borderRadius: 8,
  display: 'flex',
  flexShrink: 0,
  height: 32,
  justifyContent: 'center',
  overflow: 'hidden',
  width: 32,
})

export const titleIconImg = style({
  height: 22,
  objectFit: 'contain',
  width: 22,
})

export const coreBadge = style({
  background: theme.surface.onMainContentSubtle,
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 6,
  color: theme.content.tertiary,
  fontFamily: '"Roboto Mono", monospace',
  fontSize: 11,
  letterSpacing: '0.06em',
  paddingBlock: 2,
  paddingInline: 8,
  textTransform: 'uppercase',
})

export const body = style({
  display: 'flex',
  flex: 1,
  flexDirection: 'column',
  gap: 12,
  minHeight: 0,
  overflow: 'auto',
  paddingBlock: 32,
  paddingInline: 32,
})

export const card = style({
  alignSelf: 'center',
  background: theme.surface.card,
  border: `1px solid ${theme.stroke.primary}`,
  borderRadius: 14,
  display: 'flex',
  flexDirection: 'column',
  gap: 16,
  maxWidth: 560,
  padding: 24,
  width: '100%',
})

export const fields = style({
  display: 'flex',
  flexDirection: 'column',
  gap: 14,
})

export const field = style({
  display: 'flex',
  flexDirection: 'column',
  gap: 6,
})

export const fieldLabel = style({
  alignItems: 'center',
  display: 'flex',
  gap: 8,
})

export const fieldKey = style({
  color: theme.content.primary,
  fontFamily: '"Roboto Mono", monospace',
  fontSize: 13,
  fontWeight: 600,
})

export const required = style({
  color: theme.content.warning,
  fontSize: 11,
  letterSpacing: '0.04em',
  textTransform: 'uppercase',
})

export const secretTag = style({
  background: theme.pill.gray.background,
  borderRadius: 4,
  color: theme.pill.gray.color,
  fontFamily: '"Roboto Mono", monospace',
  fontSize: 11,
  paddingBlock: 1,
  paddingInline: 6,
})

export const inputRow = style({
  alignItems: 'center',
  display: 'flex',
  gap: 6,
})

export const input = style({
  background: theme.surface.mainContent,
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 8,
  color: theme.content.primary,
  flex: 1,
  fontFamily: 'inherit',
  fontSize: 13,
  paddingBlock: 8,
  paddingInline: 10,
  ':focus': { borderColor: theme.stroke.focused, outline: 'none' },
})

export const eyeBtn = style({
  alignItems: 'center',
  background: 'transparent',
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 8,
  cursor: 'pointer',
  display: 'flex',
  height: 34,
  justifyContent: 'center',
  width: 34,
  ':hover': { background: theme.surface.onMainContentHover },
})

export const fieldHint = style({
  marginInlineStart: 2,
})

export const errorBox = style({
  alignItems: 'center',
  background: theme.pill.red.background,
  border: `1px solid ${theme.pill.red.stroke}`,
  borderRadius: 8,
  color: theme.pill.red.color,
  display: 'flex',
  gap: 8,
  padding: 10,
})

export const methodTabs = style({
  display: 'flex',
  gap: 4,
  marginBlockEnd: 8,
  padding: 4,
  background: theme.surface.onMainContent,
  borderRadius: 8,
})

export const methodTab = style({
  background: 'transparent',
  border: 'none',
  borderRadius: 6,
  color: theme.content.secondary,
  cursor: 'pointer',
  fontSize: 12,
  fontWeight: 500,
  padding: '6px 10px',
  transition: 'background 80ms ease, color 80ms ease',
  ':disabled': { cursor: 'not-allowed', opacity: 0.6 },
  ':hover': { background: theme.surface.onMainContentHover, color: theme.content.primary },
  selectors: {
    '&[data-active="true"]': {
      background: theme.surface.card,
      color: theme.content.primary,
    },
  },
})

export const oauthFields = style({
  display: 'flex',
  flexDirection: 'column',
  gap: 12,
})

export const oauthBox = style({
  alignItems: 'flex-start',
  background: theme.surface.onMainContent,
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 8,
  display: 'flex',
  gap: 10,
  marginBlockStart: 12,
  padding: 12,
})
