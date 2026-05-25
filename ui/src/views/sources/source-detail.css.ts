import { style } from '@vanilla-extract/css'

import { theme } from '@/wax/theme/theme.css'

export const root = style({
  display: 'flex',
  flexDirection: 'column',
  height: '100%',
  minHeight: 0,
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

export const originBadge = style({
  background: theme.surface.onMainContent,
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 6,
  color: theme.content.secondary,
  fontSize: 11,
  fontWeight: 600,
  letterSpacing: '0.04em',
  padding: '3px 8px',
  textTransform: 'uppercase',
})

export const body = style({
  display: 'flex',
  flexDirection: 'column',
  gap: 20,
  overflow: 'auto',
  padding: 24,
})

export const grid = style({
  display: 'grid',
  gap: 16,
  gridTemplateColumns: 'repeat(auto-fit, minmax(280px, 1fr))',
})

export const card = style({
  background: theme.surface.card,
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 10,
  display: 'flex',
  flexDirection: 'column',
  gap: 12,
  padding: 16,
})

export const cardTitle = style({
  alignItems: 'center',
  display: 'flex',
  gap: 8,
  justifyContent: 'space-between',
})

export const cardList = style({
  display: 'flex',
  flexDirection: 'column',
  gap: 8,
})

export const keyValue = style({
  alignItems: 'baseline',
  display: 'grid',
  gap: 10,
  gridTemplateColumns: 'minmax(120px, max-content) 1fr',
})

export const keyLabel = style({
  color: theme.content.secondary,
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
  fontSize: 12,
})

export const keyValueText = style({
  color: theme.content.primary,
  fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
  fontSize: 12,
  wordBreak: 'break-all',
})

export const validateBox = style({
  alignItems: 'flex-start',
  background: theme.surface.onMainContent,
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 8,
  display: 'flex',
  flexDirection: 'column',
  gap: 8,
  padding: 12,
})

export const validateRow = style({
  alignItems: 'center',
  display: 'flex',
  gap: 8,
})

export const errorBox = style({
  alignItems: 'flex-start',
  background: theme.pill.red.background,
  border: `1px solid ${theme.pill.red.stroke}`,
  borderRadius: 8,
  color: theme.pill.red.color,
  display: 'flex',
  gap: 8,
  padding: 10,
})

export const deleteCard = style({
  background: theme.pill.red.background,
  border: `1px solid ${theme.pill.red.stroke}`,
  borderRadius: 10,
  color: theme.pill.red.color,
  display: 'flex',
  flexDirection: 'column',
  gap: 12,
  padding: 16,
})
