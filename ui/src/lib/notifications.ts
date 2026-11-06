// Toast list logic for NotificationHost (pure, so it is unit-tested without a DOM).
// The core sends `ui.notification` (show) and `ui.notification_closed` (hide); questions
// (with actions) stay until answered, the rest fade out after a few seconds.

import type { NotifyLevel, UiNotification } from './api/types';

export interface Toast extends UiNotification {
  /** ms timestamp when it fades out; null = stays (questions). */
  expiresAt: number | null;
}

/** How long a plain notification stays, by level. */
export const SHOW_MS: Record<NotifyLevel, number> = {
  info: 6000,
  success: 6000,
  warning: 10000,
  error: 12000,
};

/** At most this many toasts are shown; the rest are counted ("+2 more"). */
export const MAX_VISIBLE = 3;

/** Add (or replace, same id) a toast. Questions go first, then newest first. */
export function addToast(list: Toast[], n: UiNotification, now: number): Toast[] {
  const sticky = n.sticky || n.actions.length > 0;
  const toast: Toast = { ...n, expiresAt: sticky ? null : now + (SHOW_MS[n.level] ?? SHOW_MS.info) };
  const rest = list.filter((t) => t.id !== n.id);
  const questions = rest.filter((t) => t.expiresAt === null);
  const plain = rest.filter((t) => t.expiresAt !== null);
  return sticky ? [toast, ...questions, ...plain] : [...questions, toast, ...plain];
}

export function removeToast(list: Toast[], id: string): Toast[] {
  return list.filter((t) => t.id !== id);
}

/** Drop the toasts whose time is up. Returns the same array when nothing changed. */
export function expire(list: Toast[], now: number): Toast[] {
  const kept = list.filter((t) => t.expiresAt === null || t.expiresAt > now);
  return kept.length === list.length ? list : kept;
}

export function visibleToasts(list: Toast[]): { shown: Toast[]; hidden: number } {
  return { shown: list.slice(0, MAX_VISIBLE), hidden: Math.max(0, list.length - MAX_VISIBLE) };
}
