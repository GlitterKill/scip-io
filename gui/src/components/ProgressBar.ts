export interface ProgressBarOptions {
  label?: string;
  value: number;
  showValue?: boolean;
  size?: 'sm' | 'md' | 'lg';
  indeterminate?: boolean;
}

export function renderProgressBar(options: ProgressBarOptions): HTMLElement {
  const { label, value, showValue = true, size = 'md', indeterminate = false } = options;
  const clamped = Math.max(0, Math.min(100, value));

  const wrapper = document.createElement('div');
  wrapper.className = 'progress-wrapper';

  if (label || showValue) {
    const labelRow = document.createElement('div');
    labelRow.className = 'progress-wrapper__label';

    const labelText = document.createElement('span');
    labelText.textContent = label || '';
    labelRow.appendChild(labelText);

    if (showValue && !indeterminate) {
      const valueText = document.createElement('span');
      valueText.className = 'progress-wrapper__value';
      valueText.textContent = `${Math.round(clamped)}%`;
      labelRow.appendChild(valueText);
    }

    wrapper.appendChild(labelRow);
  }

  const track = document.createElement('div');
  const sizeClass = size === 'sm' ? ' progress--sm' : size === 'lg' ? ' progress--lg' : '';
  track.className = `progress${sizeClass}${indeterminate ? ' progress--indeterminate' : ''}`;

  const fill = document.createElement('div');
  fill.className = 'progress__fill';
  if (!indeterminate) {
    fill.style.width = `${clamped}%`;
  }

  track.appendChild(fill);
  wrapper.appendChild(track);

  return wrapper;
}

export function updateProgressBar(wrapper: HTMLElement, value: number) {
  const clamped = Math.max(0, Math.min(100, value));
  const fill = wrapper.querySelector('.progress__fill') as HTMLElement | null;
  if (fill) {
    fill.style.width = `${clamped}%`;
  }
  const valueEl = wrapper.querySelector('.progress-wrapper__value');
  if (valueEl) {
    valueEl.textContent = `${Math.round(clamped)}%`;
  }
}
