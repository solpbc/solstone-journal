// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

function sourceLinkBackHref(referrer, pageUrl) {
  if (!referrer || typeof referrer !== 'string') {
    return '/app/home/';
  }
  try {
    const refUrl = new URL(referrer);
    const targetUrl = new URL(pageUrl);
    if (refUrl.origin === targetUrl.origin) {
      return 'back';
    }
  } catch (_) {
    return '/app/home/';
  }
  return '/app/home/';
}

(function () {
  const link = document.getElementById('back-link');
  if (link) {
    link.addEventListener('click', function (e) {
      if (sourceLinkBackHref(document.referrer, window.location.href) === 'back') {
        e.preventDefault();
        history.back();
      }
    });
  }
})();
