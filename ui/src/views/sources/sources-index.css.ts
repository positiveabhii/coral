import { style } from '@vanilla-extract/css'

import { theme } from '@/wax/theme/theme.css'

export const root = style({
  display: 'flex',
  flexDirection: 'column',
  height: '100%',
  minHeight: 0,
})

export const body = style({
  display: 'flex',
  flexDirection: 'column',
  flex: 1,
  gap: 28,
  minHeight: 0,
  overflow: 'auto',
  paddingBlock: 24,
  paddingInline: 32,
})

// ─────────────────────────────────────────────────────────────────────
// Section primitives

export const section = style({
  display: 'flex',
  flexDirection: 'column',
  gap: 12,
})

export const sectionHead = style({
  alignItems: 'baseline',
  display: 'flex',
  gap: 10,
  justifyContent: 'space-between',
})

export const sectionHeadLeft = style({
  alignItems: 'baseline',
  display: 'flex',
  gap: 8,
  minWidth: 0,
})

export const sectionCount = style({
  background: theme.surface.onMainContentSubtle,
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 999,
  color: theme.content.tertiary,
  fontFamily: '"Roboto Mono", monospace',
  fontSize: 11,
  paddingBlock: 1,
  paddingInline: 8,
})

export const sectionSecondary = style({
  whiteSpace: 'nowrap',
})

// ─────────────────────────────────────────────────────────────────────
// Connected cards

export const grid = style({
  display: 'grid',
  gap: 12,
  gridTemplateColumns: 'repeat(auto-fill, minmax(260px, 1fr))',
})

export const cardWrap = style({
  display: 'flex',
  position: 'relative',
})

export const card = style({
  background: theme.surface.card,
  border: `1px solid ${theme.stroke.primary}`,
  borderRadius: 12,
  cursor: 'pointer',
  display: 'flex',
  flex: 1,
  flexDirection: 'column',
  gap: 16,
  height: '100%',
  padding: 16,
  textAlign: 'left',
  transition: 'border-color 120ms ease, background 120ms ease',
  width: '100%',
  ':hover': {
    background: theme.surface.onMainContent,
    borderColor: theme.stroke.focused,
  },
})

export const cardHeader = style({
  alignItems: 'flex-start',
  display: 'flex',
  justifyContent: 'space-between',
})

export const iconBox = style({
  alignItems: 'center',
  background: theme.surface.onMainContent,
  borderRadius: 8,
  display: 'flex',
  flexShrink: 0,
  height: 36,
  justifyContent: 'center',
  overflow: 'hidden',
  width: 36,
})

export const providerIcon = style({
  height: 22,
  objectFit: 'contain',
  width: 22,
})

export const tagStack = style({
  alignItems: 'flex-end',
  display: 'flex',
  flexDirection: 'column',
  gap: 4,
})

export const originTag = style({
  color: theme.content.tertiary,
  fontSize: 11,
  letterSpacing: '0.06em',
  textTransform: 'uppercase',
})

export const statusPill = style({
  alignItems: 'center',
  background: theme.surface.onMainContentSubtle,
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 999,
  color: theme.content.secondary,
  display: 'inline-flex',
  fontSize: 11,
  gap: 4,
  paddingBlock: 1,
  paddingInline: 6,
  selectors: {
    '&[data-state="ok"]': { color: theme.content.success },
    '&[data-state="error"]': { color: theme.content.error },
  },
})

export const statusDot = style({
  background: theme.content.secondary,
  borderRadius: '50%',
  display: 'inline-block',
  height: 6,
  width: 6,
  selectors: {
    '&[data-state="ok"]': { background: theme.content.success },
    '&[data-state="error"]': { background: theme.content.error },
  },
})

export const cardBody = style({
  display: 'flex',
  flexDirection: 'column',
  gap: 4,
})

export const cardDesc = style({
  marginBlockStart: 6,
})

export const cardActions = style({
  display: 'flex',
  gap: 4,
  insetBlockStart: 8,
  insetInlineEnd: 8,
  opacity: 0,
  position: 'absolute',
  transition: 'opacity 120ms ease',
  selectors: {
    [`${cardWrap}:hover &, &:focus-within`]: { opacity: 1 },
  },
})

export const iconBtn = style({
  alignItems: 'center',
  background: theme.surface.floating,
  border: `1px solid ${theme.stroke.floating}`,
  borderRadius: 6,
  cursor: 'pointer',
  display: 'flex',
  height: 26,
  justifyContent: 'center',
  padding: 0,
  width: 26,
  ':hover': { background: theme.surface.onMainContentHover, borderColor: theme.stroke.focused },
})

// ─────────────────────────────────────────────────────────────────────
// Empty connected

export const empty = style({
  alignItems: 'center',
  border: `1px dashed ${theme.stroke.secondary}`,
  borderRadius: 14,
  display: 'flex',
  flexDirection: 'column',
  gap: 6,
  marginInline: 'auto',
  maxWidth: 480,
  paddingBlock: 56,
  paddingInline: 32,
  textAlign: 'center',
  width: '100%',
})

export const emptyPlus = style({
  alignItems: 'center',
  background: theme.surface.onMainContent,
  borderRadius: 10,
  display: 'flex',
  height: 56,
  justifyContent: 'center',
  marginBlockEnd: 16,
  width: 56,
})

export const emptyCta = style({ marginBlockStart: 12 })

// ─────────────────────────────────────────────────────────────────────
// Library (faceted)

export const libraryToolbar = style({
  alignItems: 'center',
  display: 'flex',
  flexWrap: 'wrap',
  gap: 12,
  justifyContent: 'space-between',
})

export const libraryFilters = style({
  alignItems: 'center',
  display: 'flex',
  flexWrap: 'wrap',
  gap: 6,
})

export const facetChip = style({
  alignItems: 'center',
  background: 'transparent',
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 999,
  color: theme.content.secondary,
  cursor: 'pointer',
  display: 'inline-flex',
  fontFamily: 'inherit',
  fontSize: 12,
  gap: 6,
  paddingBlock: 4,
  paddingInline: 10,
  ':hover': { background: theme.surface.onMainContent },
})

export const facetChipActive = style({
  background: theme.surface.onMainContent,
  borderColor: theme.stroke.focused,
  color: theme.content.primary,
})

export const facetCount = style({
  color: theme.content.tertiary,
  fontFamily: '"Roboto Mono", monospace',
  fontSize: 11,
})

export const facetSep = style({
  background: theme.stroke.secondary,
  height: 16,
  width: 1,
  marginInline: 2,
})

export const librarySearch = style({
  minWidth: 240,
  width: 280,
})

export const libraryGrid = style({
  display: 'grid',
  gap: 10,
  gridTemplateColumns: 'repeat(auto-fill, minmax(240px, 1fr))',
})

export const tile = style({
  background: theme.surface.card,
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 10,
  cursor: 'pointer',
  display: 'flex',
  flexDirection: 'column',
  gap: 10,
  height: '100%',
  padding: 12,
  textAlign: 'left',
  transition: 'border-color 120ms ease, background 120ms ease',
  width: '100%',
  ':hover': {
    background: theme.surface.onMainContent,
    borderColor: theme.stroke.focused,
  },
})

export const tileBuild = style({
  background: theme.surface.onMainContentSubtle,
})

export const tileHeader = style({
  alignItems: 'center',
  display: 'flex',
  justifyContent: 'space-between',
})

export const tileIcon = style({
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

export const tileIconBuild = style({
  alignItems: 'center',
  background: theme.surface.onMainContent,
  border: `1px dashed ${theme.stroke.secondary}`,
  borderRadius: 8,
  display: 'flex',
  flexShrink: 0,
  height: 32,
  justifyContent: 'center',
  width: 32,
})

export const tileIconImg = style({
  height: 20,
  objectFit: 'contain',
  width: 20,
})

export const tileBody = style({
  display: 'flex',
  flexDirection: 'column',
  gap: 2,
  minWidth: 0,
})

export const tileDesc = style({
  display: '-webkit-box',
  overflow: 'hidden',
  textOverflow: 'ellipsis',
  WebkitBoxOrient: 'vertical' as 'vertical',
  WebkitLineClamp: 2,
})

export const tileFooter = style({
  alignItems: 'center',
  display: 'flex',
  justifyContent: 'space-between',
  marginBlockStart: 'auto',
})

export const smallPill = style({
  background: theme.surface.onMainContentSubtle,
  border: `1px solid ${theme.stroke.secondary}`,
  borderRadius: 999,
  color: theme.content.tertiary,
  fontFamily: '"Roboto Mono", monospace',
  fontSize: 10,
  letterSpacing: '0.04em',
  paddingBlock: 1,
  paddingInline: 6,
  textTransform: 'uppercase',
})

export const tileAddHint = style({
  alignItems: 'center',
  color: theme.content.tertiary,
  display: 'inline-flex',
  fontSize: 12,
  gap: 4,
})

export const installedHint = style({
  alignItems: 'center',
  color: theme.content.success,
  display: 'inline-flex',
  fontSize: 12,
  gap: 4,
})

export const thinNotice = style({
  background: theme.surface.onMainContentSubtle,
  border: `1px dashed ${theme.stroke.secondary}`,
  borderRadius: 10,
  padding: 16,
  textAlign: 'center',
})
