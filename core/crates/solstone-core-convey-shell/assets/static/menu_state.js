// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Restore sidebar state before first paint to prevent FOUC.
(function () {
  try {
    var s = localStorage.getItem('solstone:menu-state');
    if (s === 'full') document.body.classList.add('menu-full');
    else if (s === 'all') document.body.classList.add('menu-all');
  } catch (e) {
    // Default collapsed menu state is safe when storage is unavailable.
  }
})();
