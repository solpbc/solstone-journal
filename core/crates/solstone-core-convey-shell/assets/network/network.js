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

  function initPairingCeremony(options = {}) {
    const documentRef = options.document || global.document;
    const root = findById(documentRef, 'link-workspace-root');
    const dialog = findById(documentRef, 'link-pairing-dialog');
    if (!root || !dialog || typeof global.fetch !== 'function') return null;

    const elements = {
      starting: findById(documentRef, 'link-pairing-starting'),
      material: findById(documentRef, 'link-pairing-material'),
      expired: findById(documentRef, 'link-pairing-expired'),
      unavailable: findById(documentRef, 'link-pairing-unavailable'),
      windowClosed: findById(documentRef, 'link-pairing-window-closed'),
      complete: findById(documentRef, 'link-pairing-complete'),
      label: findById(documentRef, 'link-device-label'),
      labelRow: findById(documentRef, 'link-pairing-label-row'),
      networkLine: findById(documentRef, 'link-pairing-network-line'),
      fingerprint: findById(documentRef, 'link-pairing-fingerprint'),
      linkValue: findById(documentRef, 'link-pairing-link-value'),
      qr: findById(documentRef, 'link-pairing-qr'),
      check: findById(documentRef, 'link-pairing-check'),
      successHeading: findById(documentRef, 'link-pairing-success-heading'),
      successSubhead: findById(documentRef, 'link-pairing-success-subhead'),
    };
    if (Object.values(elements).some((element) => !element)) return null;

    const ceremony = {
      generation: 0,
      state: 'idle',
      material: null,
      timer: null,
      unsubscribe: null,
      openerSelector: '[data-pairing-action="open"]',
    };

    function copy(key) {
      return resolve(global.LinkCopy, key);
    }

    function prefix() {
      return global.location?.pathname?.startsWith('/app/link') ? '/app/link' : '/app/network';
    }

    function clearTimer() {
      if (ceremony.timer !== null) {
        global.clearTimeout(ceremony.timer);
        ceremony.timer = null;
      }
    }

    function clearSubscription() {
      if (typeof ceremony.unsubscribe === 'function') ceremony.unsubscribe();
      ceremony.unsubscribe = null;
    }

    function startGeneration() {
      ceremony.generation += 1;
      clearTimer();
      clearSubscription();
      ceremony.material = null;
      return ceremony.generation;
    }

    function displayState(next, recovery = '') {
      ceremony.state = next;
      elements.starting.hidden = next !== 'starting';
      elements.material.hidden = next !== 'material';
      elements.expired.hidden = next !== 'expired';
      elements.unavailable.hidden = next !== 'unavailable' || recovery === 'window-closed';
      elements.windowClosed.hidden = next !== 'unavailable' || recovery !== 'window-closed';
      elements.complete.hidden = next !== 'complete';
      elements.check.hidden = next !== 'material';
      if (next !== 'material') setCheckBusy(false);
      dialog.dataset.pairingState = next;
    }

    function setCheckBusy(busy) {
      elements.check.disabled = busy;
      if (busy) elements.check.setAttribute('aria-busy', 'true');
      else elements.check.removeAttribute('aria-busy');
    }

    function formatExpiry(value) {
      const seconds = Math.max(0, Math.ceil(value));
      return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, '0')}`;
    }

    function defaultDeviceLabel() {
      const today = new Date();
      const month = today.toLocaleString('en-US', { month: 'short' }).toLowerCase();
      return copy('DEVICE_LABEL_DEFAULT_FORMAT')
        .replace('{month}', month)
        .replace('{day}', String(today.getDate()));
    }

    function renderCode(pairLink) {
      const qr = global.qrcode(0, 'M');
      const split = pairLink.indexOf('#');
      const prefixPart = split >= 0 ? pairLink.slice(0, split + 1) : pairLink;
      const payloadPart = split >= 0 ? pairLink.slice(split + 1) : '';
      qr.addData(prefixPart, 'Byte');
      qr.addData(payloadPart, 'Alphanumeric');
      qr.make();
      elements.qr.innerHTML = qr.createSvgTag({
        cellSize: 8,
        margin: 2,
        title: 'pair QR',
        alt: 'pair QR',
      });
      elements.qr.querySelector('svg')?.setAttribute('data-module-count', qr.getModuleCount());
    }

    function validMaterial(body) {
      return Boolean(
        body
        && typeof body === 'object'
        && !Array.isArray(body)
        && typeof body.nonce === 'string'
        && body.nonce.trim()
        && typeof body.pair_link === 'string'
        && body.pair_link
        && typeof body.ca_fingerprint === 'string'
        && body.ca_fingerprint
        && Number.isFinite(body.expires_in)
        && body.expires_in > 0
        && (body.device_label === undefined || typeof body.device_label === 'string')
        && typeof global.qrcode === 'function'
      );
    }

    function renderMaterial(body) {
      const label = typeof body.device_label === 'string' ? body.device_label.trim() : '';
      elements.label.textContent = label;
      elements.labelRow.hidden = !label;
      elements.networkLine.textContent = copy('PAIR_NETWORK_LINE').replace('{time}', formatExpiry(body.expires_in));
      elements.fingerprint.textContent = body.ca_fingerprint;
      elements.linkValue.textContent = body.pair_link;
      renderCode(body.pair_link);
    }

    function renderCompletion() {
      const label = ceremony.material?.device_label?.trim() || '';
      const shortFingerprint = ceremony.material?.ca_fingerprint?.replace(/^sha256:/, '').slice(0, 16) || '';
      elements.successHeading.textContent = copy('SUCCESS_HEADING').replace('{label}', label);
      elements.successSubhead.textContent = copy('SUCCESS_SUBHEAD').replace('{short_fp}', shortFingerprint);
    }

    function openRecovery(kind) {
      clearTimer();
      clearSubscription();
      ceremony.material = null;
      displayState('unavailable', kind);
    }

    async function responseBody(response) {
      try {
        return await response.json();
      } catch (_error) {
        return null;
      }
    }

    function armExpiry(generation, seconds) {
      ceremony.timer = global.setTimeout(() => {
        if (generation !== ceremony.generation || ceremony.state !== 'material') return;
        clearSubscription();
        ceremony.material = null;
        displayState('expired');
      }, seconds * 1000);
    }

    function listenForCompletion(generation) {
      if (!global.appEvents || typeof global.appEvents.listen !== 'function') return;
      ceremony.unsubscribe = global.appEvents.listen('link', () => {
        if (generation !== ceremony.generation || ceremony.state !== 'material') return;
        checkCurrentNonce(generation);
      });
    }

    async function checkCurrentNonce(generation = ceremony.generation, reportBusy = false) {
      const nonce = ceremony.material?.nonce;
      if (!nonce || generation !== ceremony.generation || ceremony.state !== 'material') return;
      if (reportBusy) setCheckBusy(true);
      let response;
      let body;
      try {
        response = await global.fetch(`${prefix()}/api/pair/nonce-status?nonce=${encodeURIComponent(nonce)}`, {
          headers: { accept: 'application/json' },
        });
        body = await responseBody(response);
      } catch (_error) {
        if (generation !== ceremony.generation) return;
        openRecovery('unavailable');
        return;
      } finally {
        if (reportBusy && generation === ceremony.generation && ceremony.state === 'material') {
          setCheckBusy(false);
        }
      }
      if (generation !== ceremony.generation) return;
      if (!response.ok || !body || typeof body.present !== 'boolean' || typeof body.used !== 'boolean') {
        openRecovery('unavailable');
        return;
      }
      if (body.used) {
        clearTimer();
        clearSubscription();
        renderCompletion();
        displayState('complete');
      } else if (!body.present) {
        clearTimer();
        clearSubscription();
        ceremony.material = null;
        openRecovery('window-closed');
      }
    }

    async function requestMaterial(generation) {
      displayState('starting');
      let response;
      let body;
      try {
        response = await global.fetch(`${prefix()}/pair-start`, {
          method: 'POST',
          headers: { 'content-type': 'application/json', accept: 'application/json' },
          body: JSON.stringify({ device_label: defaultDeviceLabel() }),
        });
        body = await responseBody(response);
      } catch (_error) {
        if (generation !== ceremony.generation) return;
        openRecovery('unavailable');
        return;
      }
      if (generation !== ceremony.generation) return;
      if (!response.ok || !validMaterial(body)) {
        openRecovery('unavailable');
        return;
      }
      ceremony.material = body;
      renderMaterial(body);
      displayState('material');
      armExpiry(generation, body.expires_in);
      listenForCompletion(generation);
    }

    function open() {
      options.beforeOpen?.();
      const generation = startGeneration();
      dialog.hidden = false;
      requestMaterial(generation);
    }

    function regenerate() {
      if (dialog.hidden) return;
      const generation = startGeneration();
      requestMaterial(generation);
    }

    function restoreOpenerFocus() {
      global.requestAnimationFrame(() => {
        const opener = root.querySelector(ceremony.openerSelector);
        if (opener && opener.isConnected && typeof opener.focus === 'function') {
          opener.focus();
        } else if (typeof root.focus === 'function') {
          root.focus();
        }
      });
    }

    function close({ restoreFocus = true } = {}) {
      startGeneration();
      displayState('idle');
      dialog.hidden = true;
      if (restoreFocus) restoreOpenerFocus();
    }

    async function copyLink() {
      const value = ceremony.material?.pair_link;
      if (!value) return;
      let copied = false;
      try {
        copied = Boolean(await options.clipboardWriteText?.(value));
      } catch (error) {
        global.logError?.(error, { context: 'link: pairing-link clipboard write failed' });
      }
      options.showToast?.(copy(copied ? 'PAIR_LINK_COPY_SUCCESS_TOAST' : 'PAIR_LINK_COPY_FAIL_TOAST'));
    }

    root.addEventListener('click', (event) => {
      const control = event.target?.closest?.('[data-pairing-action]');
      if (!control || !root.contains(control)) return;
      const action = control.dataset.pairingAction;
      if (action === 'open') {
        ceremony.openerSelector = '[data-pairing-action="open"]';
        open();
      } else if (action === 'close') {
        close();
      } else if (action === 'regenerate') {
        regenerate();
      } else if (action === 'check') {
        if (!control.disabled) checkCurrentNonce(ceremony.generation, true);
      } else if (action === 'copy') {
        copyLink();
      }
    });

    documentRef.addEventListener('keydown', (event) => {
      if (event.key === 'Escape' && !dialog.hidden) {
        event.preventDefault();
        close();
      }
    });

    return { close };
  }

  // ── delivery vs connection ────────────────────────────────────────────────
  // A paired device carries two independent measurements: whether material is
  // arriving (capture_state / last_accepted_ingest_at) and whether a heartbeat
  // is fresh (state / last_seen_at). The card leads with delivery, because that
  // is what the owner is asking about; the heartbeat is a secondary line.
  const DELIVERY_GROUP_ORDER = ['failing', 'adding', 'recent', 'quiet', 'never', 'unknown'];
  const DELIVERY_GROUP_LABELS = {
    failing: 'not adding right now',
    adding: 'adding to your journal now',
    recent: 'added recently',
    quiet: 'quiet for a while',
    never: 'nothing added yet',
    unknown: 'delivery unavailable',
  };
  const DELIVERY_GROUP_CHIP_CLASS = {
    failing: 'stale',
    adding: 'connected',
    recent: 'neutral',
    quiet: 'neutral',
    never: 'neutral',
    unknown: 'neutral',
  };
  const CONNECTION_LINE_LABELS = {
    connected: 'connected now',
    stale: 'connection not reporting',
    disconnected: 'not connected right now',
  };

  function deliveryGroupFor(client) {
    if (!client || typeof client !== 'object') return 'unknown';
    if (client.failing || client.capture_state === 'degraded') return 'failing';
    if (client.capture_state === 'active') return 'adding';
    if (client.capture_state === 'stale') return 'recent';
    if (client.capture_state === 'unknown') return 'unknown';
    return client.last_accepted_ingest_at ? 'quiet' : 'never';
  }

  function deliveryChipClass(client) {
    return DELIVERY_GROUP_CHIP_CLASS[deliveryGroupFor(client)];
  }

  function elapsedSince(timestamp, nowMs) {
    const parsed = Date.parse(timestamp);
    if (!Number.isFinite(parsed)) return null;
    const now = Number.isFinite(nowMs) ? nowMs : Date.now();
    return Math.max(0, now - parsed);
  }

  function deliveryChipLabel(client, nowMs) {
    const group = deliveryGroupFor(client);
    if (group === 'failing') return 'delivery problem';
    if (group === 'adding') return 'adding now';
    if (group === 'never') return 'nothing added yet';
    if (group === 'unknown') return 'delivery unavailable';
    const elapsed = elapsedSince(client.last_accepted_ingest_at, nowMs);
    return elapsed === null ? 'added earlier' : `added ${global.relativeTime(elapsed)} ago`;
  }

  function connectionLineLabel(client) {
    if (!client || typeof client !== 'object') return 'connection unavailable';
    return CONNECTION_LINE_LABELS[client.state] || 'connection unavailable';
  }

  function checkInLabel(client, nowMs) {
    const elapsed = elapsedSince(client.last_seen_at, nowMs);
    return elapsed === null ? 'never' : `${global.relativeTime(elapsed)} ago`;
  }

  /// Local calendar day of an instant, as the YYYYMMDD key JournalFormat.day reads.
  function dayKeyFor(timestamp) {
    if (!timestamp) return '';
    const value = new Date(timestamp);
    if (Number.isNaN(value.getTime())) return '';
    const month = String(value.getMonth() + 1).padStart(2, '0');
    const day = String(value.getDate()).padStart(2, '0');
    return `${value.getFullYear()}${month}${day}`;
  }

  function groupClientsByDelivery(clients) {
    const groups = new Map(DELIVERY_GROUP_ORDER.map((group) => [group, []]));
    for (const client of clients || []) {
      groups.get(deliveryGroupFor(client)).push(client);
    }
    return DELIVERY_GROUP_ORDER.filter((group) => groups.get(group).length).map((group) => ({
      group,
      label: DELIVERY_GROUP_LABELS[group],
      clients: groups.get(group),
    }));
  }

  const NetworkRender = {
    applyCopy,
    resolve,
    initPairingCeremony,
    DELIVERY_GROUP_ORDER,
    DELIVERY_GROUP_LABELS,
    deliveryGroupFor,
    deliveryChipClass,
    deliveryChipLabel,
    connectionLineLabel,
    checkInLabel,
    dayKeyFor,
    groupClientsByDelivery,
  };
  global.NetworkRender = NetworkRender;
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = NetworkRender;
  }
})(typeof window !== 'undefined' ? window : globalThis);
