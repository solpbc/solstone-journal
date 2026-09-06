// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(() => {
  const state = {
    providers: {},
    keys: {},
    localModels: [],
    localAvailability: null,
    install: null,
    installPollGeneration: 0,
    runtimePollGeneration: 0,
    confidentialPollGeneration: 0,
    confidentialDetailOpen: false,
    selectedByoProvider: '',
    byoMode: 'pick',
    byoSelectedModel: '',
    byoCustomOpen: false,
    byoCustomModel: '',
    byoCustomCheckedModel: '',
    byoModelResolutionTargets: [],
    pendingSwitchTarget: '',
    runsNavigationGeneration: 0,
    runsRouteKey: '',
    runsLastHash: '',
    runsRequestGenerations: {day: 0, run: 0, prompt: 0, output: 0},
    runsInFlight: {day: null, run: null, prompt: null, output: null},
    runsCache: {
      day: new Map(),
      run: new Map(),
      prompt: new Map(),
      output: new Map(),
    },
    runsFailuresOnly: false,
    runsGroupKey: '',
    runsGroupOpen: new Map(),
    runsGroupShown: new Map(),
    runsFacet: '',
    runsFacetExplicit: false,
    runsSelectedUseId: '',
    runsDetail: null,
    runsModalFocus: null,
    runsPromptEscapeHandler: null,
  };
  let copy = {};
  const confidentialTerminalPhases = new Set(['not_verified', 'repair_needed', 'early_access']);
  const installInFlightStates = new Set(['resolving', 'downloading', 'verifying', 'installing']);
  const installTerminalStates = new Set(['idle', 'installed', 'failed']);
  const localSetupMissingReasons = new Set(['local_model_missing', 'model_missing', 'binary_missing', 'runtime_missing']);
  const localServerUnhealthyReasons = new Set(['local_server_unhealthy', 'server_unhealthy']);
  const pollIntervalMs = 1500;
  const confidentialPollMaxMs = 15 * 60 * 1000;
  const views = new Set(['main', 'byo-setup', 'confidential-setup', 'local-setup', 'lane-switch']);
  // A day of ordinary processing is well over a thousand runs. Group them by
  // talent and page each group; a light day still renders in full.
  const thinkingRunsPageSize = 50;
  const thinkingRunsExpandAllBelow = 50;
  // Readable names for the talent ids that do not humanize cleanly. Anything
  // else falls back to the humanized id; the exact id stays in the disclosure.
  const talentLabels = {
    'entities:detection': 'entity detection',
  };
  const providerEnv = {
    anthropic: 'ANTHROPIC_API_KEY',
    google: 'GOOGLE_API_KEY',
    openai: 'OPENAI_API_KEY',
  };
  const fallbackProviderLabels = {
    anthropic: 'Claude',
    google: 'Gemini',
    openai: 'GPT',
    local: 'Local',
  };
  let providerLabels = fallbackProviderLabels;
  const providerTerms = {
    anthropic: 'https://www.anthropic.com/legal/commercial-terms',
    google: 'https://ai.google.dev/gemini-api/terms',
    openai: 'https://openai.com/policies/row-terms-of-use',
  };
  const googleModelResolutionTargetsField = 'google_model_resolution_targets';

  function $(id) {
    return document.getElementById(id);
  }

  function setText(id, message) {
    const el = $(id);
    if (!el) return;
    el.textContent = message || '';
  }

  function setMessage(id, message, tone = '') {
    const el = $(id);
    if (!el) return;
    el.textContent = message || '';
    if (tone) {
      el.dataset.tone = tone;
    } else {
      el.removeAttribute('data-tone');
    }
  }

  function setLink(id, url, text) {
    const el = $(id);
    if (!el) return;
    el.hidden = !url;
    el.href = url || '';
    el.textContent = url ? text : '';
  }

  function setHidden(id, hidden) {
    const el = $(id);
    if (!el) return;
    el.hidden = !!hidden;
  }

  function setButtonState(id, visible, disabled) {
    const button = $(id);
    if (!button) return;
    button.hidden = !visible;
    button.disabled = !!disabled;
  }

  function setButtonText(id, text) {
    const button = $(id);
    if (!button) return;
    button.textContent = text || '';
  }

  function installIsInFlight(status) {
    return installInFlightStates.has(status?.install_state || '');
  }

  function installIsTerminal(status) {
    return installTerminalStates.has(status?.install_state || '');
  }

  function formatInstallBytes(received, total) {
    if (received === null || received === undefined || total === null || total === undefined) return '';
    const gb = 1024 * 1024 * 1024;
    return `${(Number(received) / gb).toFixed(1)} GB of ${(Number(total) / gb).toFixed(1)} GB`;
  }

  function installCopyForStatus(status, text) {
    const phase = status?.install_state || '';
    const phaseLabel = text?.phases?.[phase] || phase;
    if (installInFlightStates.has(phase)) {
      const bytesLine = formatInstallBytes(status.progress_bytes_received, status.progress_bytes_total);
      return {
        pill: text?.pill_inflight || '',
        title: 'local',
        sub: phaseLabel,
        message: bytesLine || phaseLabel,
        notice: text?.notice_inflight || '',
        activate: false,
        bootstrap: false,
        bootstrapLabel: text?.install || '',
        tone: '',
      };
    }
    if (phase === 'failed') {
      return {
        pill: text?.pill_failed || '',
        title: 'local',
        sub: text?.failed_verdict || '',
        message: '',
        notice: text?.failed_reason || '',
        activate: false,
        bootstrap: true,
        bootstrapLabel: text?.retry || '',
        tone: 'bad',
      };
    }
    return null;
  }

  function localRuntimeCopy(runtime, active, text) {
    if (!runtime) return null;
    const states = text?.states || {};
    const view = (key, options = {}) => {
      const value = states[key] || {};
      return {
        pill: value.pill || '',
        title: 'local',
        sub: value.verdict || '',
        message: '',
        notice: value.reason || '',
        activate: false,
        bootstrap: false,
        retryRuntime: !!options.retryRuntime,
        retryRuntimeLabel: text?.retry || '',
        tone: options.tone || '',
      };
    };

    if (runtime.status === 'corrupt' || runtime.phase === 'state-corrupt') {
      return view('corrupt', {tone: 'bad'});
    }
    if (runtime.status === 'unavailable' || runtime.phase === 'state-unavailable') {
      return view('unavailable', {tone: 'bad'});
    }
    if (runtime.status === 'stale' || runtime.phase === 'state-stale') {
      return view('stale', {tone: 'bad'});
    }
    if (!active) {
      return runtime.phase === 'cleanup-failed'
        ? view('cleanup_failed', {tone: 'bad'})
        : null;
    }

    if (runtime.phase === 'ready') return view('ready');
    if (runtime.phase === 'ready-proof-unavailable') {
      return view('ready_proof_unavailable');
    }
    if (runtime.phase === 'starting' || runtime.phase === 'warming') {
      return view('starting');
    }
    if (runtime.phase === 'backoff') return view('recovering');
    if (runtime.phase === 'retry-requested') return view('retrying');
    if (runtime.phase === 'host-blocked') {
      if (runtime.reason_code === 'platform-unsupported' || runtime.reason_code === 'package-unavailable') {
        return view('unsupported', {tone: 'bad'});
      }
      return view('waiting');
    }
    if (runtime.phase === 'failed') {
      return view('failed', {
        retryRuntime: runtime.can_retry === true,
        tone: 'bad',
      });
    }
    if (runtime.phase === 'stop-deferred' || runtime.phase === 'stopping') {
      return view('changing');
    }
    if (runtime.phase === 'cleanup-failed') {
      return view('cleanup_failed', {tone: 'bad'});
    }
    if (runtime.phase === 'artifact-not-ready') return null;
    // Disposition: a stopped runtime carries no diagnosis of its own; defer to the
    // readiness chain, which knows whether local is blocked and why.
    if (runtime.phase === 'stopped') return null;
    return view('checking');
  }

  async function pollLocalInstallUntilTerminal({
    fetchStatus,
    sleepFn,
    applyStatus,
    isCurrent,
    intervalMs,
    initialStatus = null,
  }) {
    if (installTerminalStates.has(initialStatus?.install_state || '')) return initialStatus;
    if (installInFlightStates.has(initialStatus?.install_state || '')) {
      if (!isCurrent()) return null;
      await sleepFn(intervalMs);
    }
    while (isCurrent()) {
      const status = await fetchStatus();
      applyStatus(status);
      if (installTerminalStates.has(status?.install_state || '')) return status;
      await sleepFn(intervalMs);
    }
    return null;
  }

  async function pollLocalRuntimeUntilStable({
    fetchStatus,
    sleepFn,
    applyStatus,
    isCurrent,
    intervalMs,
    initialStatus = null,
  }) {
    let status = initialStatus;
    while (isCurrent() && status?.poll === true) {
      await sleepFn(intervalMs);
      if (!isCurrent()) return null;
      status = await fetchStatus();
      applyStatus(status);
    }
    return status;
  }

  function handleInstallPollError({
    generation,
    currentGeneration,
    clearInstallStatus,
    stopPoll,
    showError,
    error,
  }) {
    if (generation !== currentGeneration()) return false;
    clearInstallStatus();
    stopPoll();
    showError(error?.message || '');
    return true;
  }

  function confidentialOperationIsTerminal(operation) {
    return !operation || confidentialTerminalPhases.has(operation.phase || '');
  }

  function confidentialEnableNeedsRecheck(activeLane) {
    return activeLane?.confidential_enabled === true
      && activeLane?.confidential_provenance_configured === true
      && activeLane?.confidential_operation?.phase === 'not_verified';
  }

  function confidentialOperationRender(operation, text) {
    const phase = operation?.phase || '';
    const states = text?.operation_states || {};
    if (phase === 'starting' || phase === 'waiting') {
      return {message: states[phase] || '', tone: ''};
    }
    if (phase === 'early_access') {
      return {message: states.early_access || '', tone: ''};
    }
    if (phase === 'not_verified') {
      return {message: '', tone: ''};
    }
    if (phase === 'repair_needed') {
      return {message: states.repair_needed || '', tone: 'error'};
    }
    return {message: operation?.guidance || '', tone: ''};
  }

  function confidentialSetupMetaLine(attestation, lastCheckedLabel = '') {
    const stateName = attestation?.state || '';
    const blocked = stateName === 'failed' || stateName === 'stale' || stateName === 'unreachable';
    if (!blocked || !lastCheckedLabel) return '';
    return `hardware last verified ${lastCheckedLabel}`;
  }

  function confidentialNoticeLine(operation, text) {
    const message =
      (operation?.phase || '') === 'early_access' ? text?.operation_states?.early_access || '' : '';
    return {text: message, hidden: !message};
  }

  function confidentialSetupOperationLines(operation, text, attestationMessage = '') {
    const rendered = confidentialOperationRender(operation, text);
    const notice = confidentialNoticeLine(operation, text);
    if (!notice.hidden) {
      return {state: attestationMessage, operation: '', operationTone: '', notice};
    }
    return {
      state: rendered.message || attestationMessage,
      operation: rendered.message || '',
      operationTone: rendered.tone,
      notice,
    };
  }

  function confidentialAudioSetting(activeLane) {
    return activeLane?.confidential_audio !== false;
  }

  function confidentialEgressLine(activeLane, beats) {
    return confidentialAudioSetting(activeLane)
      ? beats?.egress_audio_on || ''
      : beats?.egress_audio_off || '';
  }

  function confidentialAudioRender(activeLane, attestation, text) {
    const on = confidentialAudioSetting(activeLane);
    const audio = text?.audio || {};
    return {
      hidden: (attestation?.state || 'off') === 'off',
      on,
      label: audio.label || '',
      description: on ? audio.on || '' : audio.off || '',
      note: audio.note || '',
    };
  }

  function confidentialAudioDeferralLine(activeLane, attestation, text) {
    const stateName = attestation?.state || 'off';
    if (!confidentialAudioSetting(activeLane)) return '';
    if (
      stateName !== 'verifying' &&
      stateName !== 'failed' &&
      stateName !== 'stale' &&
      stateName !== 'unreachable'
    ) {
      return '';
    }
    return text?.audio?.deferral || '';
  }

  function confidentialAttestationRender(attestation, text, checkedLabel = '') {
    const stateName = attestation?.state || 'off';
    const states = text?.attestation_states || {};
    if (stateName === 'verified') {
      return {
        pill: 'active',
        tone: 'hot',
        message: formatCopy(states.verified || '', {checked: checkedLabel}),
        recheck: false,
      };
    }
    if (stateName === 'inactive') {
      return {
        pill: 'available',
        tone: '',
        message: states.inactive || '',
        recheck: false,
      };
    }
    if (stateName === 'failed' || stateName === 'stale' || stateName === 'unreachable') {
      return {
        pill: stateName === 'unreachable' ? 'unreachable' : 'not ready',
        tone: 'bad',
        message: states[stateName] || '',
        recheck: true,
      };
    }
    if (stateName === 'verifying') {
      return {
        pill: 'checking',
        tone: '',
        message: states.verifying || '',
        recheck: false,
      };
    }
    return {
      pill: 'off',
      tone: '',
      message: '',
      recheck: false,
    };
  }

  function clearConfidentialInProgressOperation(activeLane) {
    const operation = activeLane?.confidential_operation;
    if (!operation || confidentialOperationIsTerminal(operation)) return false;
    activeLane.confidential_operation = null;
    return true;
  }

  function confidentialGlanceForAttestation(attestation, text, checkedLabel = '') {
    const glance = text?.glance || {};
    const stateName = attestation?.state || 'off';
    if (stateName === 'off') {
      return {label: '', value: '', detail: ''};
    }
    if (stateName === 'verified') {
      const row = glance.confidential_verified || {};
      return {
        label: row.label || glance.lane_label || '',
        value: row.value || '',
        detail: formatCopy(row.detail || '', {checked: checkedLabel}),
      };
    }
    if (stateName === 'inactive') {
      const row = glance.confidential_available || {};
      return {
        label: row.label || '',
        value: row.value || '',
        detail: row.detail || '',
      };
    }
    if (stateName === 'verifying') {
      const row = glance.confidential_checking || {};
      return {
        label: row.label || '',
        value: row.value || '',
        detail: row.detail || '',
      };
    }
    const row = glance.confidential_blocked || {};
    const message = confidentialAttestationRender(attestation, text?.confidential, checkedLabel).message;
    return {
      label: row.label || '',
      value: row.value || '',
      detail: formatCopy(row.detail || '', {message}),
    };
  }

  async function pollConfidentialUntilTerminal({
    fetchStatus,
    sleepFn,
    applyStatus,
    isCurrent,
    intervalMs,
    maxElapsedMs,
    initialStatus = null,
    nowFn = Date.now,
  }) {
    const started = nowFn();
    if (initialStatus) {
      applyStatus(initialStatus);
      if (confidentialOperationIsTerminal(initialStatus.active_lane?.confidential_operation)) return initialStatus;
    }
    while (isCurrent() && nowFn() - started < maxElapsedMs) {
      const status = await fetchStatus();
      applyStatus(status);
      if (confidentialOperationIsTerminal(status?.active_lane?.confidential_operation)) return status;
      await sleepFn(intervalMs);
    }
    if (!isCurrent()) return null;
    throw new Error('confidential setup timed out');
  }

  function handleConfidentialPollError({
    generation,
    currentGeneration,
    clearOperation,
    stopPoll,
    showError,
    error,
  }) {
    if (generation !== currentGeneration()) return false;
    clearOperation();
    stopPoll();
    showError(error?.message || '');
    return true;
  }

  function formatCopy(template, values = {}) {
    return String(template || '').replace(/\{(\w+)\}/g, (_, key) => values[key] ?? '');
  }

  function byoReasonCopy(reasonCode, context, text, provider, model = '') {
    if (context === 'probe' && reasonCode === 'model_not_found') {
      return formatCopy(text?.custom_not_found || '', {provider, model});
    }
    const reasonKey = {
      provider_key_invalid: 'reason_rejected',
      provider_quota_exceeded: 'reason_quota',
      network_unreachable: 'reason_network',
      brain_refresh_timeout: 'reason_network',
    }[reasonCode || ''] || 'reason_unknown';
    return formatCopy(text?.[reasonKey] || text?.reason_unknown || '', {provider, model});
  }

  function byoModelStepAllowed(provider, validation) {
    return validation?.valid === true;
  }

  function byoEntryMode(provider, validation) {
    if (provider === 'local') return 'endpoint';
    if (byoModelStepAllowed(provider, validation)) return 'model';
    return 'paste';
  }

  function byoKeyInputEmpty(value) {
    return String(value || '').trim() === '';
  }

  function byoTierList(provider, providersPayload) {
    const tiers = providersPayload?.model_tiers?.[provider];
    if (!Array.isArray(tiers)) return [];
    const rank = {top: 0, mid: 1, lite: 2};
    return tiers
      .slice()
      .sort((left, right) => (rank[left?.tier] ?? 99) - (rank[right?.tier] ?? 99));
  }

  function preselectByoModel(provider, providersPayload) {
    const remembered = String(providersPayload?.byo_models?.[provider] || '').trim();
    if (remembered) return remembered;
    const active = providersPayload?.active || {};
    const activeModel = String(active.model || '').trim();
    if (active.provider === provider && activeModel) return activeModel;
    const lite = byoTierList(provider, providersPayload).find((tier) => tier?.tier === 'lite');
    return String(lite?.model || '').trim();
  }

  function byoTierRows(provider, providersPayload, activeModel, text) {
    return byoTierList(provider, providersPayload).map((tier) => {
      const model = String(tier?.model || '').trim();
      const rowTier = String(tier?.tier || '').trim();
      const current = !!model && model === activeModel;
      return {
        tier: rowTier,
        label: String(tier?.label || model || '').trim(),
        model,
        blurb: text?.[`tier_blurb_${rowTier}`] || '',
        tag: current ? text?.tier_tag_current || '' : rowTier === 'lite' ? text?.tier_tag_suggested || '' : '',
        current,
      };
    });
  }

  function byoModelLabel(provider, model, providersPayload) {
    const modelId = String(model || '').trim();
    const tier = byoTierList(provider, providersPayload).find((item) => item?.model === modelId);
    return String(tier?.label || modelId);
  }

  function byoCustomText(selected, selectedIsCustom, customModel) {
    const customValue = String(customModel || '');
    if (customValue) return customValue;
    return selectedIsCustom ? String(selected || '') : '';
  }

  function byoCustomShowsChecked(customValue, checkedModel) {
    const candidate = String(customValue || '').trim();
    return !!candidate && candidate === String(checkedModel || '').trim();
  }

  function byoSaveDisabled(selected, selectedIsCustom, checkedModel) {
    const model = String(selected || '').trim();
    if (!model) return true;
    if (!selectedIsCustom) return false;
    return String(checkedModel || '').trim() !== model;
  }

  function byoCustomInputDraft(value) {
    const customModel = String(value || '');
    return {
      customModel,
      checkedModel: '',
      selectedModel: customModel.trim(),
    };
  }

  async function runByoKeyCheckFlow({
    apiFn,
    applyKeys,
    provider,
    providerName,
    envVar,
    value,
    text,
    providersPayload,
    setMode,
    selectModel,
    resetDraft,
    renderFn,
    showStatus,
  }) {
    if (byoKeyInputEmpty(value)) {
      showStatus(text?.key_hint || '', '');
      return {status: 'empty'};
    }
    showStatus(formatCopy(text?.checking_key || '', {provider: providerName}), '');
    const check = await apiFn('api/keys/check', {
      method: 'POST',
      body: JSON.stringify({env_var: envVar, value}),
    });
    if (check?.valid !== true) {
      resetDraft();
      setMode('paste');
      renderFn();
      const reason = byoReasonCopy(check?.reason_code, 'key', text, providerName);
      showStatus(formatCopy(text?.key_failed || '', {provider: providerName, reason}), 'error');
      return {status: 'invalid', validation: check};
    }
    const result = await apiFn('api/keys', {
      method: 'PUT',
      body: JSON.stringify({env_var: envVar, value}),
    });
    applyKeys(result);
    const validation = result?.key_validation?.[provider] || result?.validation || {};
    if (validation.valid !== true) {
      resetDraft();
      setMode('paste');
      renderFn();
      const reason = byoReasonCopy(validation.reason_code, 'key', text, providerName);
      showStatus(formatCopy(text?.key_failed || '', {provider: providerName, reason}), 'error');
      return {status: 'invalid', validation};
    }
    resetDraft();
    if (byoModelStepAllowed(provider, validation)) {
      selectModel(preselectByoModel(provider, providersPayload));
      setMode('model');
      renderFn();
      return {status: 'model', validation};
    }
    setMode('paste');
    renderFn();
    return {status: 'checked', validation};
  }

  async function runByoModelSaveFlow({
    apiFn,
    applyProviders,
    provider,
    providerName,
    model,
    modelLabel = '',
    googleModelResolutionTargets = [],
    text,
    setMode,
    renderFn,
    showStatus,
  }) {
    showStatus(formatCopy(text?.model_saving || '', {model}), '');
    const probe = await apiFn('api/validate-model', {
      method: 'POST',
      body: JSON.stringify({provider, model}),
    });
    if (probe?.valid !== true) {
      if (probe?.reason_code === 'key_missing') {
        setMode('paste');
        renderFn();
        const reason = byoReasonCopy(probe.reason_code, 'key', text, providerName, model);
        showStatus(formatCopy(text?.key_failed || '', {provider: providerName, reason}), 'error');
        return {status: 'key_missing', probe};
      }
      const reason = byoReasonCopy(probe?.reason_code, 'probe', text, providerName, model);
      const message = probe?.reason_code === 'model_not_found'
        ? reason
        : formatCopy(text?.probe_failed_save || '', {provider: providerName, model, reason});
      showStatus(message, 'error');
      return {status: 'probe_failed', probe};
    }
    try {
      const body = {lane: 'byo', provider, model};
      if (Array.isArray(googleModelResolutionTargets) && googleModelResolutionTargets.length > 0) {
        body[googleModelResolutionTargetsField] = googleModelResolutionTargets;
      }
      const providers = await apiFn('api/providers', {
        method: 'PUT',
        body: JSON.stringify(body),
      });
      applyProviders(providers);
      renderFn();
      if (providers?.active_lane?.lane === 'confidential') {
        showStatus(
          formatCopy(text?.model_saved_restore || '', {label: modelLabel || model}),
          'ok',
        );
        return {status: 'restored', providers};
      }
      return {status: 'saved', providers};
    } catch (err) {
      renderFn();
      showStatus(err?.message || '', 'error');
      return {status: 'save_failed', error: err};
    }
  }

  async function runByoCustomProbeFlow({
    apiFn,
    provider,
    providerName,
    model,
    text,
    setMode,
    selectModel,
    markChecked,
    renderFn,
    showStatus,
  }) {
    showStatus(formatCopy(text?.custom_checking || '', {provider: providerName, model}), '');
    const probe = await apiFn('api/validate-model', {
      method: 'POST',
      body: JSON.stringify({provider, model}),
    });
    if (probe?.valid === true) {
      markChecked(model);
      selectModel(model);
      renderFn();
      showStatus(formatCopy(text?.custom_ok || '', {model}), 'ok');
      return {status: 'valid', probe};
    }
    if (probe?.reason_code === 'key_missing') {
      setMode('paste');
      renderFn();
      const reason = byoReasonCopy(probe.reason_code, 'key', text, providerName, model);
      showStatus(formatCopy(text?.key_failed || '', {provider: providerName, reason}), 'error');
      return {status: 'key_missing', probe};
    }
    const reason = byoReasonCopy(probe?.reason_code, 'probe', text, providerName, model);
    showStatus(reason, 'error');
    return {status: 'invalid', probe};
  }

  function laneCopy(id) {
    return (copy.lanes || []).find((lane) => lane.id === id) || {};
  }

  function laneDisplayLabel(lane) {
    if (lane.id === 'byo') return lane.label || 'byo';
    return (lane.label || lane.id || '').toLowerCase();
  }

  function activeLaneLabel(kind) {
    return copy.active_lane_labels?.[kind] || kind || '';
  }

  function applyCopy(payload) {
    copy = payload || {};
    providerLabels = copy.provider_labels || fallbackProviderLabels;
    setText('thinkingHeading', copy.heading || 'thinking');
  }

  function renderInitialLoading() {
    const loading = $('thinking-loading');
    if (!loading) return;
    loading.innerHTML = window.SurfaceState.loading({ text: 'loading thinking settings…' });
    loading.style.display = '';
    const app = $('thinkingApp');
    if (app) app.hidden = true;
  }

  function revealThinkingApp() {
    const loading = $('thinking-loading');
    if (loading) loading.style.display = 'none';
    const app = $('thinkingApp');
    if (app) app.hidden = false;
  }

  function renderInitialError(err) {
    window.logError?.(err, { context: 'thinking-state' });
    const loading = $('thinking-loading');
    if (!loading) return;
    loading.innerHTML = window.SurfaceState.error({
      heading: "Couldn't load thinking settings",
      desc: window.CONVEY_COPY.RELOAD_HINT,
      serverMessage: err?.serverMessage || err?.message || '',
      detail: err,
      retry: true,
    });
    loading.querySelector('.surface-state-retry')?.addEventListener('click', () => {
      init();
    });
  }

  async function loadInitialState() {
    renderInitialLoading();
    try {
      const payload = await window.apiJson('/app/thinking/api/state');
      state.providers = payload.providers || {};
      state.keys = payload.keys || {};
      applyCopy(payload.copy || {});
      revealThinkingApp();
      return true;
    } catch (err) {
      renderInitialError(err);
      return false;
    }
  }

  async function api(path, options = {}) {
    const response = await fetch(path, {
      ...options,
      headers: {
        ...(options.body ? {'Content-Type': 'application/json'} : {}),
        ...(options.headers || {}),
      },
    });
    const payload = await response.json();
    if (!response.ok || payload.error) {
      throw new Error(payload.detail || payload.error || 'request failed');
    }
    return payload;
  }

  function sleep(ms) {
    return new Promise((resolve) => setTimeout(resolve, ms));
  }

  function showView(name, options = {}) {
    let target = views.has(name) ? name : 'main';
    if (target === 'lane-switch' && !state.pendingSwitchTarget) {
      target = 'main';
    }
    if (target !== 'local-setup') {
      stopInstallPoll();
      stopRuntimePoll();
    } else if (state.localModels.length > 0) {
      refreshInstallStatus({autoResume: true}).catch((err) => {
        setMessage('localSetupMessage', err.message, 'error');
      });
      refreshLocalRuntime({autoResume: true}).catch((err) => {
        setMessage('localSetupMessage', err.message, 'error');
      });
    }
    if (target !== 'confidential-setup') {
      stopConfidentialPoll({clearOperation: true});
    }
    document.querySelectorAll('#providers [data-view]').forEach((section) => {
      section.hidden = section.dataset.view !== target;
    });
    const nextHash = `#${target}`;
    // A hashless load already is the setup route: leave the address alone rather
    // than writing a route token the owner never asked for.
    const keepHashless = options.keepHashless && target === 'main' && !window.location.hash;
    if (!keepHashless && window.location.hash !== nextHash) {
      if (options.replace) {
        window.history.replaceState(null, '', nextHash);
      } else {
        window.history.pushState(null, '', nextHash);
      }
    }
  }

  function viewFromHash() {
    const hash = window.location.hash.replace(/^#/, '');
    return views.has(hash) ? hash : 'main';
  }

  function encodeThinkingSegment(value) {
    return encodeURIComponent(value);
  }

  function decodeThinkingSegment(value) {
    try {
      return decodeURIComponent(value);
    } catch (_) {
      return null;
    }
  }

  function todayThinkingDay() {
    const now = new Date();
    return `${now.getFullYear()}${String(now.getMonth() + 1).padStart(2, '0')}${String(now.getDate()).padStart(2, '0')}`;
  }

  function thinkingRunsRoute({kind = 'runs', day = '', talent = '', useId = '', facet = '', facetExplicit = false}) {
    const resolvedFacet = facetExplicit && facet ? facet : '';
    const resolvedFacetExplicit = Boolean(resolvedFacet);
    const key = kind === 'run-id'
      ? `run:${useId}`
      : `runs:${day}:${talent}:${useId}`;
    return {kind, day, talent, useId, facet: resolvedFacet, facetExplicit: resolvedFacetExplicit, key};
  }

  function currentThinkingRunsRoute(route) {
    return thinkingRunsRoute({
      ...route,
      facet: state.runsFacet,
      facetExplicit: state.runsFacetExplicit,
    });
  }

  function thinkingRunsHash(route) {
    const suffix = route.facetExplicit && route.facet
      ? `?facet=${encodeURIComponent(route.facet)}`
      : '';
    if (route.kind === 'run-id') {
      return `#runs/run/${encodeThinkingSegment(route.useId)}${suffix}`;
    }
    const parts = ['#runs', route.day];
    if (route.talent) parts.push(encodeThinkingSegment(route.talent));
    if (route.useId) parts.push(encodeThinkingSegment(route.useId));
    return `${parts.join('/')}${suffix}`;
  }

  function parseThinkingHash() {
    const rawHash = window.location.hash.replace(/^#/, '');
    const [hash, query = ''] = rawHash.split('?', 2);
    if (!hash.startsWith('runs')) return null;
    const params = new URLSearchParams(query);
    const facet = params.get('facet') || '';
    const facetExplicit = Boolean(facet);
    const parts = hash.split('/');
    if (parts[0] !== 'runs') return null;
    if (parts.length === 1) return {kind: 'runs-root', key: 'runs-root'};
    if (parts[1] === 'run') {
      if (parts.length !== 3 || !parts[2]) return {kind: 'runs-invalid', key: 'runs-invalid'};
      const useId = decodeThinkingSegment(parts[2]);
      return useId
        ? thinkingRunsRoute({kind: 'run-id', useId, facet, facetExplicit})
        : {kind: 'runs-invalid', key: 'runs-invalid'};
    }
    if (parts.length < 2 || parts.length > 4 || !/^\d{8}$/.test(parts[1])) {
      return {kind: 'runs-invalid', key: 'runs-invalid'};
    }
    const decoded = parts.slice(2).map(decodeThinkingSegment);
    if (decoded.some((part) => !part)) return {kind: 'runs-invalid', key: 'runs-invalid'};
    const [talent = '', useId = ''] = decoded;
    if (useId && !talent) return {kind: 'runs-invalid', key: 'runs-invalid'};
    return thinkingRunsRoute({
      kind: 'runs',
      day: parts[1],
      talent,
      useId,
      facet,
      facetExplicit,
    });
  }

  function replaceThinkingHash(hash) {
    if (window.location.hash !== hash) {
      window.history.replaceState(null, '', hash);
    }
  }

  function thinkingSelectedRunId(route) {
    return (route?.kind === 'runs' || route?.kind === 'run-id') ? route.useId || '' : '';
  }

  function setThinkingRoute(route) {
    if (route?.kind === 'runs' || route?.kind === 'run-id') {
      setRunsFacet(route.facet, route.facetExplicit);
      state.runsLastHash = thinkingRunsHash(route);
    }
    const selectedUseId = thinkingSelectedRunId(route);
    if (state.runsSelectedUseId !== selectedUseId) {
      state.runsSelectedUseId = selectedUseId;
      closeThinkingPrompt();
      clearThinkingRunRenderState();
    }
    if (state.runsRouteKey !== route.key) {
      state.runsRouteKey = route.key;
      state.runsNavigationGeneration += 1;
    }
  }

  function thinkingCacheKey(kind, values = {}) {
    if (kind === 'day') return `day:${values.day}:facet:${values.facet || ''}`;
    if (kind === 'run') return `run:${values.useId}`;
    if (kind === 'prompt') return `prompt:${values.talent}`;
    if (kind === 'output') return `output:${values.day}:${values.file}`;
    return kind;
  }

  function beginThinkingRequest(kind) {
    const requestGeneration = (state.runsRequestGenerations[kind] || 0) + 1;
    state.runsRequestGenerations[kind] = requestGeneration;
    const token = {
      kind,
      navigationGeneration: state.runsNavigationGeneration,
      requestGeneration,
      hash: window.location.hash,
      selectionKey: currentRunsSelectionKey(),
    };
    state.runsInFlight[kind] = token;
    return token;
  }

  function isCurrentThinkingRequest(token) {
    return token.navigationGeneration === state.runsNavigationGeneration
      && token.requestGeneration === state.runsRequestGenerations[token.kind]
      && token.hash === window.location.hash
      && token.selectionKey === currentRunsSelectionKey();
  }

  function readThinkingCache(kind, key) {
    return state.runsCache[kind].get(key) || null;
  }

  function writeThinkingCache(kind, key, value, token) {
    if (!isCurrentThinkingRequest(token)) return false;
    state.runsCache[kind].set(key, value);
    return true;
  }

  async function loadThinkingRequest(kind, key, load, renderReady, renderFailed, shouldCache = () => true) {
    const token = beginThinkingRequest(kind);
    try {
      const value = await load();
      if (!isCurrentThinkingRequest(token)) return;
      if (!shouldCache(value)) {
        renderReady(value);
        return;
      }
      if (!writeThinkingCache(kind, key, value, token) || !isCurrentThinkingRequest(token)) return;
      renderReady(value);
    } catch (err) {
      if (isCurrentThinkingRequest(token)) renderFailed(err);
    } finally {
      if (isCurrentThinkingRequest(token)) state.runsInFlight[kind] = null;
    }
  }

  function setThinkingStatus(id, message) {
    const status = $(id);
    if (!status) return;
    status.textContent = message || '';
    status.hidden = !message;
  }

  function runContextFromRecord(route, record) {
    const day = record?.day;
    const talent = record?.name;
    const useId = record?.id || route.useId;
    if (!/^\d{8}$/.test(day || '') || !talent || !useId) return route;
    const contextual = thinkingRunsRoute({
      kind: 'runs', day, talent, useId,
      facet: route.facet,
      facetExplicit: route.facetExplicit,
    });
    if (route.day !== day || route.talent !== talent || route.useId !== useId || route.kind === 'run-id') {
      replaceThinkingHash(thinkingRunsHash(contextual));
    }
    return contextual;
  }

  function thinkingPanelHeading(tabId) {
    if (tabId === 'thinkingRunsTab') return $('thinkingRunsHeading');
    return $('thinkingHeading');
  }

  function activateThinkingTab(tabId, origin, tablistId, headingId = '') {
    const tablist = $(tablistId);
    const tab = $(tabId);
    if (!tab) return;
    const tabs = tablist?.querySelectorAll('[role="tab"]') || [];
    tabs.forEach((candidate) => {
      const selected = candidate === tab;
      candidate.setAttribute('aria-selected', String(selected));
      candidate.tabIndex = selected ? 0 : -1;
    });
    if (origin === 'pointer' || origin === 'keyboard') {
      tab.focus();
      return;
    }
    // Only an owner-initiated section change moves focus. A plain load ('reload')
    // must not draw a focus ring on a heading nobody asked for.
    if (origin === 'history' && !tablist?.contains(document.activeElement)) {
      (headingId ? $(headingId) : thinkingPanelHeading(tabId))?.focus({preventScroll: true});
    }
  }

  function activateThinkingSectionTab(tabId, origin) {
    activateThinkingTab(tabId, origin, 'thinkingSectionTabs');
  }

  function showThinkingSection(section, route, origin) {
    setThinkingRoute(route);
    document.querySelectorAll('#providers [data-view]').forEach((view) => {
      view.hidden = true;
    });
    document.querySelectorAll('[data-thinking-section]').forEach((panel) => {
      panel.hidden = panel.dataset.thinkingSection !== section;
    });
    activateThinkingSectionTab('thinkingRunsTab', origin);
    if (section === 'runs') {
      if (route.kind === 'runs') loadThinkingRuns(route);
      if (route.useId) loadThinkingRun(route);
    }
  }

  function navigateThinkingSection(section, origin) {
    const hash = section === 'setup'
      ? '#main'
      : (state.runsLastHash || '#runs');
    if (window.location.hash !== hash) window.history.pushState(null, '', hash);
    routeThinkingHash(origin);
  }

  function showThinkingSetup(origin) {
    setThinkingRoute({key: 'setup'});
    document.querySelectorAll('[data-thinking-section]').forEach((panel) => {
      panel.hidden = true;
    });
    showView(viewFromHash(), {replace: true, keepHashless: true});
    activateThinkingSectionTab('thinkingSetupTab', origin);
  }

  function routeThinkingHash(origin = 'history') {
    const hash = window.location.hash;
    if (!hash) {
      showThinkingSetup(origin);
      return;
    }
    const route = parseThinkingHash();
    if (route?.kind === 'runs-root') {
      const canonical = thinkingRunsRoute({kind: 'runs', day: todayThinkingDay()});
      replaceThinkingHash(thinkingRunsHash(canonical));
      showThinkingSection('runs', canonical, origin);
      return;
    }
    if (route?.kind === 'runs-invalid') {
      const canonical = thinkingRunsRoute({kind: 'runs', day: todayThinkingDay()});
      replaceThinkingHash(thinkingRunsHash(canonical));
      showThinkingSection('runs', canonical, origin);
      setThinkingStatus('thinkingRunsStatus', "that talent run isn't available.");
      return;
    }
    if (route?.kind === 'runs' || route?.kind === 'run-id') {
      replaceThinkingHash(thinkingRunsHash(route));
      showThinkingSection('runs', route, origin);
      setThinkingStatus('thinkingRunsStatus', '');
      return;
    }
    showThinkingSetup(origin);
  }

  function bindThinkingTablist(tablistId, activate) {
    const thinkingTabs = Array.from($(tablistId)?.querySelectorAll('[role="tab"]') || []);
    thinkingTabs.forEach((tab) => {
      tab.addEventListener('click', () => {
        if (!tab.hidden) activate(tab, 'pointer');
      });
      tab.addEventListener('keydown', (event) => {
        const visibleTabs = thinkingTabs.filter((candidate) => !candidate.hidden);
        const currentIndex = visibleTabs.indexOf(tab);
        if (currentIndex === -1) return;
        let nextIndex = currentIndex;
        if (event.key === 'ArrowLeft') nextIndex = (currentIndex + visibleTabs.length - 1) % visibleTabs.length;
        else if (event.key === 'ArrowRight') nextIndex = (currentIndex + 1) % visibleTabs.length;
        else if (event.key === 'Home') nextIndex = 0;
        else if (event.key === 'End') nextIndex = visibleTabs.length - 1;
        else return;
        event.preventDefault();
        activate(visibleTabs[nextIndex], 'keyboard');
      });
    });
  }

  function bindThinkingSectionTabs() {
    const thinkingSectionForTab = (tab) => ({
      thinkingSetupTab: 'setup',
      thinkingRunsTab: 'runs',
    })[tab.id];
    bindThinkingTablist('thinkingSectionTabs', (tab, origin) => {
      navigateThinkingSection(thinkingSectionForTab(tab), origin);
    });
  }

  function runsDayInputValue(day) {
    return `${day.slice(0, 4)}-${day.slice(4, 6)}-${day.slice(6, 8)}`;
  }

  function runsDayFromInput(value) {
    const day = String(value || '').replace(/-/g, '');
    return /^\d{8}$/.test(day) ? day : '';
  }

  function shiftThinkingDay(day, amount) {
    const value = new Date(Number(day.slice(0, 4)), Number(day.slice(4, 6)) - 1, Number(day.slice(6, 8)) + amount);
    return `${value.getFullYear()}${String(value.getMonth() + 1).padStart(2, '0')}${String(value.getDate()).padStart(2, '0')}`;
  }

  function currentRunsSelectionKey() {
    return `${state.runsRouteKey}:facet:${state.runsFacet}`;
  }

  function setRunsFacet(value, explicit) {
    const next = value || '';
    if (state.runsFacet !== next || state.runsFacetExplicit !== explicit) {
      state.runsFacet = next;
      state.runsFacetExplicit = explicit;
      state.runsNavigationGeneration += 1;
    }
  }

  function thinkingRunFacts(run) {
    return [
      ['ran', window.JournalFormat.timestamp(run.start)],
      ['model', run.model],
      ['provider', run.provider],
      ['runtime', window.JournalFormat.duration(run.runtime_seconds)],
      ['status', run.failed ? 'failed' : (run.status || 'unknown').replaceAll('_', ' ')],
      ['thinking events', run.thinking_count],
      ['tool calls', run.tool_count],
      ['facet', run.facet],
    ].filter(([, value]) => value !== null && value !== undefined && value !== '');
  }

  function appendThinkingRunFacts(host, run) {
    thinkingRunFacts(run).forEach(([label, value]) => {
      const item = document.createElement('span');
      item.textContent = `${label}: ${value}`;
      host.appendChild(item);
    });
  }

  function normalizedThinkingRuns(payload) {
    return Array.isArray(payload?.uses) ? payload.uses.map((run) => ({
      id: run.id,
      name: run.name,
      start: run.start,
      status: run.status,
      failed: run.failed === true,
      model: run.model,
      provider: run.provider,
      runtime_seconds: run.runtime_seconds,
      thinking_count: run.thinking_count,
      tool_count: run.tool_count,
      facet: run.facet,
      output_file: run.output_file,
    })).filter((run) => run.id && run.name) : [];
  }

  function renderThinkingRunsSummary(runs) {
    const summary = $('thinkingRunsSummary');
    if (!summary) return;
    summary.replaceChildren();
    const total = document.createElement('span');
    total.textContent = `${runs.length} run${runs.length !== 1 ? 's' : ''}`;
    summary.appendChild(total);
    const failed = runs.filter((run) => run.failed).length;
    if (failed) {
      const item = document.createElement('span');
      item.textContent = `${failed} failed`;
      summary.appendChild(item);
    }
  }

  function thinkingRunControl(run) {
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'thinking-runs-run-control';
    button.textContent = 'run log';
    button.addEventListener('click', () => navigateThinkingRun(run));
    return button;
  }

  function renderThinkingRunList(host, runs) {
    const table = document.createElement('table');
    table.className = 'thinking-runs-table';
    const head = document.createElement('thead');
    const headRow = document.createElement('tr');
    for (const label of ['ran', 'status', 'model', 'provider', 'runtime', 'thinking events', 'tool calls', 'facet', 'output', 'prompt']) {
      const cell = document.createElement('th');
      cell.scope = 'col';
      cell.textContent = label;
      headRow.appendChild(cell);
    }
    head.appendChild(headRow);
    table.appendChild(head);
    const body = document.createElement('tbody');
    runs.forEach((run) => {
      const row = document.createElement('tr');
      if (run.failed) row.className = 'thinking-run-failed';
      for (const value of [window.JournalFormat.timestamp(run.start), run.failed ? 'failed' : (run.status || 'unknown').replaceAll('_', ' '), run.model, run.provider, window.JournalFormat.duration(run.runtime_seconds), run.thinking_count, run.tool_count, run.facet]) {
        const cell = document.createElement('td');
        if (value !== null && value !== undefined && value !== '') cell.textContent = value;
        row.appendChild(cell);
      }
      const output = document.createElement('td');
      if (run.output_file) output.textContent = 'output';
      row.appendChild(output);
      const prompt = document.createElement('td');
      prompt.appendChild(thinkingRunControl(run));
      row.appendChild(prompt);
      body.appendChild(row);
    });
    table.appendChild(body);
    host.appendChild(table);
    const cards = document.createElement('div');
    cards.className = 'thinking-runs-cards';
    runs.forEach((run) => {
      const card = document.createElement('article');
      card.className = 'thinking-runs-card';
      const heading = document.createElement('h4');
      heading.textContent = talentLabel(run.name);
      card.appendChild(heading);
      appendThinkingRunFacts(card, run);
      card.appendChild(thinkingRunControl(run));
      cards.appendChild(card);
    });
    host.appendChild(cards);
  }

  function renderThinkingRunsDay(payload, route) {
    const matchingRuns = normalizedThinkingRuns(payload).filter(run => !route.talent || run.name === route.talent);
    const runs = state.runsFailuresOnly ? matchingRuns.filter(run => run.failed) : matchingRuns;
    updateThinkingRunsDayControls(route);
    const facet = $('thinkingRunsFacet');
    if (facet) {
      facet.replaceChildren();
      const all = document.createElement('option');
      all.value = '';
      all.textContent = 'all';
      facet.appendChild(all);
      const returnedFacets = Array.isArray(payload?.facets)
        ? payload.facets
        : Object.entries(payload?.facets && typeof payload.facets === 'object' ? payload.facets : {})
          .map(([name, metadata]) => ({
            ...(metadata && typeof metadata === 'object' ? metadata : {}),
            name,
          }));
      returnedFacets.forEach((item) => {
        const option = document.createElement('option');
        option.value = item.name || item;
        option.textContent = item.title || item.name || item;
        facet.appendChild(option);
      });
      facet.value = state.runsFacet;
    }
    const host = $('thinkingRunsContent');
    if (!host) return;
    host.replaceChildren();
    $('thinkingRunsDetail').hidden = !route.useId;
    if (!route.useId) $('thinkingRunsNoOutput').hidden = true;
    renderThinkingRunsSummary(matchingRuns);
    const controls = document.createElement('div');
    controls.className = 'thinking-runs-filters';
    if (route.talent) {
      const context = document.createElement('span');
      context.textContent = talentLabel(route.talent);
      const all = document.createElement('a');
      all.href = thinkingRunsHash(thinkingRunsRoute({...route, talent: '', useId: ''}));
      all.textContent = 'all talents for this day';
      controls.append(context, all);
    }
    const label = document.createElement('label');
    const input = document.createElement('input');
    input.type = 'checkbox'; input.checked = state.runsFailuresOnly;
    input.addEventListener('change', () => { state.runsFailuresOnly = input.checked; renderThinkingRunsDay(payload, route); host.querySelector('input[type=checkbox]')?.focus(); });
    label.append(input, document.createTextNode(' failed runs only'));
    controls.append(label); host.append(controls);
    if (!runs.length) {
      const heading = document.createElement('p');
      heading.textContent = state.runsFailuresOnly ? 'no failed runs match this view' : route.talent ? 'no runs found for this talent on this day' : 'no talent runs on this day';
      const detail = document.createElement('p');
      detail.textContent = route.talent ? 'this day has no matching run record in the current view. try all talents or another day.' : 'runs appear here when processing takes place.';
      host.append(heading, detail);
      return;
    }
    const groups = new Map();
    runs.forEach((run) => groups.set(run.name, [...(groups.get(run.name) || []), run]));
    const expandEveryGroup = Boolean(route.talent) || runs.length <= thinkingRunsExpandAllBelow;
    groups.forEach((group, name) => {
      const section = document.createElement('section');
      section.className = 'thinking-runs-group';
      const heading = document.createElement('h3');
      heading.textContent = talentLabel(name);
      section.appendChild(heading);
      const details = document.createElement('details');
      details.className = 'thinking-runs-group-detail';
      details.open = state.runsGroupOpen.has(name) ? state.runsGroupOpen.get(name) : expandEveryGroup;
      const summary = document.createElement('summary');
      const failedInGroup = group.filter((run) => run.failed).length;
      summary.textContent = failedInGroup
        ? `${group.length} run${group.length !== 1 ? 's' : ''} · ${failedInGroup} failed`
        : `${group.length} run${group.length !== 1 ? 's' : ''}`;
      details.appendChild(summary);
      details.addEventListener('toggle', () => state.runsGroupOpen.set(name, details.open));
      const exact = document.createElement('p');
      exact.className = 'thinking-runs-group-id';
      exact.textContent = `exact id: ${name}`;
      details.appendChild(exact);
      const shown = state.runsGroupShown.get(name) || thinkingRunsPageSize;
      renderThinkingRunList(details, group.slice(0, shown));
      if (group.length > shown) {
        const remaining = group.length - shown;
        const next = Math.min(thinkingRunsPageSize, remaining);
        const more = document.createElement('button');
        more.type = 'button';
        more.className = 'thinking-runs-control';
        more.textContent = `show ${next} more run${next !== 1 ? 's' : ''} of ${remaining} left`;
        more.addEventListener('click', () => {
          state.runsGroupShown.set(name, shown + thinkingRunsPageSize);
          state.runsGroupOpen.set(name, true);
          renderThinkingRunsDay(payload, route);
        });
        details.appendChild(more);
      }
      section.appendChild(details);
      host.appendChild(section);
    });
  }

  function renderThinkingRunsLoading() {
    const host = $('thinkingRunsContent');
    if (!host) return;
    host.replaceChildren();
    const message = document.createElement('p');
    message.textContent = 'loading talent runs…';
    host.appendChild(message);
  }

  function renderThinkingRunsFailure() {
    const host = $('thinkingRunsContent');
    if (!host) return;
    host.replaceChildren();
    const message = document.createElement('p');
    message.textContent = "couldn't load talent runs";
    const retry = document.createElement('button');
    retry.type = 'button';
    retry.className = 'thinking-runs-retry';
    retry.textContent = 'try again';
    retry.addEventListener('click', () => loadThinkingRuns(parseThinkingHash(), true));
    host.append(message, retry);
  }

  function renderThinkingUpdatedDays(days) {
    const host = $('thinkingRunsUpdated');
    if (!host) return;
    host.replaceChildren();
    (Array.isArray(days) ? days : []).forEach((day) => {
      const link = document.createElement('a');
      link.href = thinkingRunsHash(currentThinkingRunsRoute({kind: 'runs', day, talent: '', useId: ''}));
      link.textContent = window.JournalFormat.day(day);
      host.appendChild(link);
    });
  }

  function loadThinkingUpdatedDays() {
    const token = beginThinkingRequest('day');
    window.apiJson('/app/thinking/api/updated-days').then((days) => {
      if (isCurrentThinkingRequest(token)) renderThinkingUpdatedDays(days);
    }).catch(() => {
      if (isCurrentThinkingRequest(token)) setThinkingStatus('thinkingRunsStatus', "some run details aren't available right now.");
    }).finally(() => {
      if (isCurrentThinkingRequest(token)) state.runsInFlight.day = null;
    });
  }

  function thinkingDayUrl(route) {
    const path = `/app/thinking/api/talents/${route.day}`;
    return state.runsFacetExplicit ? `${path}?facet=${encodeURIComponent(state.runsFacet)}` : path;
  }

  function updateThinkingRunsDayControls(route) {
    const today = todayThinkingDay();
    const date = $('thinkingRunsDate');
    if (date) {
      date.value = runsDayInputValue(route.day);
      date.max = runsDayInputValue(today);
    }
    // Tomorrow has not happened; an empty future day reads as missing processing.
    const next = $('thinkingRunsNext');
    if (next) next.disabled = route.day >= today;
  }

  function resetThinkingRunGroups(key) {
    if (state.runsGroupKey === key) return;
    state.runsGroupKey = key;
    state.runsGroupOpen.clear();
    state.runsGroupShown.clear();
  }

  function loadThinkingRuns(route, force = false) {
    if (!route || route.kind !== 'runs') return;
    const key = thinkingCacheKey('day', {day: route.day, facet: state.runsFacet});
    updateThinkingRunsDayControls(route);
    resetThinkingRunGroups(key);
    const cached = readThinkingCache('day', key);
    if (cached && !force) {
      renderThinkingRunsDay(cached, route);
      loadThinkingUpdatedDays();
      return;
    }
    renderThinkingRunsLoading();
    loadThinkingRequest(
      'day',
      key,
      () => window.apiJson(thinkingDayUrl(route)),
      (payload) => {
        renderThinkingRunsDay(payload, route);
        loadThinkingUpdatedDays();
      },
      renderThinkingRunsFailure,
    );
  }

  function navigateThinkingRun(run) {
    const route = parseThinkingHash();
    if (!route?.day) return;
    const next = thinkingRunsRoute({
      kind: 'runs', day: route.day, talent: run.name, useId: run.id,
      facet: route.facet,
      facetExplicit: route.facetExplicit,
    });
    window.history.pushState(null, '', thinkingRunsHash(next));
    routeThinkingHash('pointer');
  }

  function renderThinkingRunLog(run) {
    const panel = $('thinkingRunsLogPanel');
    if (!panel) return;
    panel.replaceChildren();
    if (run.status === 'running') {
      const progress = document.createElement('p');
      progress.textContent = 'this run is still in progress.';
      const check = document.createElement('p');
      check.textContent = 'check back soon.';
      panel.append(progress, check);
      return;
    }
    // The run-level outcome is run.status; a step with no recorded detail only
    // means "nothing was written for this step", never that the run failed.
    const runFinished = run.failed !== true && run.status === 'completed';
    (Array.isArray(run.events) ? run.events : []).forEach((event) => {
      const item = document.createElement('div');
      const fields = [['thinking', event.thinking], ['tools', event.tools], ['args', event.args], ['result', event.result], ['error', event.error]];
      fields.forEach(([label, value]) => {
        if (value === null || value === undefined || value === '') return;
        const fact = document.createElement('p');
        fact.textContent = `${label}: ${typeof value === 'string' ? value : JSON.stringify(value)}`;
        item.appendChild(fact);
      });
      if (!item.children.length) {
        const note = document.createElement('p');
        if (runFinished) {
          note.textContent = 'no detail was recorded for this step';
        } else {
          note.textContent = event.event === 'tool_start' ? 'tool call did not complete' : 'did not complete';
        }
        item.appendChild(note);
      }
      panel.appendChild(item);
    });
  }

  function renderThinkingRunDetail(run, route) {
    state.runsDetail = run;
    const detail = $('thinkingRunsDetail');
    if (detail) detail.hidden = false;
    setText('thinkingRunsDetailHeading', talentLabel(run.name || route.talent));
    const exact = [run.name || route.talent, run.provider, run.model].filter(Boolean).join(' · ');
    setText('thinkingRunsDetailIdsText', exact);
    setHidden('thinkingRunsDetailIds', !exact);
    const facts = $('thinkingRunsDetailFacts');
    if (facts) {
      facts.replaceChildren();
      appendThinkingRunFacts(facts, run);
    }
    const output = $('thinkingRunsOutputTab');
    if (output) output.hidden = !run.output_file;
    $('thinkingRunsNoOutput').hidden = !!run.output_file;
    renderThinkingRunLog(run);
    activateThinkingRunDetailTab('thinkingRunsLogTab', 'history');
  }

  function clearThinkingRunRenderState() {
    state.runsDetail = null;
    $('thinkingRunsDetail').hidden = true;
    setHidden('thinkingRunsDetailIds', true);
    $('thinkingRunsDetailFacts')?.replaceChildren();
    $('thinkingRunsOutputTab').hidden = true;
    $('thinkingRunsNoOutput').hidden = true;
    $('thinkingRunsOutputPanel')?.replaceChildren();
  }

  function renderThinkingRunFailure() {
    const panel = $('thinkingRunsLogPanel');
    if (!panel) return;
    panel.replaceChildren();
    const message = document.createElement('p');
    message.textContent = "couldn't load that run";
    panel.appendChild(message);
  }

  function renderThinkingRunPending(route) {
    const detail = $('thinkingRunsDetail');
    if (detail) detail.hidden = false;
    setText('thinkingRunsDetailHeading', talentLabel(route.talent));
    setHidden('thinkingRunsDetailIds', true);
    $('thinkingRunsDetailFacts')?.replaceChildren();
    $('thinkingRunsOutputTab').hidden = true;
    $('thinkingRunsNoOutput').hidden = true;
    renderThinkingRunLog({status: 'running'});
    activateThinkingRunDetailTab('thinkingRunsLogTab', 'history');
  }

  function loadThinkingRun(route) {
    if (!route?.useId) return;
    const selectedDetail = $('thinkingRunsDetail');
    if (selectedDetail) {
      selectedDetail.hidden = false;
      requestAnimationFrame(() => {
        if (selectedDetail.isConnected) selectedDetail.scrollIntoView({block: 'start'});
      });
    }
    const key = thinkingCacheKey('run', {useId: route.useId});
    const cached = readThinkingCache('run', key);
    if (cached) {
      const contextual = runContextFromRecord(route, cached);
      const contextChanged = contextual.key !== route.key;
      setThinkingRoute(contextual);
      renderThinkingRunDetail(cached, contextual);
      if (route.kind === 'run-id' || contextChanged) loadThinkingRuns(contextual);
      return;
    }
    const detail = $('thinkingRunsDetail');
    if (detail) detail.hidden = false;
    setText('thinkingRunsDetailHeading', talentLabel(route.talent));
    setHidden('thinkingRunsDetailIds', true);
    const panel = $('thinkingRunsLogPanel');
    if (panel) panel.textContent = 'loading run details…';
    loadThinkingRequest(
      'run',
      key,
      () => window.apiJson(`/app/thinking/api/run/${encodeThinkingSegment(route.useId)}`),
      (run) => {
        if (run?.reason_code === 'talent_run_pending') {
          renderThinkingRunPending(route);
          return;
        }
        const contextual = runContextFromRecord(route, run);
        const contextChanged = contextual.key !== route.key;
        setThinkingRoute(contextual);
        renderThinkingRunDetail(run, contextual);
        if (route.kind === 'run-id' || contextChanged) loadThinkingRuns(contextual);
      },
      renderThinkingRunFailure,
      (run) => run?.reason_code !== 'talent_run_pending',
    );
  }

  function activateThinkingRunDetailTab(tabId, origin) {
    if ($(tabId)?.hidden) return;
    const output = tabId === 'thinkingRunsOutputTab';
    $('thinkingRunsLogPanel').hidden = output;
    $('thinkingRunsOutputPanel').hidden = !output;
    activateThinkingTab(tabId, origin, 'thinkingRunsDetailTabs', 'thinkingRunsDetailHeading');
    if (output) loadThinkingOutput();
  }

  function loadThinkingOutput() {
    const run = state.runsDetail;
    if (!run?.output_file || !run.day) return;
    const key = thinkingCacheKey('output', {day: run.day, file: run.output_file});
    const cached = readThinkingCache('output', key);
    const panel = $('thinkingRunsOutputPanel');
    if (cached) {
      panel.textContent = cached.content || '';
      return;
    }
    panel.textContent = 'loading output…';
    const encoded = String(run.output_file).split('/').map(encodeThinkingSegment).join('/');
    loadThinkingRequest(
      'output', key,
      () => window.apiJson(`/app/thinking/api/output/${run.day}/${encoded}`),
      (payload) => { panel.textContent = payload.content || ''; },
      () => { panel.textContent = "couldn't load that output"; },
    );
  }

  function openThinkingPrompt() {
    const run = state.runsDetail;
    if (!run?.name) return;
    const modal = $('thinkingRunsPromptModal');
    state.runsModalFocus = document.activeElement;
    modal.hidden = false;
    if (!state.runsPromptEscapeHandler) {
      state.runsPromptEscapeHandler = (event) => {
        if (event.key === 'Escape') {
          event.preventDefault();
          closeThinkingPrompt();
        }
      };
      document.addEventListener('keydown', state.runsPromptEscapeHandler);
    }
    const content = $('thinkingRunsPromptContent');
    content.textContent = 'loading run details…';
    const key = thinkingCacheKey('prompt', {talent: run.name});
    const cached = readThinkingCache('prompt', key);
    if (cached) {
      content.textContent = cached.content || '';
      return;
    }
    loadThinkingRequest(
      'prompt', key,
      () => window.apiJson(`/app/thinking/api/preview/${encodeThinkingSegment(run.name)}`),
      (payload) => { content.textContent = payload.content || ''; },
      () => { content.textContent = "couldn't load that prompt"; },
    );
  }

  function closeThinkingPrompt() {
    $('thinkingRunsPromptModal').hidden = true;
    if (state.runsPromptEscapeHandler) {
      document.removeEventListener('keydown', state.runsPromptEscapeHandler);
      state.runsPromptEscapeHandler = null;
    }
    state.runsModalFocus?.focus();
    state.runsModalFocus = null;
  }

  function bindThinkingRuns() {
    bindThinkingTablist('thinkingRunsDetailTabs', (tab, origin) => {
      activateThinkingRunDetailTab(tab.id, origin);
    });
    $('thinkingRunsPrevious')?.addEventListener('click', () => navigateThinkingRunsDay(-1));
    $('thinkingRunsNext')?.addEventListener('click', () => navigateThinkingRunsDay(1));
    $('thinkingRunsDate')?.addEventListener('change', (event) => navigateThinkingRunsDay(0, runsDayFromInput(event.target.value)));
    $('thinkingRunsFacet')?.addEventListener('change', (event) => {
      setRunsFacet(event.target.value, Boolean(event.target.value));
      const route = parseThinkingHash();
      if (route?.kind === 'runs') {
        const next = currentThinkingRunsRoute(route);
        window.history.pushState(null, '', thinkingRunsHash(next));
        routeThinkingHash('pointer');
      }
    });
    $('thinkingRunsPrompt')?.addEventListener('click', openThinkingPrompt);
    $('thinkingRunsPromptClose')?.addEventListener('click', closeThinkingPrompt);
  }

  function navigateThinkingRunsDay(amount, requestedDay = '') {
    const route = parseThinkingHash();
    const today = todayThinkingDay();
    const requested = requestedDay || shiftThinkingDay(route?.day || today, amount);
    const day = requested > today ? today : requested;
    const next = currentThinkingRunsRoute({kind: 'runs', day, talent: '', useId: ''});
    window.history.pushState(null, '', thinkingRunsHash(next));
    routeThinkingHash('pointer');
  }

  function providerLabel(provider) {
    return providerLabels[provider] || provider || 'provider';
  }

  function talentLabel(name) {
    const id = String(name || '');
    if (!id) return '';
    return talentLabels[id] || id.replace(/[_:]+/g, ' ');
  }

  function configuredProviders() {
    return Object.entries(state.keys.api_keys || {})
      .filter(([, configured]) => !!configured)
      .map(([provider]) => provider);
  }

  function localEndpointConfigured() {
    return !!state.providers.local_override?.enabled;
  }

  function byoIsUsable() {
    return configuredProviders().length > 0 || localEndpointConfigured();
  }

  function selectedByoProvider() {
    const select = $('byoProvider');
    return state.selectedByoProvider || select?.value || defaultByoProvider();
  }

  function defaultByoProvider() {
    const activeProvider = state.providers.active?.provider || '';
    if (localEndpointConfigured() && activeProvider === 'local') return 'local';
    if (providerEnv[activeProvider]) return activeProvider;
    if (localEndpointConfigured() && configuredProviders().length === 0) return 'local';
    return configuredProviders()[0] || 'anthropic';
  }

  function laneProvider(lane) {
    if (lane === 'local') return 'local';
    return selectedByoProvider();
  }

  function localReadiness() {
    const readiness = state.providers.provider_status?.local;
    if (readiness) {
      const ready = !!(readiness.generate_ready && readiness.cogitate_ready);
      const issue = Array.isArray(readiness.issues) ? readiness.issues[0] : '';
      return {
        status: ready ? 'ready' : 'blocked',
        reason: ready ? 'ready' : (issue || ''),
        summary: issue || '',
        detail: '',
      };
    }
    if (state.localAvailability?.available === true) {
      return {status: 'ready', reason: 'ready', summary: '', detail: ''};
    }
    if (state.localAvailability?.available === false) {
      return {
        status: 'blocked',
        reason: 'availability_unavailable',
        summary: state.localAvailability.reason || '',
        detail: '',
      };
    }
    return {status: '', reason: '', summary: '', detail: ''};
  }

  function localIsReady() {
    const readiness = state.providers.provider_status?.local;
    return !!(readiness?.generate_ready && readiness?.cogitate_ready);
  }

  function localIsGpuBlocked() {
    const reason = localReadiness().reason;
    return reason === 'gpu_unavailable' || reason === 'gpu_probe_failed';
  }

  function activeLanePayload() {
    return state.providers.active_lane || {};
  }

  function confidentialProvenancePresent() {
    return !!activeLanePayload().confidential_provenance_configured;
  }

  function byoKindForProvider(provider) {
    if (provider === 'local') return 'endpoint';
    return 'key';
  }

  function activeBrain() {
    const lane = state.providers.active_lane?.lane || 'none';
    const provider = state.providers.active?.provider || '';
    const byoUsable = byoIsUsable();

    if (lane === 'byo' && byoUsable) {
      return {
        kind: 'byo',
        byoKind: byoKindForProvider(provider),
        provider,
        providerLabel: providerLabel(provider),
      };
    }
    if (lane === 'local' && localIsReady()) {
      return {kind: 'local', providerLabel: 'Local'};
    }
    if (lane === 'confidential' && confidentialProvenancePresent()) {
      return {kind: 'confidential', providerLabel: 'Confidential processing'};
    }
    return {kind: 'none', providerLabel: ''};
  }

  function laneIsUsable(lane) {
    if (lane === 'byo') return byoIsUsable();
    if (lane === 'confidential') return confidentialProvenancePresent();
    if (lane === 'local') return localIsReady() && !localEndpointConfigured();
    return false;
  }

  function relativeTime(iso) {
    if (!iso) return '';
    const stamp = Date.parse(iso);
    if (Number.isNaN(stamp)) return '';
    const seconds = Math.max(0, Math.floor((Date.now() - stamp) / 1000));
    if (seconds < 60) return 'just now';
    const minutes = Math.floor(seconds / 60);
    if (minutes < 60) return `${minutes} min ago`;
    const hours = Math.floor(minutes / 60);
    if (hours < 24) return `${hours} hours ago`;
    return shortDate(iso);
  }

  function shortDate(iso) {
    if (!iso) return '';
    const date = new Date(iso);
    if (Number.isNaN(date.getTime())) return '';
    return date.toLocaleDateString(undefined, {month: 'short', day: 'numeric'}).toLowerCase();
  }

  function renderGlance() {
    const brain = state.providers.brain || {};
    setText('thinkingIntro', brain.state === 'ready' ? 'your processing engine is ready. you can change it in setup.' : 'choose or check your processing engine in setup.');
    const glance = $('brainGlance');
    const glanceLabel = $('thinkingActiveLane');
    const identity = brain.identity || {};
    const evidence = brain.evidence || {};
    const component = brain.failing_component ? ` (${brain.failing_component})` : '';
    if (glance) glance.classList.toggle('none', brain.state === 'unknown' && !brain.reason_code);
    if (glanceLabel) glanceLabel.hidden = false;
    setText('thinkingActiveLane', 'brain health');
    setText('thinkingActiveValue', brain.headline || '');
    if (identity.lane && identity.provider && identity.model) {
      if (brain.state === 'ready') {
        // The brain evidence is the last time generate + cogitate answered, a
        // different measurement from the lane card's attestation check. Name it.
        const confirmed = evidence.age_text ? ` · last confirmed ${evidence.age_text} ago` : '';
        setText('thinkingActiveDetail', `${window.JournalFormat.processingLane(identity.lane)}${confirmed}`);
      } else {
        setText('thinkingActiveDetail', `${window.JournalFormat.processingLane(identity.lane)}: ${brain.reason_text || ''}${component}`);
      }
    } else if (identity.lane || identity.provider || identity.model) {
      setText('thinkingActiveDetail', `${brain.reason_text || ''}${component}`);
    } else {
      setText('thinkingActiveDetail', '');
    }
    const identityText = [identity.lane, identity.provider, identity.model].filter(Boolean).join(' · ');
    setText('thinkingActiveIdentity', identityText);
    $('thinkingIdentityDetails').hidden = !identityText;
    renderBrainAction(brain.action || null);
  }

  function renderBrainAction(action) {
    const button = $('brainCheckAction');
    if (!button) return;
    if (!action?.label) {
      button.hidden = true;
      button.onclick = null;
      return;
    }
    button.hidden = false;
    button.textContent = action.label;
    if (action.href) {
      button.onclick = () => {
        window.location.href = action.href;
      };
      return;
    }
    if (action.refresh) {
      button.onclick = () => requestBrainCheck().catch((err) => {
        setMessage('thinkingActiveDetail', err.message, 'error');
      });
      return;
    }
    button.onclick = null;
  }

  async function requestBrainCheck() {
    const response = await api('api/brain/check', {method: 'POST', body: '{}'});
    if (response.brain) state.providers.brain = response.brain;
    renderGlance();
  }

  async function refreshConfidentialReadiness() {
    await requestBrainCheck();
    await refreshProviders();
  }

  function setCardActive(lane, active) {
    const card = $(`lane-${lane}`);
    if (!card) return;
    card.classList.toggle('active', active);
    const tag = $(`${lane}ActiveTag`);
    if (tag) tag.hidden = !active;
  }

  function setPill(id, label, tone = '') {
    const pill = $(id);
    if (!pill) return;
    pill.textContent = label;
    pill.classList.toggle('hot', tone === 'hot');
    pill.classList.toggle('bad', tone === 'bad');
  }

  function renderConfidentialDetailPanel() {
    const more = $('confidentialLaneMore');
    const panel = $('lane-detail-confidential');
    const detail = copy.confidential?.lane_detail || {};
    if (!more || !panel) return;
    more.textContent = copy.confidential?.more_label || '';
    more.setAttribute('aria-expanded', state.confidentialDetailOpen ? 'true' : 'false');
    panel.hidden = !state.confidentialDetailOpen;
    panel.textContent = '';

    const heading = document.createElement('div');
    heading.className = 'lanedetail-heading';
    heading.textContent = detail.heading || '';
    const sub = document.createElement('div');
    sub.className = 'lanedetail-sub';
    sub.textContent = detail.sub || '';
    panel.append(heading, sub);

    ['mechanism', 'egress'].forEach((key) => {
      const line = document.createElement('div');
      line.className = 'lanedetail-line';
      line.textContent = detail[key] || '';
      panel.appendChild(line);
    });

    const claims = document.createElement('div');
    claims.className = 'lanedetail-claims';
    claims.textContent = detail.claims || '';
    panel.appendChild(claims);

    ['attestation', 'early_access'].forEach((key) => {
      const line = document.createElement('div');
      line.className = 'lanedetail-line';
      line.textContent = detail[key] || '';
      panel.appendChild(line);
    });

    more.onclick = () => {
      state.confidentialDetailOpen = !state.confidentialDetailOpen;
      renderConfidentialDetailPanel();
    };
  }

  function confidentialCheckedLabel(attestation) {
    return relativeTime(attestation?.observed_at || '');
  }

  function renderConfidentialTrust() {
    const beats = copy.confidential?.setup?.trust_beats || {};
    const active = activeLanePayload();
    const attestation = active.confidential_attestation || {state: 'off'};
    const audio = confidentialAudioRender(active, attestation, copy.confidential);
    const deferral = confidentialAudioDeferralLine(active, attestation, copy.confidential);
    setText('confidentialSetupHeading', beats.heading || '');
    setText('confidentialSetupSub', beats.sub || '');
    setText('confidentialTrustHeading', beats.heading || '');
    setText('confidentialTrustSub', beats.sub || '');
    setText('confidentialTrustEgress', confidentialEgressLine(active, beats));
    setHidden('confidentialAudioRow', audio.hidden);
    const audioToggle = $('confidentialAudioToggle');
    if (audioToggle) audioToggle.checked = audio.on;
    setText('confidentialAudioLabel', audio.label);
    setText('confidentialAudioDescription', audio.description);
    setText('confidentialAudioNote', audio.note);
    setText('confidentialAudioDeferral', deferral);
    setHidden('confidentialAudioDeferral', !deferral);
    setText('confidentialTrustClaims', beats.claims || '');
    setText('confidentialTrustFailClosed', beats.attestation || '');
    setText('confidentialTrustSubstrate', beats.substrate || '');
  }

  function renderConfidentialCard() {
    const active = activeLanePayload();
    const lane = laneCopy('confidential');
    const attestation = active.confidential_attestation || {state: 'off'};
    const operation = active.confidential_operation;
    const confidentialCopy = copy.confidential || {};
    const checked = confidentialCheckedLabel(attestation);
    const rendered = confidentialAttestationRender(attestation, confidentialCopy, checked);
    const operationRendered = confidentialOperationRender(operation, confidentialCopy);
    const operationActive = !!operation && !confidentialOperationIsTerminal(operation);

    setCardActive('confidential', activeBrain().kind === 'confidential');
    setPill(
      'confidentialLanePill',
      operationActive ? operation.phase || '' : rendered.pill,
      operationActive ? '' : rendered.tone,
    );
    setText(
      'confidentialLaneDescription',
      operationRendered.message || rendered.message || lane.description || '',
    );
    setText(
      'confidentialLaneStatus',
      operationActive
        ? 'continue in browser →'
        : attestation.state === 'off'
          ? confidentialCopy.actions?.off || ''
          : rendered.recheck
            ? confidentialCopy.actions?.recheck || ''
            : 'manage →',
    );
  }

  function renderConfidentialSetup() {
    renderConfidentialTrust();
    const active = activeLanePayload();
    const attestation = active.confidential_attestation || {state: 'off'};
    const operation = active.confidential_operation;
    const configured = !!active.confidential_provenance_configured;
    const confidentialCopy = copy.confidential || {};
    const checked = confidentialCheckedLabel(attestation);
    const rendered = confidentialAttestationRender(attestation, confidentialCopy, checked);
    const lines = confidentialSetupOperationLines(operation, confidentialCopy, rendered.message);
    const operationActive = !!operation && !confidentialOperationIsTerminal(operation);
    const phase = operation?.phase || '';

    setText('confidentialSetupTitle', activeLaneLabel('confidential'));
    setPill(
      'confidentialSetupPill',
      operationActive ? phase : rendered.pill,
      operationActive ? '' : rendered.tone,
    );
    setText('confidentialSetupState', lines.state);
    setText('confidentialSetupMeta', confidentialSetupMetaLine(attestation, checked));
    setMessage(
      'confidentialLaneOperation',
      lines.operation,
      lines.operationTone,
    );
    setLink(
      'confidentialLaneOperationLink',
      operation?.portal_url || '',
      'continue in browser →',
    );
    setText('confidentialNotice', lines.notice.text);
    setHidden('confidentialNotice', lines.notice.hidden);
    setButtonState('confidentialEnable', !configured && !operationActive, operationActive);
    setButtonText('confidentialEnable', confidentialCopy.actions?.off || '');
    setButtonState('confidentialDisable', configured, operationActive || !configured);
    setButtonText('confidentialDisable', confidentialCopy.actions?.enabled || '');
    setButtonState('confidentialRecheck', configured && rendered.recheck, operationActive);
    setButtonText('confidentialRecheck', confidentialCopy.actions?.recheck || '');
  }

  function renderMainLanes() {
    const brain = activeBrain();
    const localLane = laneCopy('local');
    const confidentialLane = laneCopy('confidential');
    const byoLane = laneCopy('byo');
    setText('localLaneTitle', laneDisplayLabel(localLane));
    setText('confidentialLaneTitle', laneDisplayLabel(confidentialLane));
    setText('byoLaneTitle', laneDisplayLabel(byoLane));
    setText(
      'forkHint',
      brain.kind === 'none'
        ? 'pick one — use local when it is ready, or bring your own.'
        : 'one at a time — the one with the dot is active right now.',
    );

    const local = localReadiness();
    const localCard = $('lane-local');
    const localActive = brain.kind === 'local';
    const gpuBlocked = localIsGpuBlocked();
    const endpointOverride = localEndpointConfigured();
    setCardActive('local', localActive);
    if (localCard) {
      localCard.classList.toggle('greyed', gpuBlocked || endpointOverride);
      localCard.setAttribute('aria-disabled', gpuBlocked ? 'true' : 'false');
    }
    if (localActive) {
      setPill('localLanePill', 'active', 'hot');
      setText('localLaneDescription', localLane.description || '');
      setText('localLaneStatus', 'manage →');
    } else if (endpointOverride) {
      setPill('localLanePill', 'endpoint');
      const managed = state.providers.active_lane?.lane === 'confidential';
      setText('localLaneDescription', managed
        ? 'not in use — the bundled model runs only when no endpoint is set, and confidential processing sets one.'
        : 'not in use — the bundled model runs only when no endpoint is set, and your own endpoint is set.');
      setText('localLaneStatus', 'set up the bundled model →');
    } else if (gpuBlocked) {
      setPill('localLanePill', 'unavailable', 'bad');
      const desc = $('localLaneDescription');
      if (desc) {
        desc.textContent = "this computer can't run a local model yet — it needs a supported GPU. ";
        const link = document.createElement('a');
        link.className = 'textlink';
        link.href = 'https://support.solstone.app/kb/solstone-memory-and-local-models';
        link.target = '_blank';
        link.rel = 'noopener noreferrer';
        link.textContent = 'minimum requirements ↗';
        desc.appendChild(link);
      }
      setText('localLaneStatus', 'not available');
    } else if (local.status === 'ready') {
      setPill('localLanePill', 'off');
      setText('localLaneDescription', localLane.description || '');
      setText('localLaneStatus', 'turn on local →');
    } else {
      setPill('localLanePill', 'off');
      setText('localLaneDescription', local.summary || 'checking whether this computer can run a local model.');
      setText('localLaneStatus', 'set up →');
    }

    renderConfidentialCard();
    renderConfidentialDetailPanel();

    const configured = configuredProviders();
    const activeByo = brain.kind === 'byo';
    const byoProvider = activeByo ? brain.provider : configured[0] || defaultByoProvider();
    setCardActive('byo', activeByo);
    setPill('byoLanePill', activeByo ? 'active' : 'off', activeByo ? 'hot' : '');
    setText('byoLaneDescription', byoLane.description || '');
    if (activeByo) {
      if (brain.byoKind === 'endpoint') {
        setText('byoLaneStatus', 'using endpoint · manage →');
      } else {
        setText('byoLaneStatus', `using ${providerLabel(byoProvider)} key · manage →`);
      }
    } else if (endpointOverride) {
      setText('byoLaneStatus', 'manage endpoint →');
    } else if (configured.length > 0) {
      setText('byoLaneStatus', `manage ${providerLabel(byoProvider)} key →`);
    } else {
      setText('byoLaneStatus', 'add a key or URL →');
    }
  }

  function setSelectedByoProvider(provider) {
    state.selectedByoProvider = provider || defaultByoProvider();
    const select = $('byoProvider');
    if (select) select.value = state.selectedByoProvider;
  }

  function setByoModelResolutionTargets(targets) {
    state.byoModelResolutionTargets = Array.isArray(targets)
      ? targets.filter((target) => typeof target === 'string')
      : [];
  }

  function clearByoModelResolutionTargets() {
    state.byoModelResolutionTargets = [];
  }

  function hasConfidentialPriorResolutionTarget(targets = state.byoModelResolutionTargets) {
    return Array.isArray(targets) && targets.includes('confidential_prior');
  }

  function restoreOnlyModelResolutionActive(targets = state.byoModelResolutionTargets) {
    return hasConfidentialPriorResolutionTarget(targets)
      && state.providers.active_lane?.lane === 'confidential';
  }

  function resetByoDraft() {
    state.byoSelectedModel = '';
    state.byoCustomOpen = false;
    state.byoCustomModel = '';
    state.byoCustomCheckedModel = '';
  }

  function changeByoProvider(provider) {
    clearByoModelResolutionTargets();
    setSelectedByoProvider(provider);
    resetByoDraft();
    const keyInput = $('byoKeyInput');
    if (keyInput) keyInput.value = '';
    const customInput = $('byoCustomModel');
    if (customInput) customInput.value = '';
    const selected = selectedByoProvider();
    const validation = state.keys.key_validation?.[selected];
    const mode = byoEntryMode(selected, validation);
    if (mode === 'model') state.byoSelectedModel = preselectByoModel(selected, state.providers);
    state.byoMode = mode;
    renderByo();
    renderMainLanes();
  }

  function setByoSelectedModel(model) {
    state.byoSelectedModel = String(model || '').trim();
    renderByo();
  }

  function renderByoModelPanel(provider, validation, byoText) {
    const providerName = providerLabel(provider);
    const checked = relativeTime(validation?.timestamp) || relativeTime(new Date().toISOString());
    const selected = state.byoSelectedModel || (state.byoCustomOpen ? '' : preselectByoModel(provider, state.providers));
    const activeModel = state.providers.active?.provider === provider ? state.providers.active?.model || '' : '';
    const rows = byoTierRows(provider, state.providers, activeModel, byoText);
    const catalogModels = new Set(rows.map((row) => row.model).filter(Boolean));
    const selectedIsCustom = !!selected && !catalogModels.has(selected);
    if (!state.byoSelectedModel && selected) state.byoSelectedModel = selected;
    const customText = byoCustomText(selected, selectedIsCustom, state.byoCustomModel);
    if (selectedIsCustom && !state.byoCustomModel && customText) {
      state.byoCustomModel = customText;
      state.byoCustomOpen = true;
    }

    setText('byoKeyCheckstripText', formatCopy(byoText.key_ok_strip || '', {provider: providerName, when: checked}));
    setButtonText('byoCheckAgain', byoText.check_again || '');
    setText('byoModelHeading', byoText.model_heading || '');
    setText('byoModelSub', formatCopy(byoText.model_sub || '', {provider: providerName}));
    renderConfigurationGuidance();
    setButtonText('byoCustomToggle', byoText.custom_toggle || '');
    $('byoCustomToggle')?.setAttribute('aria-expanded', state.byoCustomOpen ? 'true' : 'false');
    setHidden('byoCustomRow', !state.byoCustomOpen);
    setText('byoCustomLabel', byoText.custom_label || '');
    setButtonText('byoCustomCheck', byoText.custom_check || '');
    setButtonText('byoDifferentKey', byoText.use_different_key || '');

    const customInput = $('byoCustomModel');
    if (customInput && document.activeElement !== customInput) {
      customInput.value = state.byoCustomModel;
    }
    const customValue = String(state.byoCustomModel || '').trim();
    if ($('byoCustomCheck')) {
      $('byoCustomCheck').disabled = !customValue;
    }
    if (byoCustomShowsChecked(customValue, state.byoCustomCheckedModel)) {
      setMessage('byoCustomStatus', formatCopy(byoText.custom_ok || '', {model: customValue}), 'ok');
    } else {
      setMessage('byoCustomStatus', '', '');
    }

    const grid = $('byoModelGrid');
    if (grid) {
      grid.innerHTML = '';
      rows.forEach((row) => {
        const card = document.createElement('article');
        card.className = 'prov';
        card.classList.toggle('active', row.model === selected);
        const label = document.createElement('label');
        label.className = 'tierchoice';
        const input = document.createElement('input');
        input.type = 'radio';
        input.name = 'byoModelChoice';
        input.value = row.model;
        input.checked = row.model === selected;
        input.addEventListener('change', () => setByoSelectedModel(input.value));
        const body = document.createElement('span');
        body.className = 'tierbody';
        const top = document.createElement('span');
        top.className = 'cardtop';
        const title = document.createElement('strong');
        title.textContent = row.label;
        top.appendChild(title);
        if (row.tag) {
          const tag = document.createElement('span');
          tag.className = `pill${row.current ? ' hot' : ''}`;
          tag.textContent = row.tag;
          top.appendChild(tag);
        }
        const modelLine = document.createElement('span');
        modelLine.className = 'meta';
        modelLine.textContent = row.model;
        const blurb = document.createElement('span');
        blurb.textContent = row.blurb;
        body.appendChild(top);
        body.appendChild(modelLine);
        body.appendChild(blurb);
        label.appendChild(input);
        label.appendChild(body);
        card.appendChild(label);
        grid.appendChild(card);
      });
    }

    const selectedLabel = byoModelLabel(provider, selected, state.providers);
    const saveCopy = restoreOnlyModelResolutionActive()
      ? byoText.model_save_restore
      : byoText.model_save;
    setButtonText('byoModelSave', formatCopy(saveCopy || '', {label: selectedLabel}));
    if ($('byoModelSave')) {
      $('byoModelSave').disabled = byoSaveDisabled(selected, selectedIsCustom, state.byoCustomCheckedModel);
    }
  }

  function renderByo() {
    if (!state.selectedByoProvider) setSelectedByoProvider(defaultByoProvider());
    const byoText = copy.byo_setup || {};
    const provider = selectedByoProvider();
    const validation = state.keys.key_validation?.[provider];
    const configured = !!state.keys.api_keys?.[provider];
    let mode = state.byoMode;
    if (mode === 'model' && !byoModelStepAllowed(provider, validation)) {
      resetByoDraft();
      mode = 'paste';
      state.byoMode = mode;
    }
    const endpointMode = mode === 'endpoint';
    const pickMode = mode === 'pick';
    const pasteMode = mode === 'paste';
    const modelMode = mode === 'model';

    setText('byoSetupTitle', activeLaneLabel('byo'));
    setText('byoIntro', byoText.intro || '');
    setText('byoModeKey', byoText.chooser_key || '');
    setText('byoModeEndpoint', byoText.chooser_endpoint || '');
    setText('byoPickTitle', byoText.key_heading || '');
    setText('byoPickSub', byoText.key_sub || '');
    setText('byoEndpointTitle', byoText.endpoint_heading || '');
    setText('byoEndpointSub', byoText.endpoint_sub || '');
    setText('byoEndpointHonesty', byoText.endpoint_honesty || '');
    setButtonText('byoSaveKey', byoText.paste_cta || '');
    document.querySelectorAll('[data-byo-key-link]').forEach((link) => {
      link.textContent = byoText.get_key || '';
    });
    const keyModeButton = $('byoModeKey');
    const endpointModeButton = $('byoModeEndpoint');
    if (keyModeButton) {
      keyModeButton.classList.toggle('primary', !endpointMode);
      keyModeButton.setAttribute('aria-pressed', endpointMode ? 'false' : 'true');
    }
    if (endpointModeButton) {
      endpointModeButton.classList.toggle('primary', endpointMode);
      endpointModeButton.setAttribute('aria-pressed', endpointMode ? 'true' : 'false');
    }

    setHidden('byoPickPanel', !(pickMode || endpointMode));
    setHidden('byoProviderGrid', !pickMode);
    setHidden('byoPickTitle', !pickMode);
    setHidden('byoPickSub', !pickMode);
    setHidden('byoEndpointPanel', !endpointMode);
    setHidden('byoPastePanel', !pasteMode);
    setHidden('byoModelPanel', !modelMode);
    setText('byoBackLink', pasteMode || modelMode ? '‹ pick a different provider' : '‹ thinking');

    document.querySelectorAll('[data-provider-card]').forEach((card) => {
      const cardProvider = card.dataset.providerCard;
      const picked = cardProvider === provider;
      card.classList.toggle('active', picked);
      card.classList.toggle('greyed', false);
      const pill = $(`prov-${cardProvider}-pill`);
      if (pill) {
        pill.textContent = picked ? 'selected' : (state.keys.api_keys?.[cardProvider] ? 'saved' : 'pick');
        pill.classList.toggle('hot', picked);
      }
    });

    setText('prov-google-desc', 'use a Google AI Studio key.');
    if (providerEnv[provider]) {
      setText('byoPasteTitle', formatCopy(byoText.paste_title, {provider: providerLabel(provider)}));
      setText('byoKeyLabel', `your ${providerLabel(provider)} key`);
      setText('byoKeyHint', byoText.key_hint || '');
      const terms = $('byoTermsLine');
      if (terms) {
        terms.textContent = `${formatCopy(byoText.terms, {provider: providerLabel(provider)})} `;
        const link = document.createElement('a');
        link.className = 'textlink';
        link.href = providerTerms[provider] || providerTerms.anthropic;
        link.target = '_blank';
        link.rel = 'noopener noreferrer';
        link.textContent = byoText.terms_link || '';
        terms.appendChild(link);
      }
    }

    if ($('byoSaveKey')) $('byoSaveKey').disabled = false;
    if ($('byoClearKey')) $('byoClearKey').disabled = !configured;
    if (pasteMode && validation && validation.valid === false) {
      const reason = byoReasonCopy(validation.reason_code, 'key', byoText, providerLabel(provider));
      setMessage(
        'byoKeyStatus',
        formatCopy(byoText.key_failed || '', {provider: providerLabel(provider), reason}),
        'error',
      );
    } else if (pasteMode && validation && validation.valid === true) {
      setMessage(
        'byoKeyStatus',
        formatCopy(byoText.key_ok_strip || '', {
          provider: providerLabel(provider),
          when: relativeTime(validation.timestamp) || relativeTime(new Date().toISOString()),
        }),
        'ok',
      );
    } else {
      setMessage('byoKeyStatus', pasteMode ? byoText.key_hint || '' : '', '');
    }
    setMessage('byoModelStatus', '', '');
    if (modelMode) {
      renderByoModelPanel(provider, validation, byoText);
    }
  }

  function openGoogleModelResolutionGuidance(guidance) {
    resetByoDraft();
    setByoModelResolutionTargets(guidance?.[googleModelResolutionTargetsField] || []);
    setSelectedByoProvider('google');
    const validation = state.keys.key_validation?.google;
    if (byoModelStepAllowed('google', validation)) {
      state.byoMode = 'model';
      state.byoSelectedModel = preselectByoModel('google', state.providers);
    } else {
      state.byoMode = 'paste';
    }
    renderByo();
    showView('byo-setup');
  }

  function renderConfigurationGuidance() {
    const guidance = state.providers.configuration_guidance;
    const notice = $('byoConfigurationGuidance');
    if (!notice) return;
    notice.textContent = '';
    if (!guidance) {
      setHidden('byoConfigurationGuidance', true);
      return;
    }
    const heading = document.createElement('strong');
    heading.textContent = guidance.heading || '';
    const action = guidance.action || {};
    const link = document.createElement('a');
    link.className = 'textlink';
    link.href = action.href || '#byo-setup';
    link.textContent = action.label || '';
    link.addEventListener('click', (event) => {
      event.preventDefault();
      openGoogleModelResolutionGuidance(guidance);
    });
    notice.append(heading, ' ', link);
    setHidden('byoConfigurationGuidance', false);
  }

  function localCopy() {
    if (localEndpointConfigured()) {
      return {
        pill: 'endpoint',
        title: 'local',
        sub: "you're pointed at your own URL",
        message: '',
        notice: "you're pointed at your own URL — clear it to run the bundled model",
        activate: false,
        bootstrap: false,
        tone: '',
        endpointOverride: true,
      };
    }
    const installOverride = installCopyForStatus(state.install, copy.local_install || {});
    const runtimeOverride = localRuntimeCopy(
      state.providers.local_runtime,
      state.providers.active_lane?.lane === 'local',
      copy.local_recovery || {},
    );
    if (installIsInFlight(state.install)) return installOverride;
    if (
      runtimeOverride
      && ['ready', 'ready-proof-unavailable'].includes(state.providers.local_runtime?.phase)
    ) {
      return runtimeOverride;
    }
    if (installOverride) return installOverride;
    if (runtimeOverride) return runtimeOverride;
    const local = localReadiness();
    const reason = local.reason;
    // Disposition: ready is the only readiness branch that may offer activation.
    if (local.status === 'ready' || reason === 'ready') {
      return {
        pill: 'off',
        title: 'local',
        sub: 'this computer can run a local model',
        message: '',
        notice: copy.glance?.local?.detail || '',
        activate: true,
        bootstrap: false,
        tone: '',
      };
    }
    // Disposition: gpu_unavailable is honest unavailability — do not offer setup
    // when the required hardware is absent.
    if (reason === 'gpu_unavailable') {
      return {
        pill: 'unavailable',
        title: 'local',
        sub: "this computer can't run one yet",
        message: '',
        notice: `this computer doesn't have a supported GPU, so a local model would be too slow to use. you can still use ${activeLaneLabel('byo')}.`,
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    // Disposition: gpu_probe_failed is retained for readiness outcomes even though
    // provider issues do not emit it.
    if (reason === 'gpu_probe_failed') {
      return {
        pill: 'unavailable',
        title: 'local',
        sub: "this computer can't run one yet",
        message: '',
        notice: `couldn't check this computer's GPU. you can still use ${activeLaneLabel('byo')}.`,
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    // Disposition: local_model_installing has no producer in this repository —
    // consumer-only vocabulary, kept inert.
    if (reason === 'local_model_installing') {
      return {
        pill: copy.local_install?.pill_inflight || '',
        title: 'local',
        sub: 'setting up a local model…',
        message: local.detail || local.summary || '',
        notice: copy.local_install?.notice_inflight || '',
        activate: false,
        bootstrap: false,
        tone: '',
      };
    }
    // Disposition: local_model_loading has no producer in this repository —
    // consumer-only vocabulary, kept inert.
    if (reason === 'local_model_loading') {
      return {
        pill: 'starting',
        title: 'local',
        sub: 'starting the local model…',
        message: local.detail || local.summary || '',
        notice: 'local thinking will stay in your journal once the model is ready.',
        activate: false,
        bootstrap: false,
        tone: '',
      };
    }
    // Disposition: bundled setup gaps share one install action across local_model_missing,
    // model_missing, binary_missing, and runtime_missing.
    if (localSetupMissingReasons.has(reason)) {
      return {
        pill: 'setup needed',
        title: 'local',
        sub: 'local setup is not finished yet',
        message: local.detail || local.summary || '',
        notice: 'finish local setup before turning on local thinking.',
        activate: false,
        bootstrap: true,
        bootstrapLabel: copy.local_install?.install || '',
        tone: '',
      };
    }
    // Disposition: local_endpoint_unreachable cannot arrive here; the endpoint
    // override returns before readiness is consulted.
    if (reason === 'local_endpoint_unreachable') {
      return {
        pill: 'not ready',
        title: 'local',
        sub: "your local endpoint didn't answer",
        message: local.detail || local.summary || '',
        notice: `check the endpoint in ${activeLaneLabel('byo')}, then try again.`,
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    // Disposition: local_server_unhealthy and server_unhealthy share the same
    // unhealthy local-service view.
    if (localServerUnhealthyReasons.has(reason)) {
      return {
        pill: 'not ready',
        title: 'local',
        sub: "local thinking isn't ready yet",
        message: local.detail || local.summary || '',
        notice: 'check again after the local service settles.',
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    // Disposition: ram_insufficient is retained for readiness outcomes; provider
    // issues do not emit it.
    if (reason === 'ram_insufficient') {
      return {
        pill: 'unavailable',
        title: 'local',
        sub: 'this computer needs more memory for local thinking',
        message: '',
        notice: `you can still use ${activeLaneLabel('byo')}.`,
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    // Disposition: unknown blocked readiness stays bad and visible; missing
    // readiness data stays neutral.
    if (local.status === 'blocked') {
      return {
        pill: 'not ready',
        title: 'local',
        sub: "couldn't get local processing ready",
        message: local.detail || local.summary || '',
        notice: `try again, or use ${activeLaneLabel('byo')}.`,
        activate: false,
        bootstrap: false,
        tone: 'bad',
      };
    }
    return {
      pill: 'checking',
      title: 'local',
      sub: local.summary || state.localAvailability?.reason || 'checking local readiness.',
      message: '',
      notice: '',
      activate: false,
      bootstrap: false,
      tone: '',
    };
  }

  function renderLocal() {
    const local = localCopy();
    setPill('localSetupPill', local.pill, local.tone);
    setText('localSetupTitle', local.title);
    setText('localSetupSub', local.sub);
    setMessage('localSetupMessage', local.message, local.tone === 'bad' ? 'error' : '');
    setText('localNotice', local.notice);
    setText(
      'localOverrideNoticeText',
      state.providers?.active_lane?.lane === 'confidential'
        ? 'Turn off confidential thinking first, then switch to the bundled local model.'
        : "you're pointed at your own URL — clear it to run the bundled model",
    );
    setHidden('localOverrideNotice', !local.endpointOverride);
    setButtonState('localBootstrap', local.bootstrap, !local.bootstrap);
    setButtonText('localBootstrap', local.bootstrapLabel || copy.local_install?.install || '');
    setButtonState('localRuntimeRetry', local.retryRuntime, !local.retryRuntime);
    setButtonText('localRuntimeRetry', local.retryRuntimeLabel || copy.local_recovery?.retry || '');
    setButtonState('localActivate', local.activate, !local.activate);
    setButtonState('localRefresh', true, false);
    const links = $('localSetupLinks');
    if (links) {
      links.textContent = '';
      if (local.tone === 'bad' && state.install?.install_state !== 'failed') {
        const requirements = document.createElement('a');
        requirements.className = 'textlink';
        requirements.href = 'https://support.solstone.app/kb/solstone-memory-and-local-models';
        requirements.target = '_blank';
        requirements.rel = 'noopener noreferrer';
        requirements.textContent = 'minimum requirements ↗';
        const byo = document.createElement('button');
        byo.type = 'button';
        byo.className = 'textlink';
        byo.dataset.openView = 'byo-setup';
        byo.textContent = activeLaneLabel('byo');
        byo.addEventListener('click', () => showView('byo-setup'));
        links.append(requirements, document.createTextNode(' or use '), byo);
      }
    }
  }

  function renderLocalEndpoint() {
    const endpoint = state.providers.local_override || {};
    if ($('localEndpointUrl')) $('localEndpointUrl').value = endpoint.endpoint_url || '';
    if ($('localEndpointModel')) $('localEndpointModel').value = endpoint.served_model_id || '';
  }

  function renderLocalModels() {
    const select = $('localModelSelect');
    if (!select) return;
    select.innerHTML = '';
    for (const model of state.localModels) {
      const option = document.createElement('option');
      option.value = model.name;
      option.textContent = model.label || model.name;
      select.appendChild(option);
    }
  }

  function byoSetupLabel(brain) {
    if (brain.kind !== 'byo') return activeLaneLabel(brain.kind);
    if (brain.byoKind === 'endpoint') return copy.lane_switch?.setup_endpoint || 'endpoint';
    return brain.providerLabel || activeLaneLabel('byo');
  }

  function byoSavedSetupLabel(provider) {
    if (provider === 'local') return copy.lane_switch?.setup_endpoint || 'your endpoint';
    return copy.lane_switch?.setup_key || 'a saved key';
  }

  function renderLaneSwitch() {
    const brain = activeBrain();
    const switchCopy = copy.lane_switch || {};
    const target = state.pendingSwitchTarget || '';
    const targetProvider = target === 'byo' ? selectedByoProvider() : laneProvider(target);
    const currentLabel = brain.kind === 'none' ? activeLaneLabel('none') : activeLaneLabel(brain.kind);
    const targetLabel = target === 'byo'
      ? activeLaneLabel('byo')
      : activeLaneLabel(target);
    setText('switchHeading', switchCopy.heading || '');
    setText('switchCurrentNodeLabel', switchCopy.current_label || '');
    setText('switchTargetNodeLabel', switchCopy.target_label || '');
    setText('switchCurrentLabel', currentLabel);
    setText('switchTargetLabel', targetLabel);
    setMessage('switchStatus', '');
    if (target === 'byo') {
      setText(
        'switchNote',
        formatCopy(switchCopy.to_byo_note, {setup: byoSavedSetupLabel(targetProvider)}),
      );
    } else if (target === 'local') {
      setText(
        'switchNote',
        formatCopy(switchCopy.to_local_note, {current: byoSetupLabel(brain)}),
      );
    } else {
      setText('switchNote', 'you can switch back anytime.');
    }
    const primary = $('switchConfirmPrimary');
    if (primary) {
      primary.dataset.switchLane = target;
      primary.textContent = switchCopy.confirm || '';
    }
    const cancel = $('switchCancel');
    if (cancel) {
      cancel.textContent = formatCopy(switchCopy.cancel, {current: currentLabel});
    }
  }

  function renderAll() {
    renderGlance();
    renderMainLanes();
    renderByo();
    renderLocalEndpoint();
    renderConfidentialSetup();
    renderLocal();
    renderLaneSwitch();
  }

  async function refreshProvidersPayload() {
    const model = $('localModelSelect')?.value;
    const suffix = model ? `?local_model=${encodeURIComponent(model)}` : '';
    return api(`api/providers${suffix}`);
  }

  async function refreshProviders() {
    state.providers = await refreshProvidersPayload();
    renderAll();
  }

  async function refreshKeys() {
    state.keys = await api('api/keys');
    renderAll();
  }

  function openConsentTab(operation) {
    const url = operation?.portal_url;
    if (url) window.open(url, '_blank', 'noopener');
  }

  async function refreshLocalModels() {
    state.localModels = await api('api/local/models');
    renderLocalModels();
  }

  async function refreshLocalAvailability() {
    const model = $('localModelSelect')?.value || '';
    const suffix = model ? `?model=${encodeURIComponent(model)}` : '';
    state.localAvailability = await api(`api/local/availability${suffix}`);
    renderAll();
  }

  function selectedLocalModelId() {
    return $('localModelSelect')?.value || state.localModels[0]?.name || '';
  }

  function stopInstallPoll() {
    state.installPollGeneration += 1;
  }

  function stopRuntimePoll() {
    state.runtimePollGeneration += 1;
  }

  function stopConfidentialPoll(options = {}) {
    state.confidentialPollGeneration += 1;
    if (options.clearOperation && clearConfidentialInProgressOperation(state.providers.active_lane)) {
      renderAll();
    }
  }

  function applyConfidentialProviders(payload, generation) {
    if (generation !== undefined && generation !== state.confidentialPollGeneration) return false;
    state.providers = payload || state.providers;
    renderAll();
    return true;
  }

  function startConfidentialPoll(initialStatus = null) {
    stopConfidentialPoll();
    const generation = state.confidentialPollGeneration;
    return pollConfidentialUntilTerminal({
      fetchStatus: refreshProvidersPayload,
      sleepFn: sleep,
      applyStatus: (payload) => applyConfidentialProviders(payload, generation),
      isCurrent: () => generation === state.confidentialPollGeneration,
      intervalMs: pollIntervalMs,
      maxElapsedMs: confidentialPollMaxMs,
      initialStatus,
    })
      .then((payload) => {
        if (generation !== state.confidentialPollGeneration) return;
        if (payload === null) {
          if (clearConfidentialInProgressOperation(state.providers.active_lane)) {
            renderAll();
          }
          return;
        }
        stopConfidentialPoll();
      })
      .catch((err) => {
        handleConfidentialPollError({
          generation,
          currentGeneration: () => state.confidentialPollGeneration,
          clearOperation: () => {
            if (clearConfidentialInProgressOperation(state.providers.active_lane)) {
              renderAll();
            }
          },
          stopPoll: stopConfidentialPoll,
          showError: (message) => setMessage('confidentialLaneOperation', message, 'error'),
          error: err,
        });
      });
  }

  async function fetchInstallStatus(model = selectedLocalModelId()) {
    if (!model) return null;
    return api(`api/local/bootstrap/status?model=${encodeURIComponent(model)}`);
  }

  async function fetchLocalRuntime() {
    return api('api/local/runtime');
  }

  function applyLocalRuntime(status, generation) {
    if (generation !== undefined && generation !== state.runtimePollGeneration) return false;
    const currentRevision = state.providers.local_runtime?.health_revision;
    const nextRevision = status?.health_revision;
    const currentRetryRevision = state.providers.local_runtime?.retry_revision;
    const nextRetryRevision = status?.retry_revision;
    if (
      (
        Number.isInteger(currentRevision)
        && Number.isInteger(nextRevision)
        && nextRevision < currentRevision
      )
      || (
        Number.isInteger(currentRetryRevision)
        && Number.isInteger(nextRetryRevision)
        && nextRetryRevision < currentRetryRevision
      )
    ) {
      return false;
    }
    state.providers.local_runtime = status || null;
    renderAll();
    return true;
  }

  function startRuntimePoll(initialStatus = state.providers.local_runtime) {
    stopRuntimePoll();
    const generation = state.runtimePollGeneration;
    return pollLocalRuntimeUntilStable({
      fetchStatus: fetchLocalRuntime,
      sleepFn: sleep,
      applyStatus: (status) => applyLocalRuntime(status, generation),
      isCurrent: () => generation === state.runtimePollGeneration,
      intervalMs: pollIntervalMs,
      initialStatus,
    })
      .then(() => {
        if (generation === state.runtimePollGeneration) stopRuntimePoll();
      })
      .catch((err) => {
        if (generation !== state.runtimePollGeneration) return;
        markLocalRuntimeStale();
        setMessage('localSetupMessage', err.message, 'error');
      });
  }

  function markLocalRuntimeStale() {
    stopRuntimePoll();
    state.providers.local_runtime = {
      status: 'stale',
      phase: 'state-stale',
      reason_code: 'refresh-failed',
      health_revision: null,
      desired_fingerprint_sha256: null,
      retry_revision: null,
      retry_pending: false,
      can_retry: false,
      poll: false,
      updated_at: null,
    };
    renderAll();
  }

  async function refreshLocalRuntime({autoResume = false} = {}) {
    let status;
    try {
      status = await fetchLocalRuntime();
    } catch (err) {
      markLocalRuntimeStale();
      throw err;
    }
    applyLocalRuntime(status);
    if (status?.poll === true && autoResume) {
      startRuntimePoll(status);
    } else {
      stopRuntimePoll();
    }
    return status;
  }

  function applyLocalInstallStatus(status, generation) {
    if (generation !== undefined && generation !== state.installPollGeneration) return false;
    state.install = status || null;
    renderAll();
    return true;
  }

  function startInstallPoll(initialStatus = null) {
    const model = selectedLocalModelId();
    if (!model) return null;
    stopInstallPoll();
    const generation = state.installPollGeneration;
    return pollLocalInstallUntilTerminal({
      fetchStatus: () => fetchInstallStatus(model),
      sleepFn: sleep,
      applyStatus: (status) => applyLocalInstallStatus(status, generation),
      isCurrent: () => generation === state.installPollGeneration,
      intervalMs: pollIntervalMs,
      initialStatus,
    })
      .then((status) => {
        if (generation !== state.installPollGeneration) return;
        if (installIsTerminal(status)) {
          stopInstallPoll();
          if (status?.install_state === 'installed') {
            Promise.all([
              refreshProviders(),
              refreshLocalAvailability(),
              refreshLocalRuntime({autoResume: true}),
            ]).catch((err) => {
              setMessage('localSetupMessage', err.message, 'error');
            });
          }
        }
      })
      .catch((err) => {
        handleInstallPollError({
          generation,
          currentGeneration: () => state.installPollGeneration,
          clearInstallStatus: () => applyLocalInstallStatus(null, generation),
          stopPoll: stopInstallPoll,
          showError: (message) => setMessage('localSetupMessage', message, 'error'),
          error: err,
        });
      });
  }

  async function refreshInstallStatus({autoResume = false} = {}) {
    const status = await fetchInstallStatus();
    applyLocalInstallStatus(status);
    if (installIsInFlight(status) && autoResume) {
      startInstallPoll(status);
    } else if (installIsTerminal(status)) {
      stopInstallPoll();
    }
    return status;
  }

  async function switchLane(lane) {
    const payload = {lane};
    if (lane === 'byo') {
      payload.provider = laneProvider('byo');
    }
    state.providers = await api('api/providers', {
      method: 'PUT',
      body: JSON.stringify(payload),
    });
    renderAll();
  }

  async function activateLane(target) {
    const brain = activeBrain();
    if (brain.kind !== 'none' && brain.kind !== target && laneIsUsable(target)) {
      state.pendingSwitchTarget = target;
      renderLaneSwitch();
      showView('lane-switch');
      return;
    }
    await switchLane(target);
    if (target === 'local') {
      await refreshLocalRuntime({autoResume: true});
      showView('local-setup');
      return;
    }
    showView('main');
  }

  async function enableConfidential() {
    setMessage('confidentialLaneOperation', '');
    let start;
    try {
      start = await api('api/confidential/enable', {method: 'POST'});
    } catch (err) {
      setMessage('confidentialLaneOperation', err.message, 'error');
      return;
    }
    if (state.providers.active_lane) {
      state.providers.active_lane.confidential_operation = start?.operation || null;
      renderAll();
    }
    openConsentTab(start?.operation);
    await startConfidentialPoll();
    if (confidentialEnableNeedsRecheck(state.providers.active_lane)) {
      try {
        await refreshConfidentialReadiness();
      } catch (err) {
        setMessage('confidentialLaneOperation', err.message, 'error');
      }
    }
  }

  async function recheckConfidential() {
    setMessage('confidentialLaneOperation', '');
    await refreshConfidentialReadiness();
  }

  async function disableConfidential() {
    await api('api/confidential/disable', {method: 'POST'});
    await Promise.all([refreshProviders(), refreshKeys(), refreshLocalAvailability()]);
    showView('main');
  }

  async function setConfidentialAudio(enabled) {
    setMessage('confidentialLaneOperation', '');
    try {
      await api('/app/settings/api/config', {
        method: 'PUT',
        body: JSON.stringify({
          section: 'transcribe',
          data: {confidential_audio: enabled},
        }),
      });
    } catch (err) {
      // The write never landed, so authoritative state is unchanged.
      setMessage('confidentialLaneOperation', err.message, 'error');
      renderConfidentialSetup();
      return;
    }
    try {
      await refreshProviders();
    } catch (err) {
      // The write landed but the re-read failed; stale payload may under-claim what leaves.
      setMessage('confidentialLaneOperation', err.message, 'error');
    }
  }

  async function saveByoKey() {
    const provider = laneProvider('byo');
    const value = $('byoKeyInput')?.value || '';
    const result = await runByoKeyCheckFlow({
      apiFn: api,
      applyKeys: (keys) => {
        state.keys = keys;
      },
      provider,
      providerName: providerLabel(provider),
      envVar: providerEnv[provider],
      value,
      text: copy.byo_setup || {},
      providersPayload: state.providers,
      setMode: (mode) => {
        state.byoMode = mode;
      },
      selectModel: (model) => {
        state.byoSelectedModel = model;
      },
      resetDraft: resetByoDraft,
      renderFn: renderByo,
      showStatus: (message, tone) => setMessage('byoKeyStatus', message, tone),
    });
    if (result.status !== 'empty' && $('byoKeyInput')) $('byoKeyInput').value = '';
  }

  async function clearByoKey() {
    const provider = laneProvider('byo');
    const result = await api('api/keys', {
      method: 'PUT',
      body: JSON.stringify({env_var: providerEnv[provider], value: ''}),
    });
    state.keys = result;
    if (state.providers.byo_models) delete state.providers.byo_models[provider];
    resetByoDraft();
    state.byoMode = 'paste';
    if ($('byoKeyInput')) $('byoKeyInput').value = '';
    renderAll();
    await refreshProviders();
  }

  async function recheckByoKey() {
    const provider = laneProvider('byo');
    const byoText = copy.byo_setup || {};
    setMessage('byoModelStatus', formatCopy(byoText.checking_key || '', {provider: providerLabel(provider)}), '');
    const result = await api('api/validate-keys', {method: 'POST'});
    state.keys.key_validation = result.key_validation || {};
    const validation = state.keys.key_validation?.[provider] || {};
    if (byoModelStepAllowed(provider, validation)) {
      if (!state.byoSelectedModel) state.byoSelectedModel = preselectByoModel(provider, state.providers);
      state.byoMode = 'model';
      renderByo();
      return;
    }
    resetByoDraft();
    state.byoMode = 'paste';
    renderByo();
    if (validation.valid === false) {
      const reason = byoReasonCopy(validation.reason_code, 'key', byoText, providerLabel(provider));
      setMessage('byoKeyStatus', formatCopy(byoText.key_failed || '', {provider: providerLabel(provider), reason}), 'error');
    }
  }

  async function probeByoCustomModel() {
    const provider = laneProvider('byo');
    const model = String(state.byoCustomModel || '').trim();
    if (!model) return;
    await runByoCustomProbeFlow({
      apiFn: api,
      provider,
      providerName: providerLabel(provider),
      model,
      text: copy.byo_setup || {},
      setMode: (mode) => {
        state.byoMode = mode;
      },
      selectModel: (candidate) => {
        state.byoSelectedModel = candidate;
      },
      markChecked: (candidate) => {
        state.byoCustomCheckedModel = candidate;
      },
      renderFn: renderByo,
      showStatus: (message, tone) => {
        const id = state.byoMode === 'paste' ? 'byoKeyStatus' : 'byoCustomStatus';
        setMessage(id, message, tone);
      },
    });
  }

  async function saveByoModel() {
    const provider = laneProvider('byo');
    const model = String(state.byoSelectedModel || '').trim();
    if (!model) return;
    const googleModelResolutionTargets = state.byoModelResolutionTargets.slice();
    const modelLabel = byoModelLabel(provider, model, state.providers);
    const result = await runByoModelSaveFlow({
      apiFn: api,
      applyProviders: (providers) => {
        state.providers = providers;
      },
      provider,
      providerName: providerLabel(provider),
      model,
      modelLabel,
      googleModelResolutionTargets,
      text: copy.byo_setup || {},
      setMode: (mode) => {
        state.byoMode = mode;
      },
      renderFn: renderByo,
      showStatus: (message, tone) => setMessage(state.byoMode === 'paste' ? 'byoKeyStatus' : 'byoModelStatus', message, tone),
    });
    if (result.status === 'saved') {
      clearByoModelResolutionTargets();
      showView('main');
      renderAll();
    } else if (result.status === 'restored') {
      clearByoModelResolutionTargets();
    }
  }

  async function saveLocalEndpoint() {
    const payload = {
      endpoint_url: $('localEndpointUrl')?.value || '',
      served_model_id: $('localEndpointModel')?.value || '',
    };
    const credential = $('localEndpointCredential')?.value;
    if (credential) payload.credential = credential;
    const result = await api('api/local/endpoint', {
      method: 'POST',
      body: JSON.stringify(payload),
    });
    state.providers.local_override = result.local_endpoint || {};
    if ($('localEndpointCredential')) $('localEndpointCredential').value = '';
    setMessage('localEndpointStatus', 'endpoint saved', 'ok');
    setSelectedByoProvider('local');
    state.byoMode = 'endpoint';
    await switchLane('byo');
    await Promise.all([refreshProviders(), refreshLocalAvailability()]);
    showView('main');
  }

  async function clearLocalEndpoint() {
    const result = await api('api/local/endpoint', {method: 'DELETE'});
    state.providers.local_override = result.local_endpoint || {};
    if (selectedByoProvider() === 'local') {
      setSelectedByoProvider(defaultByoProvider());
      state.byoMode = 'pick';
    }
    setMessage('localEndpointStatus', 'endpoint cleared', 'ok');
    await Promise.all([refreshProviders(), refreshLocalAvailability()]);
  }

  async function startLocalBootstrap() {
    const model = $('localModelSelect')?.value || '';
    const status = await api(`api/local/bootstrap?model=${encodeURIComponent(model)}`, {method: 'POST'});
    state.install = status || null;
    renderAll();
    if (installIsInFlight(status)) {
      startInstallPoll(status);
    } else {
      await refreshInstallStatus({autoResume: true});
    }
    await Promise.all([
      refreshProviders(),
      refreshLocalAvailability(),
      refreshLocalRuntime({autoResume: true}),
    ]);
  }

  async function retryLocalRuntime() {
    const runtime = state.providers.local_runtime;
    if (!runtime?.can_retry) return;
    const button = $('localRuntimeRetry');
    if (button) button.disabled = true;
    try {
      const status = await api('api/local/runtime/retry', {
        method: 'POST',
        body: JSON.stringify({
          health_revision: runtime.health_revision,
          retry_revision: runtime.retry_revision,
          desired_fingerprint_sha256: runtime.desired_fingerprint_sha256,
        }),
      });
      applyLocalRuntime(status);
      startRuntimePoll(status);
    } catch (_err) {
      await refreshLocalRuntime({autoResume: true});
    }
  }

  function openLane(lane) {
    if (lane === 'confidential') {
      showView('confidential-setup');
      return;
    }
    if (lane === 'local' && laneIsUsable(lane) && activeBrain().kind !== lane) {
      activateLane(lane).catch((err) => setMessage(`${lane}LaneStatus`, err.message, 'error'));
      return;
    }
    if (lane === 'byo') {
      clearByoModelResolutionTargets();
      const provider = defaultByoProvider();
      setSelectedByoProvider(provider);
      resetByoDraft();
      const validation = state.keys.key_validation?.[provider];
      const mode = byoEntryMode(provider, validation);
      if (mode === 'model') state.byoSelectedModel = preselectByoModel(provider, state.providers);
      state.byoMode = mode === 'paste' && configuredProviders().length === 0 && activeBrain().kind !== 'byo' ? 'pick' : mode;
      renderByo();
    }
    showView(`${lane}-setup`);
  }

  function bindOpenView(el) {
    el.addEventListener('click', (event) => {
      event.preventDefault();
      event.stopPropagation();
      const lane = el.dataset.lane || el.closest('[data-lane]')?.dataset.lane;
      if (lane) {
        openLane(lane);
      } else {
        showView(el.dataset.openView || 'main');
      }
    });
    el.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter' && event.key !== ' ') return;
      event.preventDefault();
      const lane = el.dataset.lane || el.closest('[data-lane]')?.dataset.lane;
      if (lane) {
        openLane(lane);
      } else {
        showView(el.dataset.openView || 'main');
      }
    });
  }

  function bind() {
    document.querySelectorAll('[data-open-view]').forEach(bindOpenView);
    $('byoModeKey')?.addEventListener('click', () => {
      if (selectedByoProvider() === 'local') {
        setSelectedByoProvider(configuredProviders()[0] || 'anthropic');
      }
      resetByoDraft();
      state.byoMode = 'pick';
      renderByo();
      renderMainLanes();
    });
    $('byoModeEndpoint')?.addEventListener('click', () => {
      setSelectedByoProvider('local');
      resetByoDraft();
      state.byoMode = 'endpoint';
      renderByo();
      renderMainLanes();
    });
    document.querySelectorAll('[data-byo-provider]').forEach((button) => {
      button.addEventListener('click', () => {
        changeByoProvider(button.dataset.byoProvider);
      });
    });
    document.querySelectorAll('[data-switch-lane]').forEach((button) => {
      button.addEventListener('click', () => {
        const lane = button.dataset.switchLane;
        if (!lane) return;
        switchLane(lane)
          .then(() => showView('main'))
          .catch((err) => setMessage('switchStatus', err.message, 'error'));
      });
    });
    $('byoProvider')?.addEventListener('change', () => {
      changeByoProvider($('byoProvider')?.value || defaultByoProvider());
    });
    $('byoBackLink')?.addEventListener('click', () => {
      if (state.byoMode === 'paste' || state.byoMode === 'model') {
        resetByoDraft();
        state.byoMode = 'pick';
        renderByo();
        return;
      }
      showView('main');
    });
    $('byoSaveKey')?.addEventListener('click', () => saveByoKey().catch((err) => setMessage('byoKeyStatus', err.message, 'error')));
    $('byoClearKey')?.addEventListener('click', () => clearByoKey().catch((err) => setMessage('byoKeyStatus', err.message, 'error')));
    $('byoCheckAgain')?.addEventListener('click', () => recheckByoKey().catch((err) => setMessage('byoModelStatus', err.message, 'error')));
    $('byoCustomToggle')?.addEventListener('click', () => {
      state.byoCustomOpen = !state.byoCustomOpen;
      renderByo();
    });
    $('byoCustomModel')?.addEventListener('input', (event) => {
      const draft = byoCustomInputDraft(event.target.value);
      state.byoCustomModel = draft.customModel;
      state.byoCustomCheckedModel = draft.checkedModel;
      state.byoSelectedModel = draft.selectedModel;
      renderByo();
    });
    $('byoCustomCheck')?.addEventListener('click', () => probeByoCustomModel().catch((err) => setMessage('byoCustomStatus', err.message, 'error')));
    $('byoModelSave')?.addEventListener('click', () => saveByoModel().catch((err) => setMessage('byoModelStatus', err.message, 'error')));
    $('byoDifferentKey')?.addEventListener('click', () => {
      resetByoDraft();
      state.byoMode = 'paste';
      const keyInput = $('byoKeyInput');
      if (keyInput) keyInput.value = '';
      renderByo();
    });
    $('confidentialEnable')?.addEventListener('click', () => enableConfidential().catch((err) => setMessage('confidentialLaneOperation', err.message, 'error')));
    $('confidentialRecheck')?.addEventListener('click', () => recheckConfidential().catch((err) => setMessage('confidentialLaneOperation', err.message, 'error')));
    $('confidentialDisable')?.addEventListener('click', () => disableConfidential().catch((err) => setMessage('confidentialLaneOperation', err.message, 'error')));
    $('confidentialAudioToggle')?.addEventListener('change', (event) => setConfidentialAudio(event.target.checked));
    $('localRefresh')?.addEventListener('click', () => {
      stopInstallPoll();
      stopRuntimePoll();
      stopConfidentialPoll();
      Promise.all([
        refreshProviders(),
        refreshLocalAvailability(),
        refreshInstallStatus({autoResume: true}),
        refreshLocalRuntime({autoResume: true}),
      ]).catch((err) => setMessage('localSetupMessage', err.message, 'error'));
    });
    $('localBootstrap')?.addEventListener('click', () => startLocalBootstrap().catch((err) => setMessage('localSetupMessage', err.message, 'error')));
    $('localRuntimeRetry')?.addEventListener('click', () => retryLocalRuntime().catch((err) => setMessage('localSetupMessage', err.message, 'error')));
    $('localActivate')?.addEventListener('click', () => activateLane('local').catch((err) => setMessage('localSetupMessage', err.message, 'error')));
    $('localModelSelect')?.addEventListener('change', () => {
      stopInstallPoll();
      stopRuntimePoll();
      stopConfidentialPoll();
      state.install = null;
      renderAll();
      Promise.all([
        refreshLocalAvailability(),
        refreshProviders(),
        refreshInstallStatus({autoResume: true}),
        refreshLocalRuntime({autoResume: true}),
      ]).catch((err) => setMessage('localSetupMessage', err.message, 'error'));
    });
    $('localEndpointSave')?.addEventListener('click', () => saveLocalEndpoint().catch((err) => setMessage('localEndpointStatus', err.message, 'error')));
    $('localEndpointClear')?.addEventListener('click', () => clearLocalEndpoint().catch((err) => setMessage('localEndpointStatus', err.message, 'error')));
    $('localEndpointClearFromLocal')?.addEventListener('click', () => clearLocalEndpoint().catch((err) => setMessage('localSetupMessage', err.message, 'error')));
    window.addEventListener('hashchange', () => routeThinkingHash('history'));
  }

  async function init() {
    const loaded = await loadInitialState();
    if (!loaded) return;
    bind();
    bindThinkingSectionTabs();
    bindThinkingRuns();
    setSelectedByoProvider(defaultByoProvider());
    renderAll();
    routeThinkingHash('reload');
    try {
      await refreshLocalModels();
      await refreshInstallStatus({autoResume: true});
      await refreshLocalRuntime({autoResume: viewFromHash() === 'local-setup'});
      await refreshLocalAvailability();
      await Promise.all([refreshProviders(), refreshKeys()]);
    } catch (err) {
      setMessage('thinkingActiveDetail', err.message, 'error');
    }
  }

  init();
})();
