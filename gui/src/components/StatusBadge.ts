export type BadgeStatus = 'installed' | 'not-installed' | 'outdated' | 'running' | 'error';

export function renderStatusBadge(status: BadgeStatus, label?: string): HTMLElement {
  const badge = document.createElement('span');
  badge.className = `badge badge--${status}`;

  const dot = document.createElement('span');
  dot.className = 'badge__dot';
  badge.appendChild(dot);

  const text = document.createElement('span');
  text.textContent = label || formatStatus(status);
  badge.appendChild(text);

  return badge;
}

function formatStatus(status: BadgeStatus): string {
  switch (status) {
    case 'installed':
      return 'Installed';
    case 'not-installed':
      return 'Not Installed';
    case 'outdated':
      return 'Outdated';
    case 'running':
      return 'Running';
    case 'error':
      return 'Error';
    default:
      return status;
  }
}
