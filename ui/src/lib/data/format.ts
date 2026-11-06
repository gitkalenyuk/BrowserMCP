// Small formatters for the library page.

export function formatBytes(n: number | null | undefined): string {
  if (n === null || n === undefined || !Number.isFinite(n) || n < 0) return '?';
  if (n < 1024) return `${n} B`;
  const units = ['kB', 'MB', 'GB', 'TB'];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v >= 100 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

/** Last path segment, for Windows and POSIX paths. */
export function fileName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

export function stateLabel(state: string): string {
  switch (state) {
    case 'in_progress':
      return 'Downloading';
    case 'complete':
      return 'Done';
    case 'cancelled':
      return 'Cancelled';
    case 'interrupted':
      return 'Failed';
    default:
      return state;
  }
}

/** Case-insensitive filter of logins by site or username. */
export function matchesLogin(e: { origin: string; username: string; note: string }, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return e.origin.toLowerCase().includes(q) || e.username.toLowerCase().includes(q) || e.note.toLowerCase().includes(q);
}
