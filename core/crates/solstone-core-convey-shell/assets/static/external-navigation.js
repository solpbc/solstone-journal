// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(() => {
  function getContract() {
    return window.solstoneJournalWebHost || null;
  }

  function capableHost() {
    if (window.top !== window) return false;
    const contract = getContract();
    if (!contract || contract.version !== 1) return false;

    const capability = contract.javascriptCapability;
    if (typeof capability === 'string' && capability !== '' && window[capability] === contract.version) {
      return true;
    }

    const product = contract.userAgentProduct;
    if (typeof product === 'string' && product !== '') {
      const ua = typeof navigator !== 'undefined' && typeof navigator.userAgent === 'string'
        ? navigator.userAgent
        : '';
      const tokens = ua.split(' ');
      for (let i = 0; i < tokens.length; i++) {
        if (tokens[i] !== '' && tokens[i] === product) {
          return true;
        }
      }
    }

    return false;
  }

  function allowedUrl(value) {
    if (typeof value !== 'string') return false;
    if (!/^https?:\/\//i.test(value)) return false;

    try {
      const parsed = new URL(value);
      if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return false;
      if (parsed.username || parsed.password) return false;
      const currentOrigin = window.location ? window.location.origin : '';
      if (currentOrigin && parsed.origin === currentOrigin) return false;
      return true;
    } catch (_err) {
      return false;
    }
  }

  function reserveBlank() {
    if (capableHost()) {
      return { kind: 'native' };
    }

    let popup;
    try {
      popup = window.open('about:blank', '_blank');
    } catch (_err) {
      return { kind: 'blocked' };
    }

    if (!popup || popup.closed) {
      return { kind: 'blocked' };
    }

    try {
      popup.opener = null;
    } catch (_err) {}

    return { kind: 'popup', popup };
  }

  function navigateReserved(reservation, url, method) {
    if (!allowedUrl(url)) return false;
    if (!reservation || reservation.kind === 'blocked') return false;

    if (reservation.kind === 'native') {
      const targetWindow = window.top || window;
      targetWindow.location.assign(url);
      return true;
    }

    if (reservation.kind === 'popup') {
      const popup = reservation.popup;
      if (!popup || popup.closed) return false;

      try {
        popup.opener = null;
      } catch (_err) {}

      if (method === 'replace' && popup.location && typeof popup.location.replace === 'function') {
        popup.location.replace(url);
      } else if (popup.location && 'href' in popup.location) {
        popup.location.href = url;
      } else {
        popup.location = url;
      }
      return true;
    }

    return false;
  }

  function openDelayed(url, features = 'noopener') {
    if (!allowedUrl(url)) return false;

    if (capableHost()) {
      const targetWindow = window.top || window;
      targetWindow.location.assign(url);
      return true;
    }

    try {
      window.open(url, '_blank', features);
      return true;
    } catch (_err) {
      return false;
    }
  }

  if (typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
    document.addEventListener(
      'click',
      (event) => {
        if (!capableHost()) return;
        const target = event.target;
        const anchor = target && typeof target.closest === 'function'
          ? target.closest('a[data-solstone-outside]')
          : null;
        if (!anchor || typeof anchor.getAttribute !== 'function') return;

        event.preventDefault();
        const rawHref = anchor.getAttribute('href');
        if (allowedUrl(rawHref)) {
          const targetWindow = window.top || window;
          targetWindow.location.assign(rawHref);
        }
      },
      true,
    );
  }

  window.solstoneOutside = {
    capableHost,
    allowedUrl,
    reserveBlank,
    navigateReserved,
    openDelayed,
  };
})();
