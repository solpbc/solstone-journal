// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(() => {
  'use strict';

  const STRATEGIES = Object.freeze({
    NATIVE_BROKER: 'native_broker',
    BYO_HTTPS: 'byo_https',
    JOURNAL_LOOPBACK: 'journal_loopback',
    DEVICE_CODE: 'device_code',
    PASTE_URL: 'paste_url',
  });

  const MAX_PREFERENCES = 16;

  let activeHostSession = null;
  let idCounter = 0;

  function makeRequestId() {
    if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
      return crypto.randomUUID();
    }
    idCounter += 1;
    return `oauth-${Date.now()}-${idCounter}-${Math.random().toString(16).slice(2, 10)}`;
  }

  function getAuthContract() {
    const contract = window.solstoneAuthContract;
    if (!contract || typeof contract !== 'object') return null;
    if (contract.version !== 2) return null;
    if (
      typeof contract.capability !== 'string' ||
      typeof contract.prepare !== 'string' ||
      typeof contract.prepared !== 'string' ||
      typeof contract.authorize !== 'string' ||
      typeof contract.result !== 'string' ||
      typeof contract.cancel !== 'string' ||
      typeof contract.unsupported !== 'string'
    ) {
      return null;
    }
    return contract;
  }

  function getNativeAuthHost() {
    if (window.top !== window) return null;
    const contract = getAuthContract();
    if (!contract) return null;
    const host = window[contract.capability];
    if (!host || typeof host !== 'object') return null;
    if (host.version !== contract.version) return null;
    if (typeof host.send !== 'function' || !Array.isArray(host.types)) return null;
    if (
      !host.types.includes(contract.prepare) ||
      !host.types.includes(contract.authorize) ||
      !host.types.includes(contract.cancel)
    ) {
      return null;
    }
    return { host, contract };
  }

  function supportsNativeBroker() {
    return getNativeAuthHost() !== null;
  }

  function isLoopbackHostname(hostname) {
    if (typeof hostname !== 'string' || hostname === '') return false;
    const lower = hostname.toLowerCase();
    return (
      lower === '127.0.0.1' ||
      lower === 'localhost' ||
      lower === '::1' ||
      lower === '[::1]' ||
      lower.endsWith('.localhost')
    );
  }

  function isEmbeddedNativeHost() {
    const contract = getAuthContract();
    if (contract && typeof window[contract.capability] !== 'undefined') {
      return true;
    }
    if (
      window.solstoneOutside &&
      typeof window.solstoneOutside.capableHost === 'function' &&
      window.solstoneOutside.capableHost()
    ) {
      return true;
    }
    return false;
  }

  function isSameMachineBrowser() {
    if (isEmbeddedNativeHost()) return false;
    const hostname = window.location ? window.location.hostname : '';
    return isLoopbackHostname(hostname);
  }

  function closeReservation(reservation) {
    if (!reservation || reservation.kind !== 'popup' || !reservation.popup) return;
    try {
      if (!reservation.popup.closed && typeof reservation.popup.close === 'function') {
        reservation.popup.close();
      }
    } catch (_err) {}
  }

  function openAuthorizeUrl(url, reservation) {
    const outside = window.solstoneOutside;
    if (outside && reservation && reservation.kind !== 'blocked') {
      if (typeof outside.navigateReserved === 'function' && outside.navigateReserved(reservation, url)) {
        return true;
      }
    }
    closeReservation(reservation);
    if (outside && typeof outside.openDelayed === 'function') {
      return Boolean(outside.openDelayed(url));
    }
    try {
      const opened = window.open(url, '_blank', 'noopener');
      return Boolean(opened && !opened.closed);
    } catch (_err) {
      return false;
    }
  }

  function openExternalUrl(url) {
    const outside = window.solstoneOutside;
    if (outside && typeof outside.openDelayed === 'function') {
      return Boolean(outside.openDelayed(url));
    }
    try {
      const opened = window.open(url, '_blank', 'noopener');
      return Boolean(opened && !opened.closed);
    } catch (_err) {
      return false;
    }
  }

  async function sendHostCancel(entry) {
    if (!entry) return null;
    try {
      return await entry.host.send({
        type: entry.contract.cancel,
        id: makeRequestId(),
        session_id: entry.sessionId,
      });
    } catch (_err) {
      return null;
    }
  }

  async function cancelActive() {
    const entry = activeHostSession;
    activeHostSession = null;
    return sendHostCancel(entry);
  }

  function isAborted(signal) {
    return Boolean(signal && signal.aborted);
  }

  async function runNativeBrokerStep(options, skipped) {
    const native = getNativeAuthHost();
    if (!native) {
      skipped.push({ strategy: STRATEGIES.NATIVE_BROKER, reason: 'host_absent' });
      return null;
    }
    const preferences = options.preferences;
    if (!Array.isArray(preferences) || preferences.length === 0 || preferences.length > MAX_PREFERENCES) {
      skipped.push({ strategy: STRATEGIES.NATIVE_BROKER, reason: 'no_preferences' });
      return null;
    }

    const { host, contract } = native;
    const prepareId = makeRequestId();
    let prepared;
    try {
      prepared = await host.send({
        type: contract.prepare,
        id: prepareId,
        preferences,
      });
    } catch (_err) {
      skipped.push({ strategy: STRATEGIES.NATIVE_BROKER, reason: 'prepare_failed' });
      return null;
    }

    if (!prepared || typeof prepared !== 'object' || prepared.id !== prepareId) {
      skipped.push({ strategy: STRATEGIES.NATIVE_BROKER, reason: 'invalid_prepare_reply' });
      return null;
    }
    if (prepared.type === contract.unsupported) {
      skipped.push({ strategy: STRATEGIES.NATIVE_BROKER, reason: 'unsupported' });
      return null;
    }
    if (
      prepared.type !== contract.prepared ||
      prepared.outcome !== 'ready' ||
      typeof prepared.session_id !== 'string' ||
      prepared.session_id === '' ||
      typeof prepared.redirect_uri !== 'string' ||
      prepared.redirect_uri === ''
    ) {
      skipped.push({
        strategy: STRATEGIES.NATIVE_BROKER,
        reason: typeof prepared.outcome === 'string' ? prepared.outcome : 'not_ready',
      });
      return null;
    }

    const sessionEntry = {
      host,
      contract,
      sessionId: prepared.session_id,
    };
    activeHostSession = sessionEntry;

    let started;
    try {
      if (isAborted(options.signal)) {
        await cancelActive();
        return {
          ok: false,
          strategy: STRATEGIES.NATIVE_BROKER,
          brokerMode: prepared.mode || null,
          outcome: 'cancelled',
          reason: null,
          skipped,
        };
      }
      started = await options.start({
        strategy: STRATEGIES.NATIVE_BROKER,
        brokerMode: prepared.mode || null,
        redirectUri: prepared.redirect_uri,
        signal: options.signal || null,
      });
    } catch (err) {
      if (activeHostSession === sessionEntry) {
        await cancelActive();
      }
      throw err;
    }

    if (!started || started.available === false) {
      if (activeHostSession === sessionEntry) {
        await cancelActive();
      }
      skipped.push({ strategy: STRATEGIES.NATIVE_BROKER, reason: 'start_unavailable' });
      return null;
    }

    if (
      typeof started.authorizeUrl !== 'string' ||
      started.authorizeUrl === '' ||
      typeof started.expectedState !== 'string' ||
      started.expectedState === ''
    ) {
      if (activeHostSession === sessionEntry) {
        await cancelActive();
      }
      throw new Error('oauth_broker_invalid_start_payload');
    }

    const abortHandler = () => {
      if (activeHostSession === sessionEntry) {
        void cancelActive();
      }
    };
    if (options.signal && typeof options.signal.addEventListener === 'function') {
      options.signal.addEventListener('abort', abortHandler, { once: true });
      if (options.signal.aborted) {
        abortHandler();
      }
    }

    const authorizeId = makeRequestId();
    const authorizeReq = {
      type: contract.authorize,
      id: authorizeId,
      session_id: prepared.session_id,
      authorize_url: started.authorizeUrl,
      expected_state: started.expectedState,
    };
    if (typeof started.timeoutSeconds === 'number') {
      authorizeReq.timeout_seconds = started.timeoutSeconds;
    }

    let authReply;
    try {
      authReply = await host.send(authorizeReq);
    } finally {
      if (options.signal && typeof options.signal.removeEventListener === 'function') {
        options.signal.removeEventListener('abort', abortHandler);
      }
      if (activeHostSession === sessionEntry) {
        activeHostSession = null;
      }
    }

    if (
      authReply &&
      typeof authReply === 'object' &&
      authReply.id === authorizeId &&
      authReply.type === contract.result &&
      authReply.outcome === 'callback' &&
      typeof authReply.callback_url === 'string' &&
      authReply.callback_url !== ''
    ) {
      const finishResult = await options.finish({
        strategy: STRATEGIES.NATIVE_BROKER,
        brokerMode: prepared.mode || null,
        attemptId: started.attemptId || null,
        callbackUrl: authReply.callback_url,
        started,
        signal: options.signal || null,
      });
      return {
        ok: true,
        strategy: STRATEGIES.NATIVE_BROKER,
        brokerMode: prepared.mode || null,
        started,
        result: finishResult,
        skipped,
      };
    }

    return {
      ok: false,
      strategy: STRATEGIES.NATIVE_BROKER,
      brokerMode: prepared.mode || null,
      outcome:
        authReply && typeof authReply.outcome === 'string' ? authReply.outcome : 'error',
      reason:
        authReply && typeof authReply.reason === 'string' ? authReply.reason : null,
      skipped,
    };
  }

  async function run(options) {
    if (!options || typeof options !== 'object') {
      throw new TypeError('oauth_broker_options_required');
    }
    if (typeof options.start !== 'function') {
      throw new TypeError('oauth_broker_start_adapter_required');
    }
    if (typeof options.finish !== 'function') {
      throw new TypeError('oauth_broker_finish_adapter_required');
    }

    const signal = options.signal || null;
    const skipped = [];
    const sameMachine =
      typeof options.sameMachine === 'boolean'
        ? options.sameMachine
        : isSameMachineBrowser();
    const canByoHttps = Boolean(options.byoHttps) || typeof options.startByoHttps === 'function';
    const canJournalLoopback = options.allowJournalLoopback !== false && sameMachine;
    const canDeviceCode =
      options.allowDeviceCode !== false && typeof options.onDeviceCode === 'function';
    const canPasteFallback = typeof options.onPasteFallback === 'function';

    // Reserve a blank browser tab synchronously on click when falling directly into a
    // plain-browser URL step so popup blockers do not block the post-await navigation.
    let reservation = null;
    const willTryNative =
      supportsNativeBroker() &&
      Array.isArray(options.preferences) &&
      options.preferences.length > 0;
    const wantsImmediateBrowserPopup =
      !willTryNative &&
      (canByoHttps ||
        canJournalLoopback ||
        (!canDeviceCode && canPasteFallback && options.openAuthorizeUrl !== false));
    if (
      wantsImmediateBrowserPopup &&
      window.solstoneOutside &&
      typeof window.solstoneOutside.reserveBlank === 'function'
    ) {
      reservation = window.solstoneOutside.reserveBlank();
    }

    try {
      // Step 1: Native app broker (window.solstoneJournalHost).
      const nativeResult = await runNativeBrokerStep(options, skipped);
      if (nativeResult !== null) {
        closeReservation(reservation);
        return nativeResult;
      }

      if (isAborted(signal)) {
        closeReservation(reservation);
        return { ok: false, strategy: null, outcome: 'cancelled', skipped };
      }

      // Step 2: Journal BYO HTTPS callback.
      if (canByoHttps) {
        const started = await (options.startByoHttps
          ? options.startByoHttps({ strategy: STRATEGIES.BYO_HTTPS, signal })
          : options.start({ strategy: STRATEGIES.BYO_HTTPS, signal }));
        if (started && started.available !== false) {
          const opened = openAuthorizeUrl(started.authorizeUrl, reservation);
          reservation = null;
          const result =
            typeof options.waitForCompletion === 'function'
              ? await options.waitForCompletion({
                  strategy: STRATEGIES.BYO_HTTPS,
                  attemptId: started.attemptId || null,
                  started,
                  opened,
                  signal,
                })
              : null;
          return {
            ok: true,
            strategy: STRATEGIES.BYO_HTTPS,
            opened,
            started,
            result,
            skipped,
          };
        }
        skipped.push({ strategy: STRATEGIES.BYO_HTTPS, reason: 'unavailable' });
      } else {
        skipped.push({ strategy: STRATEGIES.BYO_HTTPS, reason: 'not_configured' });
      }

      if (isAborted(signal)) {
        closeReservation(reservation);
        return { ok: false, strategy: null, outcome: 'cancelled', skipped };
      }

      // Step 3: Journal-direct loopback (same-machine plain browser).
      if (canJournalLoopback) {
        const started = await options.start({
          strategy: STRATEGIES.JOURNAL_LOOPBACK,
          signal,
        });
        if (started && started.available !== false) {
          const opened = openAuthorizeUrl(started.authorizeUrl, reservation);
          reservation = null;
          const result =
            typeof options.waitForCompletion === 'function'
              ? await options.waitForCompletion({
                  strategy: STRATEGIES.JOURNAL_LOOPBACK,
                  attemptId: started.attemptId || null,
                  started,
                  opened,
                  signal,
                })
              : null;
          if (!result || result.fallbackToPaste !== true) {
            return {
              ok: true,
              strategy: STRATEGIES.JOURNAL_LOOPBACK,
              opened,
              started,
              result,
              skipped,
            };
          }
          skipped.push({
            strategy: STRATEGIES.JOURNAL_LOOPBACK,
            reason: 'fallback_to_paste',
          });
        } else {
          skipped.push({ strategy: STRATEGIES.JOURNAL_LOOPBACK, reason: 'unavailable' });
        }
      } else {
        skipped.push({
          strategy: STRATEGIES.JOURNAL_LOOPBACK,
          reason: sameMachine ? 'disabled' : 'not_same_machine',
        });
      }

      if (isAborted(signal)) {
        closeReservation(reservation);
        return { ok: false, strategy: null, outcome: 'cancelled', skipped };
      }

      // Step 4: Device Code flow (RFC 8628).
      if (canDeviceCode) {
        closeReservation(reservation);
        reservation = null;
        const started = await (options.startDeviceCode
          ? options.startDeviceCode({ strategy: STRATEGIES.DEVICE_CODE, signal })
          : options.start({ strategy: STRATEGIES.DEVICE_CODE, signal }));
        if (started && started.available !== false) {
          const uiResult = await options.onDeviceCode({
            attemptId: started.attemptId || null,
            userCode: started.userCode || null,
            verificationUri: started.verificationUri || null,
            verificationUriComplete: started.verificationUriComplete || null,
            expiresIn: started.expiresIn ?? null,
            interval: started.interval ?? null,
            started,
            openUrl: openExternalUrl,
            signal,
          });
          if (isAborted(signal) || (uiResult && uiResult.outcome === 'cancelled')) {
            return {
              ok: false,
              strategy: STRATEGIES.DEVICE_CODE,
              outcome: 'cancelled',
              skipped,
            };
          }
          const result =
            typeof options.waitForCompletion === 'function'
              ? await options.waitForCompletion({
                  strategy: STRATEGIES.DEVICE_CODE,
                  attemptId: started.attemptId || null,
                  started,
                  signal,
                })
              : uiResult ?? null;
          return {
            ok: true,
            strategy: STRATEGIES.DEVICE_CODE,
            started,
            result,
            skipped,
          };
        }
        skipped.push({ strategy: STRATEGIES.DEVICE_CODE, reason: 'unavailable' });
      } else {
        skipped.push({ strategy: STRATEGIES.DEVICE_CODE, reason: 'not_supported' });
      }

      if (isAborted(signal)) {
        closeReservation(reservation);
        return { ok: false, strategy: null, outcome: 'cancelled', skipped };
      }

      // Step 5: Paste-redirect-URL fallback.
      if (canPasteFallback) {
        const started = await (options.startPaste
          ? options.startPaste({ strategy: STRATEGIES.PASTE_URL, signal })
          : options.start({ strategy: STRATEGIES.PASTE_URL, signal }));
        if (started && started.available !== false) {
          let opened = false;
          if (started.authorizeUrl && options.openAuthorizeUrl !== false) {
            opened = openAuthorizeUrl(started.authorizeUrl, reservation);
            reservation = null;
          } else {
            closeReservation(reservation);
            reservation = null;
          }

          let finished = false;
          let finishedValue = null;
          const submitCallbackUrl = async (callbackUrl) => {
            const out = await options.finish({
              strategy: STRATEGIES.PASTE_URL,
              attemptId: started.attemptId || null,
              callbackUrl,
              started,
              signal,
            });
            finished = true;
            finishedValue = out;
            return out;
          };

          const pasteResult = await options.onPasteFallback({
            attemptId: started.attemptId || null,
            authorizeUrl: started.authorizeUrl || null,
            expectedState: started.expectedState || null,
            redirectUri: started.redirectUri || null,
            opened,
            started,
            submitCallbackUrl,
            openUrl: openExternalUrl,
            signal,
          });

          if (finished) {
            return {
              ok: true,
              strategy: STRATEGIES.PASTE_URL,
              opened,
              started,
              result: finishedValue,
              skipped,
            };
          }
          if (isAborted(signal) || (pasteResult && pasteResult.outcome === 'cancelled')) {
            return {
              ok: false,
              strategy: STRATEGIES.PASTE_URL,
              outcome: 'cancelled',
              skipped,
            };
          }
          if (pasteResult && typeof pasteResult.callbackUrl === 'string' && pasteResult.callbackUrl !== '') {
            const out = await submitCallbackUrl(pasteResult.callbackUrl);
            return {
              ok: true,
              strategy: STRATEGIES.PASTE_URL,
              opened,
              started,
              result: out,
              skipped,
            };
          }
          return {
            ok: false,
            strategy: STRATEGIES.PASTE_URL,
            outcome:
              pasteResult && typeof pasteResult.outcome === 'string'
                ? pasteResult.outcome
                : 'cancelled',
            skipped,
          };
        }
        skipped.push({ strategy: STRATEGIES.PASTE_URL, reason: 'unavailable' });
      } else {
        skipped.push({ strategy: STRATEGIES.PASTE_URL, reason: 'not_configured' });
      }

      closeReservation(reservation);
      reservation = null;
      return {
        ok: false,
        strategy: null,
        outcome: 'unavailable',
        skipped,
      };
    } catch (err) {
      closeReservation(reservation);
      throw err;
    }
  }

  if (typeof window.addEventListener === 'function') {
    window.addEventListener('pagehide', () => {
      if (activeHostSession) {
        void cancelActive();
      }
    });
  }

  window.solstoneOAuthBroker = Object.freeze({
    STRATEGIES,
    supportsNativeBroker,
    isSameMachineBrowser,
    cancelActive,
    run,
  });
})();
