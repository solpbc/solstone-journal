// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  function resolve(copy, key) {
    if (!copy || typeof key !== 'string' || !key) return '';
    const value = key.split('.').reduce((current, part) => {
      if (current === undefined || current === null) return undefined;
      return current[part];
    }, copy);
    return value === undefined || value === null ? '' : String(value);
  }

  function applyCopy(root, copy) {
    if (!root || typeof root.querySelectorAll !== 'function') return;
    root.querySelectorAll('[data-copy]').forEach((el) => {
      el.textContent = resolve(copy, el.dataset.copy);
    });
    root.querySelectorAll('[data-copy-attr]').forEach((el) => {
      const assignments = String(el.dataset.copyAttr || '')
        .split(';')
        .map((part) => part.trim())
        .filter(Boolean);
      assignments.forEach((assignment) => {
        const separator = assignment.indexOf(':');
        if (separator <= 0) return;
        const attr = assignment.slice(0, separator).trim();
        const key = assignment.slice(separator + 1).trim();
        if (!attr || !key || typeof el.setAttribute !== 'function') return;
        el.setAttribute(attr, resolve(copy, key));
      });
    });
  }

  function findById(root, id) {
    if (!root) return null;
    if (typeof root.getElementById === 'function') {
      return root.getElementById(id);
    }
    if (typeof root.querySelector === 'function') {
      return root.querySelector(`#${id}`);
    }
    return null;
  }



  const NetworkRender = { applyCopy, resolve };
  global.NetworkRender = NetworkRender;
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = NetworkRender;
  }
})(typeof window !== 'undefined' ? window : globalThis);
