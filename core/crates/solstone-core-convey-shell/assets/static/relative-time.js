// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

function relativeTime(ms) {
  let seconds = Math.floor(ms / 1000);
  if (!Number.isFinite(seconds) || seconds < 0) seconds = 0;

  let value;
  let unit;
  if (seconds < 60) {
    value = seconds;
    unit = 'second';
  } else {
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) {
      value = minutes;
      unit = 'minute';
    } else {
      const hours = Math.floor(minutes / 60);
      if (hours < 24) {
        value = hours;
        unit = 'hour';
      } else {
        const days = Math.floor(hours / 24);
        if (days < 7) {
          value = days;
          unit = 'day';
        } else if (days < 28) {
          value = Math.floor(days / 7);
          unit = 'week';
        } else if (days < 60) {
          return '1 month';
        } else {
          value = Math.floor(days / 30);
          unit = 'month';
        }
      }
    }
  }
  return `${value} ${unit}${value === 1 ? '' : 's'}`;
}

window.relativeTime = relativeTime;
