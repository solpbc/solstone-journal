// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  'use strict';

  const API_ESTIMATE = '/app/export/api/estimate';
  const API_ESTIMATE_REFRESH = '/app/export/api/estimate/refresh';
  const API_STATE = '/app/export/api/state';
  const API_JOBS = '/app/export/api/jobs';

  const POLL_FAST_MS = 1000;
  const POLL_READY_MS = 15000;

  function t(id, args) {
    if (!global.JournalI18n || typeof global.JournalI18n.render !== 'function') {
      return id;
    }
    return global.JournalI18n.render(id, args || []);
  }

  function formatBytes(bytes) {
    const n = Number(bytes);
    if (!Number.isFinite(n) || n <= 0) return '0 B';
    const units = ['B', 'KB', 'MB', 'GB', 'TB'];
    let value = n;
    let idx = 0;
    while (value >= 1000 && idx < units.length - 1) {
      value /= 1000;
      idx += 1;
    }
    if (idx === 0) return `${Math.round(value)} B`;
    return `${value >= 100 ? value.toFixed(0) : value.toFixed(1)} ${units[idx]}`;
  }

  function formatRate(bytesPerSec) {
    const n = Number(bytesPerSec);
    if (!Number.isFinite(n) || n <= 0) return '';
    return `${formatBytes(n)}/s`;
  }

  function formatDays(count) {
    return t('journal.window.export.days.count', [['count', Math.max(0, Number(count) || 0)]]);
  }

  function formatFiles(count) {
    return t('journal.window.export.files.count', [['count', Math.max(0, Number(count) || 0)]]);
  }

  function formatPreparedTime(isoString) {
    if (!isoString) return '';
    const date = new Date(isoString);
    if (Number.isNaN(date.getTime())) return isoString;
    try {
      return date.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
    } catch (_err) {
      return isoString;
    }
  }

  function minutesSince(isoString, nowMs) {
    if (!isoString) return 0;
    const ms = Date.parse(isoString);
    if (!Number.isFinite(ms)) return 0;
    return Math.max(0, Math.floor((nowMs - ms) / 60000));
  }

  function getExportContract() {
    const contract = global.solstoneExportContract;
    if (!contract || typeof contract !== 'object') return null;
    if (contract.version !== 2) return null;
    if (
      typeof contract.capability !== 'string' ||
      typeof contract.start !== 'string' ||
      typeof contract.status !== 'string' ||
      typeof contract.event !== 'string'
    ) {
      return null;
    }
    return contract;
  }

  function getNativeExportHost() {
    if (global.top && global.top !== global) return null;
    const contract = getExportContract();
    if (!contract) return null;
    const host = global[contract.capability];
    if (!host || typeof host !== 'object') return null;
    if (host.version !== contract.version) return null;
    if (typeof host.send !== 'function' || !Array.isArray(host.types)) return null;
    if (!host.types.includes(contract.start) || !host.types.includes(contract.status)) {
      return null;
    }
    return { host, contract };
  }

  function isOlderPairedHost() {
    if (getNativeExportHost() !== null) return false;
    const contract = getExportContract();
    if (contract && typeof global[contract.capability] !== 'undefined') return true;
    const webHost = global.solstoneJournalWebHost;
    if (webHost && typeof webHost.javascriptCapability === 'string' && global[webHost.javascriptCapability]) {
      return true;
    }
    return false;
  }

  function el(doc, tag, className, text) {
    const node = doc.createElement(tag);
    if (className) node.className = className;
    if (text != null) node.textContent = text;
    return node;
  }

  function createButton(doc, opts) {
    const btn = el(doc, 'button', ['export-btn', opts.variant || ''].filter(Boolean).join(' '), opts.label);
    btn.setAttribute('type', 'button');
    if (opts.id) btn.setAttribute('id', opts.id);
    if (opts.focusKey) btn.setAttribute('data-export-focus', opts.focusKey);
    if (opts.disabled) btn.setAttribute('disabled', '');
    if (opts.onClick) btn.addEventListener('click', opts.onClick);
    return btn;
  }

  function createDetails(doc, foldKey, summaryText, openDefault, openMap, buildBody) {
    const details = el(doc, 'details', 'export-fold');
    details.setAttribute('data-export-fold', foldKey);
    const shouldOpen = Object.prototype.hasOwnProperty.call(openMap, foldKey)
      ? Boolean(openMap[foldKey])
      : Boolean(openDefault);
    if (shouldOpen) details.setAttribute('open', '');
    details.addEventListener('toggle', () => {
      openMap[foldKey] = details.hasAttribute('open');
    });
    const summary = el(doc, 'summary', null, summaryText);
    summary.setAttribute('data-export-focus', `fold:${foldKey}`);
    details.appendChild(summary);
    const body = el(doc, 'div', 'export-fold-body');
    buildBody(body);
    details.appendChild(body);
    return details;
  }

  function createMoveGuide(doc, openMap) {
    return createDetails(
      doc,
      'move-guide',
      t('journal.window.export.move.summary'),
      false,
      openMap,
      (body) => {
        const list = el(doc, 'ol', 'export-steps');
        list.appendChild(el(doc, 'li', null, t('journal.window.export.move.step1')));
        list.appendChild(el(doc, 'li', null, t('journal.window.export.move.step2')));
        list.appendChild(el(doc, 'li', null, t('journal.window.export.move.step3')));
        list.appendChild(el(doc, 'li', null, t('journal.window.export.move.step4')));
        body.appendChild(list);
      }
    );
  }

  function createCodeLine(doc, text, extraButton) {
    const wrap = el(doc, 'div', 'export-code');
    wrap.appendChild(el(doc, 'code', null, text));
    if (extraButton) wrap.appendChild(extraButton);
    return wrap;
  }

  function isMobilePlatform(platform) {
    return platform === 'ios' || platform === 'ipados' || platform === 'android' ||
      platform === 'ios_host' || platform === 'android_host';
  }

  function stagingDriveName(estimate) {
    return estimate && estimate.staging_volume_same_as_journal === false
      ? t('journal.window.export.drive.temp')
      : t('journal.window.export.drive.journal');
  }

  function stagingDriveLabel(estimate) {
    return estimate && estimate.staging_volume_same_as_journal === false
      ? t('journal.window.export.drive.label.temp')
      : t('journal.window.export.drive.label.journal');
  }

  function renderOffloadNotice(doc, estimate) {
    const breakdown = estimate && estimate.breakdown;
    if (!breakdown || !breakdown.already_offloaded) return null;
    const off = breakdown.already_offloaded;
    const backupState = (estimate.backup && estimate.backup.state) || 'not_configured';

    if (off.degraded) {
      const band = el(doc, 'div', 'export-band is-warn');
      band.setAttribute('data-export-notice', 'offload-degraded');
      band.appendChild(el(doc, 'span', 'export-band-title', t('journal.window.export.offload.degraded.title')));
      band.appendChild(el(doc, 'p', null, t('journal.window.export.offload.degraded.body')));
      const actions = el(doc, 'div', 'export-band-actions');
      const link = el(doc, 'a', 'export-inline-link', t('journal.window.export.open.backup'));
      link.setAttribute('href', '/app/backup/');
      actions.appendChild(link);
      band.appendChild(actions);
      return band;
    }

    if (!off.days || off.days <= 0) return null;

    const daysLabel = formatDays(off.days);
    const sizeLabel = formatBytes(off.bytes);
    let tone = 'is-calm';
    let titleId = 'journal.window.export.offload.healthy.title';
    let bodyId = 'journal.window.export.offload.healthy.body';
    let linkId = 'journal.window.export.offload.healthy.link';
    let noticeName = 'offload-healthy';

    if (backupState === 'paused') {
      tone = 'is-warn';
      titleId = 'journal.window.export.offload.paused.title';
      bodyId = 'journal.window.export.offload.paused.body';
      linkId = 'journal.window.export.open.backup';
      noticeName = 'offload-paused';
    } else if (backupState === 'unreachable') {
      tone = 'is-warn';
      titleId = 'journal.window.export.offload.unreachable.title';
      bodyId = 'journal.window.export.offload.unreachable.body';
      linkId = 'journal.window.export.open.backup';
      noticeName = 'offload-unreachable';
    }

    const band = el(doc, 'div', `export-band ${tone}`);
    band.setAttribute('data-export-notice', noticeName);
    band.appendChild(el(doc, 'span', 'export-band-title', t(titleId, [['days', daysLabel]])));
    band.appendChild(el(doc, 'p', null, t(bodyId, [['size', sizeLabel]])));
    const actions = el(doc, 'div', 'export-band-actions');
    const link = el(doc, 'a', 'export-inline-link', t(linkId));
    link.setAttribute('href', '/app/backup/');
    actions.appendChild(link);
    band.appendChild(actions);
    return band;
  }

  function renderBreakdown(doc, estimate, selectedMode, openMap) {
    const isMeasuring = !estimate || estimate.status === 'measuring';
    const breakdown = estimate && estimate.breakdown;
    const off = breakdown && breakdown.already_offloaded;
    const hasOffloaded = Boolean(off && off.days > 0);
    const isDegraded = Boolean(off && off.degraded);
    const defaultOpen = selectedMode === 'without_processed_media' || hasOffloaded || isDegraded;

    return createDetails(
      doc,
      'breakdown',
      t('journal.window.export.breakdown.summary'),
      defaultOpen,
      openMap,
      (body) => {
        body.appendChild(el(doc, 'p', null, t('journal.window.export.breakdown.intro')));
        const table = el(doc, 'table', 'export-table');
        const thead = el(doc, 'thead');
        const headRow = el(doc, 'tr');
        headRow.appendChild(el(doc, 'th', null, t('journal.window.export.breakdown.col.category')));
        headRow.appendChild(el(doc, 'th', null, t('journal.window.export.breakdown.col.size')));
        headRow.appendChild(el(doc, 'th', null, t('journal.window.export.breakdown.col.status')));
        thead.appendChild(headRow);
        table.appendChild(thead);

        const tbody = el(doc, 'tbody');
        const counting = t('journal.window.export.breakdown.counting');

        function addRow(title, note, sizeText, statusClass, statusText) {
          const tr = el(doc, 'tr');
          const c1 = el(doc, 'td');
          c1.appendChild(el(doc, 'div', null, title));
          if (note) c1.appendChild(el(doc, 'div', 'export-faint', note));
          tr.appendChild(c1);
          tr.appendChild(el(doc, 'td', 'export-num', sizeText));
          const c3 = el(doc, 'td');
          c3.appendChild(el(doc, 'span', `export-pop ${statusClass}`, statusText));
          tr.appendChild(c3);
          tbody.appendChild(tr);
        }

        const proc = breakdown && breakdown.processed_media_omitted;
        const unproc = breakdown && breakdown.unprocessed_or_failed_media;
        const rest = breakdown && breakdown.kept_non_media;

        addRow(
          t('journal.window.export.breakdown.processed.title'),
          proc ? t('journal.window.export.breakdown.processed.note', [['files', formatFiles(proc.files)]]) : '',
          isMeasuring || !proc ? counting : formatBytes(proc.bytes),
          selectedMode === 'without_processed_media' ? 'is-omit' : 'is-keep',
          selectedMode === 'without_processed_media'
            ? t('journal.window.export.breakdown.status.omitted')
            : t('journal.window.export.breakdown.status.included')
        );

        const backupAmount = isMeasuring || !off
          ? counting
          : off.degraded
            ? t('journal.window.export.breakdown.unknown')
            : off.days > 0
              ? t('journal.window.export.breakdown.offloaded.amount', [
                  ['size', formatBytes(off.bytes)],
                  ['days', formatDays(off.days)],
                ])
              : formatFiles(0);
        const backupClass = off && (off.degraded || off.days > 0) ? 'is-backup' : 'is-keep';
        const backupLabel = off && off.degraded
          ? t('journal.window.export.breakdown.status.unreadable')
          : off && off.days > 0
            ? t('journal.window.export.breakdown.status.backup')
            : t('journal.window.export.breakdown.status.none');
        addRow(
          t('journal.window.export.breakdown.offloaded.title'),
          t('journal.window.export.breakdown.offloaded.note'),
          backupAmount,
          backupClass,
          backupLabel
        );

        addRow(
          t('journal.window.export.breakdown.unprocessed.title'),
          unproc ? t('journal.window.export.breakdown.unprocessed.note', [['files', formatFiles(unproc.files)]]) : '',
          isMeasuring || !unproc ? counting : formatBytes(unproc.bytes),
          'is-keep',
          t('journal.window.export.breakdown.status.included')
        );

        addRow(
          t('journal.window.export.breakdown.rest.title'),
          rest ? t('journal.window.export.breakdown.rest.note', [['files', formatFiles(rest.files)]]) : '',
          isMeasuring || !rest ? counting : formatBytes(rest.bytes),
          'is-keep',
          t('journal.window.export.breakdown.status.included')
        );

        table.appendChild(tbody);
        body.appendChild(table);
        body.appendChild(
          el(
            doc,
            'p',
            'export-faint',
            `${t('journal.window.export.breakdown.always.excluded')} ${t('journal.window.export.breakdown.external.note')}`
          )
        );
      }
    );
  }

  function renderSpacePreflight(doc, estimate, selectedMode, openMap, actions) {
    if (!estimate || estimate.status !== 'ready' || !estimate.breakdown) return null;
    const freeBytes = estimate.journal_disk_free_bytes;
    if (typeof freeBytes !== 'number') return null;

    const fullReq = estimate.full_required_bytes;
    const lightReq = estimate.light_required_bytes;
    const fullFits = estimate.full_fits !== false;
    const lightFits = estimate.light_fits !== false;
    const isMac = estimate.journal_host_os === 'macos';
    const cliFull = estimate.cli_command_full || 'solstone journal archive export --out /path/to/drive/journal.zip';

    const driveName = stagingDriveName(estimate);
    const driveLabel = stagingDriveLabel(estimate);

    if (!lightFits) {
      const band = el(doc, 'div', 'export-band is-err');
      band.setAttribute('data-export-space', 'both-tight');
      band.appendChild(el(doc, 'span', 'export-band-title', t('journal.window.export.space.both.title', [['drive', driveName]])));
      band.appendChild(
        el(
          doc,
          'p',
          null,
          t(isMac ? 'journal.window.export.space.both.macos' : 'journal.window.export.space.both.other', [
            ['drive', driveName],
            ['free', formatBytes(freeBytes)],
            ['light', formatBytes(lightReq || estimate.breakdown.light_bytes)],
          ])
        )
      );
      band.appendChild(createCodeLine(doc, cliFull));
      return band;
    }

    if (selectedMode === 'full' && !fullFits) {
      const band = el(doc, 'div', 'export-band is-warn');
      band.setAttribute('data-export-space', 'full-tight');
      band.appendChild(el(doc, 'span', 'export-band-title', t('journal.window.export.space.tight.title', [['drive', driveName]])));
      band.appendChild(
        el(
          doc,
          'p',
          null,
          t(isMac ? 'journal.window.export.space.tight.macos' : 'journal.window.export.space.tight.other', [
            ['drive', driveName],
            ['needed', formatBytes(fullReq || estimate.breakdown.full_bytes)],
            ['free', formatBytes(freeBytes)],
            ['light', formatBytes(estimate.breakdown.light_bytes)],
          ])
        )
      );
      const actRow = el(doc, 'div', 'export-band-actions');
      actRow.appendChild(
        createButton(doc, {
          id: 'exportSwitchLightBtn',
          focusKey: 'switch-light',
          variant: 'is-small',
          label: t('journal.window.export.space.switch.light'),
          onClick: () => actions.selectMode('without_processed_media'),
        })
      );
      band.appendChild(actRow);
      band.appendChild(
        createDetails(
          doc,
          'external-drive-cli',
          t('journal.window.export.space.cli.summary'),
          false,
          openMap,
          (body) => {
            body.appendChild(
              el(
                doc,
                'p',
                null,
                t(isMac ? 'journal.window.export.space.cli.macos' : 'journal.window.export.space.cli.other')
              )
            );
            body.appendChild(createCodeLine(doc, cliFull));
          }
        )
      );
      return band;
    }

    const neededBytes = selectedMode === 'full'
      ? (fullReq || estimate.breakdown.full_bytes)
      : (lightReq || estimate.breakdown.light_bytes);
    const row = el(doc, 'div', 'export-space-row');
    row.setAttribute('data-export-space', 'ok');
    row.appendChild(
      el(
        doc,
        'span',
        null,
        t('journal.window.export.space.ok', [
          ['drive', driveLabel],
          ['free', formatBytes(freeBytes)],
          ['needed', formatBytes(neededBytes)],
        ])
      )
    );
    return row;
  }

  function renderErrorCard(doc, errInfo, estimate, actions) {
    const isMac = estimate && estimate.journal_host_os === 'macos';
    const code = errInfo.code || 'prepare_failed';
    let tone = 'is-err';
    let title = t('journal.window.export.err.generic.title');
    let body = t('journal.window.export.err.generic.body');
    let primaryBtn = null;
    let extraCli = null;

    if (code === 'source_changed') {
      tone = 'is-warn';
      title = t('journal.window.export.err.source.changed.title');
      body = t('journal.window.export.err.source.changed.body');
      primaryBtn = createButton(doc, {
        focusKey: 'err-retry',
        variant: 'is-primary',
        label: t('journal.window.export.err.source.changed.action'),
        onClick: () => actions.clearErrorAndPrepare(),
      });
    } else if (code === 'insufficient_journal_space') {
      const isLight = errInfo.mode === 'without_processed_media';
      const driveName = stagingDriveName(estimate);
      tone = 'is-err';
      title = t('journal.window.export.err.no.space.title', [['drive', driveName]]);
      if (isLight) {
        body = t(
          isMac ? 'journal.window.export.err.no.space.macos.light' : 'journal.window.export.err.no.space.other.light',
          [['drive', driveName]]
        );
      } else {
        body = t(isMac ? 'journal.window.export.err.no.space.macos' : 'journal.window.export.err.no.space.other');
        primaryBtn = createButton(doc, {
          focusKey: 'err-switch-light',
          variant: 'is-small',
          label: t('journal.window.export.err.no.space.action'),
          onClick: () => actions.clearErrorAndSelectLight(),
        });
      }
      extraCli = isLight
        ? ((estimate && estimate.cli_command_light) || 'solstone journal archive export --without-processed-media --out /path/to/drive/journal.zip')
        : ((estimate && estimate.cli_command_full) || 'solstone journal archive export --out /path/to/drive/journal.zip');
    } else if (code === 'export_expired') {
      tone = 'is-warn';
      title = t('journal.window.export.err.expired.title');
      body = t('journal.window.export.err.expired.body');
      primaryBtn = createButton(doc, {
        focusKey: 'err-new',
        variant: 'is-primary',
        label: t('journal.window.export.err.expired.action'),
        onClick: () => actions.clearError(),
      });
    } else if (code === 'export_invalidated_by_erasure') {
      tone = 'is-warn';
      title = t('journal.window.export.err.erased.title');
      body = t('journal.window.export.err.erased.body');
      primaryBtn = createButton(doc, {
        focusKey: 'err-new',
        variant: 'is-primary',
        label: t('journal.window.export.err.expired.action'),
        onClick: () => actions.clearError(),
      });
    } else if (code === 'export_copy_unconfirmed') {
      tone = 'is-warn';
      title = t('journal.window.export.err.copy.unconfirmed.title');
      body = t('journal.window.export.err.copy.unconfirmed.body');
      primaryBtn = createButton(doc, {
        id: 'exportRefreshCopyBtn',
        focusKey: 'err-refresh-copy',
        variant: 'is-primary',
        disabled: Boolean(actions.isRefreshingCopy && actions.isRefreshingCopy()),
        label: t('journal.window.export.err.copy.unconfirmed.action'),
        onClick: () => actions.refreshCurrentCopy(),
      });
    } else if (code === 'checksum_mismatch' || code === 'export_verification_failed') {
      tone = 'is-err';
      title = t('journal.window.export.err.checksum.title');
      body = t('journal.window.export.err.checksum.body');
      primaryBtn = createButton(doc, {
        focusKey: 'err-save-again',
        variant: 'is-primary',
        label: t('journal.window.export.err.checksum.action'),
        onClick: () => actions.retryNativeSave(),
      });
    } else if (code === 'revoked' || code === 'export_requester_revoked') {
      tone = 'is-err';
      title = t('journal.window.export.err.revoked.title');
      body = t('journal.window.export.err.revoked.body');
    } else if (code === 'dest_write_failed' || code === 'write_failed' || code === 'destination_write_failed') {
      tone = 'is-err';
      title = t('journal.window.export.err.dest.write.title');
      body = t('journal.window.export.err.dest.write.body');
      primaryBtn = createButton(doc, {
        focusKey: 'err-choose-another',
        variant: 'is-primary',
        label: t('journal.window.export.space.choose.another'),
        onClick: () => actions.retryNativeSave(),
      });
    } else if (code === 'insufficient_device_space' || code === 'insufficient_space') {
      const mobile = isMobilePlatform(errInfo.platform);
      const lightBytes = estimate && estimate.breakdown && estimate.breakdown.light_bytes;
      const isLight = errInfo.mode === 'without_processed_media' || typeof lightBytes !== 'number' || lightBytes <= 0;
      tone = 'is-warn';
      title = t(mobile ? 'journal.window.export.space.device.mobile.title' : 'journal.window.export.space.device.desktop.title');
      if (isLight) {
        body = t(
          mobile ? 'journal.window.export.space.device.mobile.body.light' : 'journal.window.export.space.device.desktop.body.light',
          [
            ['needed', formatBytes(errInfo.required_bytes || 0)],
            ['free', formatBytes(errInfo.available_bytes || 0)],
          ]
        );
      } else {
        body = t(
          mobile ? 'journal.window.export.space.device.mobile.body' : 'journal.window.export.space.device.desktop.body',
          [
            ['needed', formatBytes(errInfo.required_bytes || 0)],
            ['free', formatBytes(errInfo.available_bytes || 0)],
            ['light', formatBytes(lightBytes)],
          ]
        );
      }
      primaryBtn = createButton(doc, {
        focusKey: 'err-choose-another',
        variant: 'is-small',
        label: t('journal.window.export.space.choose.another'),
        onClick: () => actions.retryNativeSave(),
      });
    } else {
      primaryBtn = createButton(doc, {
        focusKey: 'err-retry',
        variant: 'is-primary',
        label: t('journal.window.export.err.source.changed.action'),
        onClick: () => actions.clearError(),
      });
    }

    const band = el(doc, 'div', `export-band ${tone}`);
    band.setAttribute('role', 'alert');
    band.setAttribute('data-export-error', code);
    band.appendChild(el(doc, 'span', 'export-band-title', title));
    band.appendChild(el(doc, 'p', null, body));
    if (extraCli) band.appendChild(createCodeLine(doc, extraCli));
    if (primaryBtn) {
      const row = el(doc, 'div', 'export-band-actions');
      row.appendChild(primaryBtn);
      band.appendChild(row);
    }
    return band;
  }

  function renderNativeTransfer(doc, nativeState, ready, estimate, openMap, actions) {
    const phase = nativeState.phase;
    const platform = nativeState.platform || '';
    const mobile = isMobilePlatform(platform);
    const totalBytes = Number(nativeState.total_bytes || (ready && ready.size_bytes) || 0);
    const doneBytes = Number(nativeState.bytes_written || 0);
    const pct = totalBytes > 0 ? Math.min(100, Math.round((doneBytes / totalBytes) * 100)) : 0;

    if (phase === 'verifying') {
      const head = el(doc, 'header', 'export-head');
      head.appendChild(el(doc, 'h1', null, t('journal.window.export.transfer.verifying')));
      const track = el(doc, 'div', 'export-prog-track is-ok');
      const fill = el(doc, 'div', 'export-prog-fill');
      fill.style.width = '100%';
      track.appendChild(fill);
      return [head, track];
    }

    if (phase === 'handed_off') {
      const nodes = [];
      const isAppleFiles = platform === 'ios' || platform === 'ipados' || platform === 'ios_host';
      const matchingReady =
        ready && nativeState.export_id && ready.export_id === nativeState.export_id ? ready : null;
      const head = el(doc, 'header', 'export-head');
      head.appendChild(
        el(
          doc,
          'h1',
          null,
          t(
            isAppleFiles
              ? 'journal.window.export.transfer.handed.title'
              : 'journal.window.export.transfer.handed.provider.title'
          )
        )
      );
      nodes.push(head);
      const band = el(doc, 'div', 'export-band is-calm');
      band.setAttribute('role', 'status');
      band.setAttribute('data-export-native-state', 'handed_off');
      band.appendChild(
        el(
          doc,
          'p',
          null,
          t(
            isAppleFiles
              ? 'journal.window.export.transfer.handed.body'
              : 'journal.window.export.transfer.handed.provider.body'
          )
        )
      );
      if (nativeState.destination_display) {
        band.appendChild(el(doc, 'p', null, nativeState.destination_display));
      }
      if (matchingReady) {
        const targetId = matchingReady.export_id;
        const targetSize = matchingReady.size_bytes;
        const act = el(doc, 'div', 'export-band-actions');
        act.appendChild(
          createButton(doc, {
            focusKey: 'handed-discard',
            variant: 'is-danger-outline is-small',
            label: t('journal.window.export.ready.discard'),
            onClick: () => actions.confirmDiscardNow(targetId, targetSize),
          })
        );
        band.appendChild(act);
      }
      nodes.push(band);
      nodes.push(createMoveGuide(doc, openMap));
      return nodes;
    }

    if (phase === 'saved') {
      const nodes = [];
      const matchingReady =
        ready && nativeState.export_id && ready.export_id === nativeState.export_id ? ready : null;
      const head = el(doc, 'header', 'export-head');
      head.appendChild(el(doc, 'h1', null, t('journal.window.export.transfer.saved.title')));
      nodes.push(head);
      const ok = el(doc, 'div', 'export-band is-ok');
      ok.setAttribute('role', 'status');
      ok.setAttribute('data-export-native-state', 'saved');
      ok.appendChild(
        el(
          doc,
          'span',
          'export-band-title',
          t('journal.window.export.transfer.saved.body', [['size', formatBytes(totalBytes)]])
        )
      );
      if (nativeState.destination_display) {
        ok.appendChild(el(doc, 'p', null, nativeState.destination_display));
      }
      nodes.push(ok);

      if (nativeState.can_reveal) {
        const revealLabel = platform === 'macos'
          ? t('journal.window.export.transfer.reveal.finder')
          : platform === 'windows'
            ? t('journal.window.export.transfer.reveal.explorer')
            : t('journal.window.export.transfer.reveal.files');
        const row = el(doc, 'div', 'export-actions');
        row.appendChild(
          createButton(doc, {
            focusKey: 'native-reveal',
            variant: 'is-primary',
            label: revealLabel,
            onClick: () => actions.revealNative(),
          })
        );
        nodes.push(row);
      }

      if (matchingReady && !actions.isSavedCleanupDismissed()) {
        const targetId = matchingReady.export_id;
        const targetSize = matchingReady.size_bytes;
        const cleanup = el(doc, 'div', 'export-band is-calm');
        cleanup.setAttribute('data-export-cleanup-prompt', '1');
        cleanup.appendChild(
          el(
            doc,
            'span',
            'export-band-title',
            t('journal.window.export.transfer.cleanup.title', [
              ['size', formatBytes(targetSize)],
              ['drive', stagingDriveName(estimate)],
            ])
          )
        );
        cleanup.appendChild(el(doc, 'p', null, t('journal.window.export.transfer.cleanup.body')));
        const act = el(doc, 'div', 'export-band-actions');
        act.appendChild(
          createButton(doc, {
            focusKey: 'cleanup-discard',
            variant: 'is-small',
            label: t('journal.window.export.transfer.cleanup.discard'),
            onClick: () => actions.confirmDiscardNow(targetId, targetSize),
          })
        );
        act.appendChild(
          createButton(doc, {
            focusKey: 'cleanup-keep',
            variant: 'is-quiet is-small',
            label: t('journal.window.export.transfer.cleanup.keep'),
            onClick: () => actions.dismissSavedCleanup(),
          })
        );
        cleanup.appendChild(act);
        nodes.push(cleanup);
      }

      nodes.push(createMoveGuide(doc, openMap));
      return nodes;
    }

    if (phase === 'paused') {
      const nodes = [];
      const head = el(doc, 'header', 'export-head');
      head.appendChild(el(doc, 'h1', null, t('journal.window.export.transfer.paused.title')));
      nodes.push(head);
      nodes.push(
        el(
          doc,
          'p',
          'export-lede',
          t('journal.window.export.transfer.paused.body', [
            ['done', formatBytes(doneBytes)],
            ['total', formatBytes(totalBytes)],
          ])
        )
      );
      const track = el(doc, 'div', 'export-prog-track is-paused');
      const fill = el(doc, 'div', 'export-prog-fill');
      fill.style.width = `${pct}%`;
      track.appendChild(fill);
      nodes.push(track);

      if (nativeState.pause_reason === 'disconnected') {
        const warn = el(doc, 'div', 'export-band is-warn');
        warn.appendChild(el(doc, 'p', null, t('journal.window.export.transfer.paused.disconnected')));
        nodes.push(warn);
      } else if (nativeState.pause_reason === 'relay_switched') {
        const warn = el(doc, 'div', 'export-band is-warn');
        warn.appendChild(el(doc, 'p', null, t('journal.window.export.transfer.paused.relay')));
        nodes.push(warn);
      }

      if (mobile) {
        nodes.push(el(doc, 'p', 'export-hint', t('journal.window.export.transfer.mobile.keep.open')));
      }

      const row = el(doc, 'div', 'export-actions');
      row.appendChild(
        createButton(doc, {
          focusKey: 'native-resume',
          variant: 'is-primary',
          label: t('journal.window.export.transfer.resume'),
          onClick: () => actions.resumeNative(),
        })
      );
      row.appendChild(
        createButton(doc, {
          focusKey: 'native-cancel',
          variant: 'is-quiet',
          label: t('journal.window.export.transfer.cancel'),
          onClick: () => actions.cancelNative(),
        })
      );
      nodes.push(row);
      return nodes;
    }

    // transferring (or picking)
    const nodes = [];
    const headingId = platform === 'macos'
      ? 'journal.window.export.transfer.saving.mac'
      : mobile
        ? 'journal.window.export.transfer.saving.device'
        : 'journal.window.export.transfer.saving.computer';
    const head = el(doc, 'header', 'export-head');
    head.appendChild(el(doc, 'h1', null, t(headingId)));
    nodes.push(head);

    const rateText = formatRate(nativeState.rate_bytes_per_sec);
    let progressText;
    if (rateText && nativeState.transport === 'local') {
      progressText = t('journal.window.export.transfer.progress.lan', [
        ['done', formatBytes(doneBytes)],
        ['total', formatBytes(totalBytes)],
        ['rate', rateText],
      ]);
    } else if (rateText && nativeState.transport === 'relay') {
      progressText = t('journal.window.export.transfer.progress.relay', [
        ['done', formatBytes(doneBytes)],
        ['total', formatBytes(totalBytes)],
        ['rate', rateText],
      ]);
    } else {
      progressText = t('journal.window.export.transfer.progress.plain', [
        ['done', formatBytes(doneBytes)],
        ['total', formatBytes(totalBytes)],
      ]);
    }
    const lede = el(doc, 'p', 'export-lede', progressText);
    lede.setAttribute('role', 'status');
    lede.setAttribute('aria-live', 'polite');
    nodes.push(lede);

    const track = el(doc, 'div', 'export-prog-track');
    const fill = el(doc, 'div', 'export-prog-fill');
    fill.style.width = `${pct}%`;
    track.appendChild(fill);
    nodes.push(track);

    if (nativeState.destination_display) {
      const destRow = el(doc, 'div', 'export-space-row');
      destRow.appendChild(
        el(
          doc,
          'span',
          null,
          t('journal.window.export.transfer.destination', [['dest', nativeState.destination_display]])
        )
      );
      nodes.push(destRow);
    }

    const hintParts = [t('journal.window.export.transfer.rolling')];
    if (mobile) {
      hintParts.push(t('journal.window.export.transfer.mobile.keep.open'));
    } else {
      hintParts.push(t('journal.window.export.transfer.awake'));
    }
    nodes.push(el(doc, 'p', 'export-hint', hintParts.join(' ')));

    const row = el(doc, 'div', 'export-actions');
    row.appendChild(
      createButton(doc, {
        focusKey: 'native-pause',
        label: t('journal.window.export.transfer.pause'),
        onClick: () => actions.pauseNative(),
      })
    );
    row.appendChild(
      createButton(doc, {
        focusKey: 'native-cancel',
        variant: 'is-quiet',
        label: t('journal.window.export.transfer.cancel'),
        onClick: () => actions.cancelNative(),
      })
    );
    nodes.push(row);
    return nodes;
  }

  function render(doc, root, ui, actions) {
    root.textContent = '';

    const estimate = ui.estimate;
    const state = ui.state;
    const nativeState = ui.nativeState;
    const ready = state && state.ready;

    // 1. Active or terminal native transfer takes precedence only when no newer ready copy exists.
    const nativeMatchesReady =
      !ready ||
      (nativeState && nativeState.export_id
        ? nativeState.export_id === ready.export_id
        : !['saved', 'handed_off'].includes(nativeState && nativeState.phase));
    if (
      nativeState &&
      ['transferring', 'paused', 'verifying', 'saved', 'handed_off'].includes(nativeState.phase) &&
      nativeMatchesReady
    ) {
      for (const node of renderNativeTransfer(doc, nativeState, ready, estimate, ui.openFolds, actions)) {
        root.appendChild(node);
      }
      return 'content';
    }

    // 2. Active preparation on the journal.
    if (state && state.status === 'preparing' && state.active_job) {
      const job = state.active_job;
      const isLight = job.mode === 'without_processed_media';
      const head = el(doc, 'header', 'export-head');
      head.appendChild(
        el(
          doc,
          'h1',
          null,
          t(isLight ? 'journal.window.export.preparing.light' : 'journal.window.export.preparing.full')
        )
      );
      root.appendChild(head);

      let progressLine;
      let pct = 8;
      if (job.phase === 'scanning') {
        progressLine = t('journal.window.export.preparing.scanning');
      } else if (job.phase === 'verifying') {
        progressLine = t('journal.window.export.preparing.verifying');
        pct = 96;
      } else {
        const bytesTotal = Number(job.bytes_total || 0);
        const bytesRead = Number(job.bytes_read || 0);
        pct = bytesTotal > 0 ? Math.max(4, Math.min(95, Math.round((bytesRead / bytesTotal) * 100))) : 15;
        progressLine = t('journal.window.export.preparing.progress', [
          ['done', formatBytes(bytesRead)],
          ['total', formatBytes(bytesTotal)],
          ['files_done', String(job.files_done || 0)],
          ['files_total', formatFiles(job.files_total || 0)],
        ]);
      }

      const lede = el(doc, 'p', 'export-lede', progressLine);
      lede.setAttribute('role', 'status');
      lede.setAttribute('aria-live', 'polite');
      root.appendChild(lede);

      const track = el(doc, 'div', 'export-prog-track');
      const fill = el(doc, 'div', 'export-prog-fill');
      fill.style.width = `${pct}%`;
      track.appendChild(fill);
      root.appendChild(track);

      root.appendChild(el(doc, 'p', 'export-hint', t('journal.window.export.preparing.background')));

      const row = el(doc, 'div', 'export-actions');
      row.appendChild(
        createButton(doc, {
          id: 'exportCancelPrepBtn',
          focusKey: 'cancel-prep',
          variant: 'is-quiet',
          disabled: ui.cancellingPrep,
          label: t(ui.cancellingPrep ? 'journal.window.export.preparing.cancelling' : 'journal.window.export.preparing.cancel'),
          onClick: () => actions.cancelPrepare(job.job_id),
        })
      );
      root.appendChild(row);
      return 'content';
    }

    // 3. Ready copy on the journal.
    if (state && state.status === 'ready' && ready) {
      const isLight = ready.mode === 'without_processed_media';
      const head = el(doc, 'header', 'export-head');
      head.appendChild(el(doc, 'h1', null, t('journal.window.export.ready.title')));
      root.appendChild(head);

      root.appendChild(
        el(
          doc,
          'p',
          'export-lede',
          t('journal.window.export.ready.meta', [
            ['mode', t(isLight ? 'journal.window.export.mode.light.label' : 'journal.window.export.mode.full.label')],
            ['size', formatBytes(ready.size_bytes)],
            ['when', formatPreparedTime(ready.created_at)],
          ])
        )
      );

      // Native error (e.g. unconfirmed prepared copy, checksum mismatch, dest write failed, insufficient device space).
      const nativeErrorMatchesReady = Boolean(
        nativeState &&
          (nativeState.phase === 'error' || nativeState.phase === 'failed') &&
          (!nativeState.export_id || nativeState.export_id === ready.export_id)
      );
      const isUnconfirmedCopyError = Boolean(
        nativeErrorMatchesReady && nativeState.error_code === 'export_copy_unconfirmed'
      );
      if (nativeErrorMatchesReady) {
        root.appendChild(
          renderErrorCard(
            doc,
            {
              code: nativeState.error_code || 'dest_write_failed',
              mode: ready.mode,
              platform: nativeState.platform,
              required_bytes: nativeState.required_bytes || ready.size_bytes,
              available_bytes: nativeState.available_bytes,
            },
            estimate,
            actions
          )
        );
      }

      // Credentials & 24h rolling retention surface + check what's in this copy.
      const trust = el(doc, 'div', 'export-trust');
      trust.appendChild(el(doc, 'p', 'export-trust-body', t('journal.window.export.ready.credentials')));
      trust.appendChild(
        createDetails(
          doc,
          'ready-contents',
          t('journal.window.export.ready.contents.summary'),
          false,
          ui.openFolds,
          (body) => {
            if (isLight) {
              body.appendChild(
                el(
                  doc,
                  'p',
                  null,
                  t('journal.window.export.ready.contents.light', [
                    ['days', formatDays(ready.days_count || 0)],
                    ['files', formatFiles(ready.files_count || 0)],
                    ['omitted_size', formatBytes(ready.omitted_media_bytes || 0)],
                    ['omitted_files', formatFiles(ready.omitted_media_files || 0)],
                  ])
                )
              );
            } else {
              body.appendChild(
                el(
                  doc,
                  'p',
                  null,
                  t('journal.window.export.ready.contents.full', [
                    ['days', formatDays(ready.days_count || 0)],
                    ['files', formatFiles(ready.files_count || 0)],
                  ])
                )
              );
            }
            if (ready.offload_degraded) {
              body.appendChild(el(doc, 'p', null, t('journal.window.export.offload.degraded.body')));
            } else if (ready.offloaded_days > 0) {
              body.appendChild(
                el(
                  doc,
                  'p',
                  null,
                  t('journal.window.export.offload.healthy.body', [['size', formatBytes(ready.offloaded_bytes || 0)]])
                )
              );
            }
            body.appendChild(el(doc, 'p', null, t('journal.window.export.breakdown.always.excluded')));
            const copyBtn = createButton(doc, {
              id: 'exportCopyShaBtn',
              focusKey: 'copy-sha',
              variant: 'is-small',
              label: t(ui.copiedChecksum ? 'journal.window.export.ready.checksum.copied' : 'journal.window.export.ready.checksum.copy'),
              onClick: () => actions.copyChecksum(ready.sha256),
            });
            body.appendChild(createCodeLine(doc, `SHA-256: ${ready.sha256}`, copyBtn));
          }
        )
      );
      root.appendChild(trust);

      if (ready.contents_changed_since) {
        const changed = el(doc, 'div', 'export-band is-calm');
        changed.setAttribute('data-export-changed', '1');
        changed.appendChild(el(doc, 'span', 'export-band-title', t('journal.window.export.ready.changed.title')));
        changed.appendChild(el(doc, 'p', null, t('journal.window.export.ready.changed.body')));
        root.appendChild(changed);
      }

      if (ui.browserStarted) {
        const started = el(doc, 'div', 'export-band is-ok');
        started.setAttribute('role', 'status');
        started.setAttribute('data-export-browser-started', '1');
        started.appendChild(el(doc, 'span', 'export-band-title', t('journal.window.export.browser.started.title')));
        started.appendChild(el(doc, 'p', null, t('journal.window.export.browser.started.body')));
        root.appendChild(started);
      }

      if (ui.discardConflict) {
        const conflict = el(doc, 'div', 'export-band is-warn');
        conflict.setAttribute('role', 'alert');
        conflict.appendChild(el(doc, 'p', null, t('journal.window.export.ready.discard.active')));
        root.appendChild(conflict);
      }

      const hasNative = actions.hasNativeExportHost();
      const row = el(doc, 'div', 'export-actions');
      if (hasNative) {
        row.appendChild(
          createButton(doc, {
            id: 'exportPrimarySaveBtn',
            focusKey: 'save-copy',
            variant: 'is-primary',
            disabled: isUnconfirmedCopyError,
            label: t('journal.window.export.ready.save'),
            onClick: () => {
              if (!isUnconfirmedCopyError) actions.startNativeSave(ready);
            },
          })
        );
      } else {
        const downloadHref = actions.isOlderPairedHost()
          ? (ready.legacy_download_path || `/app/import/api/journal-archive/export?id=${encodeURIComponent(ready.export_id)}`)
          : (ready.download_path || `/app/export/api/ready/${encodeURIComponent(ready.export_id)}/download`);
        const dl = el(doc, 'a', 'export-btn is-primary', t('journal.window.export.ready.download'));
        dl.setAttribute('id', 'exportPrimaryDownloadLink');
        dl.setAttribute('data-export-focus', 'download-copy');
        dl.setAttribute('href', downloadHref);
        dl.setAttribute('download', ready.filename || 'solstone-journal.zip');
        dl.addEventListener('click', () => actions.markBrowserDownloadStarted());
        row.appendChild(dl);
      }
      row.appendChild(
        createButton(doc, {
          id: 'exportDiscardOpenBtn',
          focusKey: 'discard-open',
          variant: 'is-danger-outline',
          label: t('journal.window.export.ready.discard'),
          onClick: () => actions.openDiscardConfirm(),
        })
      );
      root.appendChild(row);

      if (ui.confirmDiscard) {
        const box = el(doc, 'div', 'export-discard-confirm');
        box.setAttribute('id', 'exportDiscardConfirm');
        box.appendChild(
          el(
            doc,
            'p',
            null,
            t('journal.window.export.ready.discard.prompt', [
              ['size', formatBytes(ready.size_bytes)],
              ['drive', stagingDriveName(estimate)],
            ])
          )
        );
        const cRow = el(doc, 'div', 'export-actions');
        cRow.style.marginTop = '8px';
        const targetId = ready.export_id;
        const targetSize = ready.size_bytes;
        cRow.appendChild(
          createButton(doc, {
            id: 'exportDiscardConfirmBtn',
            focusKey: 'discard-confirm',
            variant: 'is-danger is-small',
            label: t('journal.window.export.ready.discard'),
            onClick: () => actions.confirmDiscardNow(targetId, targetSize),
          })
        );
        cRow.appendChild(
          createButton(doc, {
            id: 'exportDiscardKeepBtn',
            focusKey: 'discard-keep',
            variant: 'is-small',
            label: t('journal.window.export.ready.discard.keep'),
            onClick: () => actions.closeDiscardConfirm(),
          })
        );
        box.appendChild(cRow);
        root.appendChild(box);
      }

      root.appendChild(createMoveGuide(doc, ui.openFolds));
      return 'content';
    }

    // 4. Choose & size view (idle / cancelled / failed / discarded).
    const head = el(doc, 'header', 'export-head');
    head.appendChild(el(doc, 'h1', null, t('journal.window.export.title')));
    root.appendChild(head);
    root.appendChild(el(doc, 'p', 'export-lede', t('journal.window.export.subtitle')));

    root.appendChild(
      createDetails(
        doc,
        'disambig',
        t('journal.window.export.disambig.summary'),
        false,
        ui.openFolds,
        (body) => {
          body.appendChild(el(doc, 'p', null, t('journal.window.export.disambig.export')));
          body.appendChild(el(doc, 'p', null, t('journal.window.export.disambig.backup')));
          body.appendChild(el(doc, 'p', null, t('journal.window.export.disambig.merge')));
        }
      )
    );

    if (ui.discardedBytes != null) {
      const ok = el(doc, 'div', 'export-band is-ok');
      ok.setAttribute('role', 'status');
      ok.setAttribute('data-export-discarded', '1');
      ok.appendChild(el(doc, 'span', 'export-band-title', t('journal.window.export.ready.discarded.title')));
      ok.appendChild(
        el(
          doc,
          'p',
          null,
          t('journal.window.export.ready.discarded.body', [
            ['size', formatBytes(ui.discardedBytes)],
            ['drive', stagingDriveName(estimate)],
          ])
        )
      );
      root.appendChild(ok);
    }

    if (state && state.status === 'cancelled' && !ui.dismissedTerminal) {
      const cancelled = el(doc, 'div', 'export-band is-calm');
      cancelled.setAttribute('role', 'status');
      cancelled.appendChild(el(doc, 'span', 'export-band-title', t('journal.window.export.cancelled.title')));
      cancelled.appendChild(
        el(doc, 'p', null, t('journal.window.export.cancelled.body', [['drive', stagingDriveName(estimate)]]))
      );
      root.appendChild(cancelled);
    } else if (
      state &&
      (state.status === 'failed' ||
        state.status === 'expired' ||
        state.status === 'erased_invalidated' ||
        state.status === 'unavailable') &&
      !ui.dismissedTerminal
    ) {
      const code = (state.active_job && state.active_job.error_code) ||
        state.error_code ||
        (state.status === 'expired'
          ? 'export_expired'
          : state.status === 'erased_invalidated'
            ? 'export_invalidated_by_erasure'
            : state.status === 'unavailable'
              ? 'export_staging_unavailable'
              : 'prepare_failed');
      const errMode = (state.active_job && state.active_job.mode) || ui.mode;
      root.appendChild(renderErrorCard(doc, { code, mode: errMode }, estimate, actions));
    } else if (ui.startError) {
      root.appendChild(renderErrorCard(doc, Object.assign({ mode: ui.mode }, ui.startError), estimate, actions));
    }

    if (state && state.other_requester_busy) {
      const busy = el(doc, 'div', 'export-band is-calm');
      busy.setAttribute('role', 'status');
      busy.appendChild(el(doc, 'p', null, t('journal.window.export.busy.other')));
      root.appendChild(busy);
    }

    // Measuring / freshness bar.
    const isMeasuring = !estimate || estimate.status === 'measuring';
    const isMeasureError = estimate && estimate.status === 'error';
    const breakdown = estimate && estimate.breakdown;
    if (isMeasureError) {
      const warn = el(doc, 'div', 'export-band is-warn');
      warn.setAttribute('role', 'alert');
      warn.appendChild(el(doc, 'p', null, t('journal.window.export.measure.failed')));
      const row = el(doc, 'div', 'export-band-actions');
      row.appendChild(
        createButton(doc, {
          focusKey: 'measure-retry',
          variant: 'is-small',
          label: t('journal.window.export.try.again'),
          onClick: () => actions.refreshEstimate(),
        })
      );
      warn.appendChild(row);
      root.appendChild(warn);
    } else if (isMeasuring) {
      const progress = estimate && estimate.progress;
      const fresh = el(doc, 'div', 'export-fresh-row');
      fresh.setAttribute('role', 'status');
      fresh.setAttribute('aria-live', 'polite');
      const partialText = progress && progress.full_bytes_so_far > 0
        ? t('journal.window.export.measuring.partial', [
            ['size', formatBytes(progress.full_bytes_so_far)],
            ['days', formatDays(estimate.days_counted || 0)],
          ])
        : t('journal.window.export.measuring.initial');
      fresh.appendChild(el(doc, 'span', null, partialText));
      root.appendChild(fresh);

      const pct = progress && progress.segments_total > 0
        ? Math.max(8, Math.min(95, Math.round((progress.segments_scanned / progress.segments_total) * 100)))
        : 20;
      const track = el(doc, 'div', 'export-prog-track');
      const fill = el(doc, 'div', 'export-prog-fill');
      fill.style.width = `${pct}%`;
      track.appendChild(fill);
      root.appendChild(track);
    } else if (breakdown) {
      const fresh = el(doc, 'div', 'export-fresh-row');
      const mins = minutesSince(estimate.measured_at, actions.nowMs());
      const daysLabel = formatDays(estimate.days_counted || 0);
      const filesLabel = formatFiles(breakdown.full_files || 0);
      const freshText = mins < 1
        ? t('journal.window.export.sized.just.now', [['days', daysLabel], ['files', filesLabel]])
        : t('journal.window.export.sized.ago', [['minutes', String(mins)], ['days', daysLabel], ['files', filesLabel]]);
      fresh.appendChild(el(doc, 'span', null, freshText));
      const recheck = el(doc, 'button', 'export-link-btn', t('journal.window.export.check.again'));
      recheck.setAttribute('type', 'button');
      recheck.setAttribute('id', 'exportRecheckBtn');
      recheck.setAttribute('data-export-focus', 'recheck');
      recheck.addEventListener('click', () => actions.refreshEstimate());
      fresh.appendChild(recheck);
      root.appendChild(fresh);
    }

    // Two explicit copy modes (full copy selected by default).
    const fullFits = !estimate || estimate.full_fits !== false;
    const lightFits = !estimate || estimate.light_fits !== false;
    const radioList = el(doc, 'div', 'export-radio-list');
    radioList.setAttribute('role', 'radiogroup');
    radioList.setAttribute('aria-label', t('journal.window.export.mode.group.label'));

    function addModeOption(value, titleId, summaryId, restoreId, sizeText, subTag, fits) {
      const selected = ui.mode === value;
      const classes = ['export-choice', selected ? 'is-selected' : '', !isMeasuring && !fits ? 'is-tight' : '']
        .filter(Boolean)
        .join(' ');
      const label = el(doc, 'label', classes);
      label.setAttribute('data-export-mode', value);
      const left = el(doc, 'span', 'export-choice-left');
      const input = doc.createElement('input');
      input.setAttribute('type', 'radio');
      input.setAttribute('name', 'export-mode');
      input.setAttribute('value', value);
      input.setAttribute('data-export-focus', `mode:${value}`);
      if (selected) input.setAttribute('checked', '');
      input.checked = selected;
      input.addEventListener('change', () => actions.selectMode(value));
      left.appendChild(input);

      const copyWrap = el(doc, 'span');
      copyWrap.appendChild(el(doc, 'span', 'export-choice-title', t(titleId)));
      copyWrap.appendChild(el(doc, 'span', 'export-choice-desc', t(summaryId)));
      copyWrap.appendChild(el(doc, 'span', 'export-choice-sub', t(restoreId)));
      left.appendChild(copyWrap);
      label.appendChild(left);

      const right = el(doc, 'span', 'export-choice-right');
      right.appendChild(el(doc, 'span', null, sizeText));
      if (subTag) right.appendChild(el(doc, 'span', 'export-choice-tag', subTag));
      label.appendChild(right);
      radioList.appendChild(label);
    }

    const progress = estimate && estimate.progress;
    const fullSizeText = isMeasuring
      ? (progress && progress.full_bytes_so_far > 0
          ? t('journal.window.export.mode.measuring.plus', [['size', formatBytes(progress.full_bytes_so_far)]])
          : t('journal.window.export.mode.measuring'))
      : (breakdown ? formatBytes(breakdown.full_bytes) : '—');
    const lightSizeText = isMeasuring
      ? t('journal.window.export.mode.measuring')
      : (breakdown ? formatBytes(breakdown.light_bytes) : '—');

    const fullTag = !isMeasuring && !fullFits ? t('journal.window.export.mode.needs.space') : '';
    const lightTag = !isMeasuring && !lightFits
      ? t('journal.window.export.mode.needs.space')
      : (!isMeasuring && breakdown && breakdown.processed_media_omitted && breakdown.processed_media_omitted.bytes > 0
          ? t('journal.window.export.mode.savings', [['size', formatBytes(breakdown.processed_media_omitted.bytes)]])
          : '');

    addModeOption(
      'full',
      'journal.window.export.mode.full.label',
      'journal.window.export.mode.full.summary',
      'journal.window.export.mode.full.restore',
      fullSizeText,
      fullTag,
      fullFits
    );
    addModeOption(
      'without_processed_media',
      'journal.window.export.mode.light.label',
      'journal.window.export.mode.light.summary',
      'journal.window.export.mode.light.restore',
      lightSizeText,
      lightTag,
      lightFits
    );
    root.appendChild(radioList);

    const offloadNotice = renderOffloadNotice(doc, estimate);
    if (offloadNotice) root.appendChild(offloadNotice);

    root.appendChild(renderBreakdown(doc, estimate, ui.mode, ui.openFolds));

    const spaceNotice = renderSpacePreflight(doc, estimate, ui.mode, ui.openFolds, actions);
    if (spaceNotice) root.appendChild(spaceNotice);

    // Paired mobile device space preflight (when device_space reported tight free space).
    if (
      ui.deviceSpace &&
      typeof ui.deviceSpace.free_bytes === 'number' &&
      breakdown &&
      isMobilePlatform(ui.deviceSpace.platform)
    ) {
      const selectedBytes = ui.mode === 'full' ? breakdown.full_bytes : breakdown.light_bytes;
      if (selectedBytes > 0 && ui.deviceSpace.free_bytes < selectedBytes) {
        const mobWarn = el(doc, 'div', 'export-band is-warn');
        mobWarn.setAttribute('data-export-device-space', 'tight');
        mobWarn.appendChild(el(doc, 'span', 'export-band-title', t('journal.window.export.space.device.mobile.title')));
        const mobIsLight = ui.mode === 'without_processed_media';
        mobWarn.appendChild(
          el(
            doc,
            'p',
            null,
            mobIsLight
              ? t('journal.window.export.space.device.mobile.body.light', [
                  ['needed', formatBytes(selectedBytes)],
                  ['free', formatBytes(ui.deviceSpace.free_bytes)],
                ])
              : t('journal.window.export.space.device.mobile.body', [
                  ['needed', formatBytes(selectedBytes)],
                  ['free', formatBytes(ui.deviceSpace.free_bytes)],
                  ['light', formatBytes(breakdown.light_bytes)],
                ])
          )
        );
        if (ui.mode === 'full' && breakdown.light_bytes < selectedBytes) {
          const mobAct = el(doc, 'div', 'export-band-actions');
          mobAct.appendChild(
            createButton(doc, {
              focusKey: 'mob-switch-light',
              variant: 'is-small',
              label: t('journal.window.export.err.no.space.action'),
              onClick: () => actions.selectMode('without_processed_media'),
            })
          );
          mobWarn.appendChild(mobAct);
        }
        root.appendChild(mobWarn);
      }
    }

    const chosenFits = ui.mode === 'full' ? fullFits : lightFits;
    const stagingUnavailable = (state && state.status === 'unavailable') || (estimate && estimate.staging_available === false);
    const canPrepare = !isMeasuring && !isMeasureError && !stagingUnavailable && chosenFits && !(state && state.other_requester_busy) && !ui.startingJob;
    const actRow = el(doc, 'div', 'export-actions');
    actRow.appendChild(
      createButton(doc, {
        id: 'exportPrepareBtn',
        focusKey: 'prepare',
        variant: 'is-primary',
        disabled: !canPrepare,
        label: t(
          ui.mode === 'full'
            ? 'journal.window.export.action.prepare.full'
            : 'journal.window.export.action.prepare.light'
        ),
        onClick: () => actions.startPrepare(),
      })
    );
    root.appendChild(actRow);

    if (!actions.hasNativeExportHost()) {
      const selectedBytes = breakdown ? (ui.mode === 'full' ? breakdown.full_bytes : breakdown.light_bytes) : null;
      root.appendChild(
        el(
          doc,
          'p',
          'export-hint',
          selectedBytes != null
            ? t('journal.window.export.browser.hint', [['size', formatBytes(selectedBytes)]])
            : t('journal.window.export.browser.hint.measuring')
        )
      );
    }

    return 'content';
  }

  function mount(ctx) {
    const doc = global.document;
    const root = doc.getElementById('export-root');
    if (!root) throw new Error('export has no root');

    const ui = {
      mode: 'full',
      estimate: null,
      state: null,
      nativeState: null,
      deviceSpace: null,
      openFolds: {},
      startingJob: false,
      cancellingPrep: false,
      refreshingCopy: false,
      browserStarted: false,
      confirmDiscard: false,
      discardConflict: false,
      discardedBytes: null,
      copiedChecksum: false,
      dismissedTerminal: false,
      dismissedSavedCleanup: false,
      startError: null,
    };

    let reported = false;
    let pollTimer = null;
    let reqCounter = 0;
    let stateReqSeq = 0;
    let latestAppliedStateSeq = 0;
    let minAcceptedStateSeq = 0;
    let estimateReqSeq = 0;
    let latestAppliedEstimateSeq = 0;
    let minAcceptedEstimateSeq = 0;
    const fetchFn = ctx.fetch || global.fetch.bind(global);
    const nowFn = ctx.now || (() => Date.now());

    function makeRequestId() {
      if (global.crypto && typeof global.crypto.randomUUID === 'function') {
        return global.crypto.randomUUID();
      }
      reqCounter += 1;
      return `exp-${nowFn()}-${reqCounter}`;
    }

    function sendNative(type, payload) {
      const pair = getNativeExportHost();
      if (!pair) return null;
      const msg = Object.assign({ type, id: makeRequestId() }, payload || {});
      try {
        const res = pair.host.send(msg);
        if (res && typeof res.then === 'function') {
          res.then((reply) => handleNativeReply(type, reply)).catch(() => {});
        } else if (replyIsObject(res)) {
          handleNativeReply(type, res);
        }
        return res;
      } catch (_err) {
        return null;
      }
    }

    function replyIsObject(val) {
      return Boolean(val && typeof val === 'object');
    }

    function handleNativeReply(type, reply) {
      if (ctx.disposed || !replyIsObject(reply)) return;
      const contract = getExportContract();
      if (contract && reply.type === contract.unsupported) return;
      if (
        contract &&
        type === contract.deviceSpace &&
        (typeof reply.free_bytes === 'number' || typeof reply.available_bytes === 'number')
      ) {
        const freeBytes = typeof reply.free_bytes === 'number' ? reply.free_bytes : reply.available_bytes;
        ui.deviceSpace = Object.assign({}, reply, { free_bytes: freeBytes });
        draw();
        return;
      }
      if (reply.phase) {
        applyNativeStatus(reply);
      }
    }

    function applyNativeStatus(detail) {
      if (ctx.disposed || !detail || typeof detail !== 'object') return;
      if (detail.phase === 'idle' || detail.phase === 'cancelled') {
        ui.nativeState = null;
      } else {
        ui.nativeState = Object.assign({}, detail);
      }
      draw();
    }

    function clearPollTimer() {
      if (pollTimer) {
        global.clearTimeout(pollTimer);
        pollTimer = null;
      }
    }

    function schedulePoll() {
      clearPollTimer();
      if (ctx.disposed || ui.refreshingCopy) return;
      const needsFast =
        (ui.estimate && ui.estimate.status === 'measuring') ||
        (ui.state && ui.state.status === 'preparing');
      const needsSlow = ui.state && ui.state.status === 'ready';
      if (!needsFast && !needsSlow) return;
      const delay = needsFast ? POLL_FAST_MS : POLL_READY_MS;
      pollTimer = global.setTimeout(() => {
        pollTimer = null;
        if (ctx.disposed || ui.refreshingCopy) return;
        const tasks = [];
        if (ui.estimate && ui.estimate.status === 'measuring') tasks.push(loadEstimate());
        if (ui.state && (ui.state.status === 'preparing' || ui.state.status === 'ready')) {
          tasks.push(loadState());
        }
        Promise.all(tasks).then((applied) => {
          if (!ctx.disposed) {
            if (applied.some(Boolean)) draw();
            if (!pollTimer) schedulePoll();
          }
        });
      }, delay);
    }

    function loadEstimate(opts) {
      const seq = ++estimateReqSeq;
      if (opts && opts.authoritative) {
        minAcceptedEstimateSeq = seq;
      }
      return fetchFn(API_ESTIMATE, { headers: { Accept: 'application/json' } })
        .then((res) => (res && res.ok ? res.json() : null))
        .then((data) => {
          if (!data || ctx.disposed) return false;
          if (seq < latestAppliedEstimateSeq || seq < minAcceptedEstimateSeq) return false;
          latestAppliedEstimateSeq = seq;
          ui.estimate = data;
          return true;
        })
        .catch(() => false);
    }

    function loadState(opts) {
      const seq = ++stateReqSeq;
      if (opts && opts.authoritative) {
        minAcceptedStateSeq = seq;
      }
      return fetchFn(API_STATE, { headers: { Accept: 'application/json' } })
        .then((res) => (res && res.ok ? res.json() : null))
        .then((data) => {
          if (!data || ctx.disposed) return false;
          if (seq < latestAppliedStateSeq || seq < minAcceptedStateSeq) return false;
          latestAppliedStateSeq = seq;
          const prevStatus = ui.state && ui.state.status;
          const prevReadyId = ui.state && ui.state.ready && ui.state.ready.export_id;
          const nextReadyId = data.ready && data.ready.export_id;
          ui.state = data;
          if (data.status !== 'preparing') ui.cancellingPrep = false;
          if (prevStatus !== data.status || prevReadyId !== nextReadyId) {
            ui.dismissedTerminal = false;
            ui.confirmDiscard = false;
            ui.discardConflict = false;
            ui.dismissedSavedCleanup = false;
            if (
              ui.nativeState &&
              (ui.nativeState.phase === 'error' || ui.nativeState.phase === 'failed') &&
              (!ui.nativeState.export_id || ui.nativeState.export_id === prevReadyId)
            ) {
              ui.nativeState = null;
            }
            if (data.status !== 'ready' || prevReadyId !== nextReadyId) {
              ui.browserStarted = false;
            }
          }
          return true;
        })
        .catch(() => false);
    }

    const actions = {
      nowMs: () => nowFn(),
      hasNativeExportHost: () => getNativeExportHost() !== null,
      isOlderPairedHost: () => isOlderPairedHost(),
      isSavedCleanupDismissed: () => ui.dismissedSavedCleanup,
      isRefreshingCopy: () => ui.refreshingCopy,
      selectMode(mode) {
        ui.mode = mode;
        ui.startError = null;
        draw();
      },
      refreshEstimate() {
        const seq = ++estimateReqSeq;
        minAcceptedEstimateSeq = seq;
        ui.estimate = Object.assign({}, ui.estimate || {}, { status: 'measuring' });
        draw();
        fetchFn(API_ESTIMATE_REFRESH, { method: 'POST', headers: { Accept: 'application/json' } })
          .then((res) => (res && res.ok ? res.json() : null))
          .then((data) => {
            if (data && !ctx.disposed && seq >= latestAppliedEstimateSeq && seq >= minAcceptedEstimateSeq) {
              latestAppliedEstimateSeq = seq;
              ui.estimate = data;
            }
            if (!ctx.disposed) {
              draw();
              schedulePoll();
            }
          })
          .catch(() => {});
      },
      startPrepare() {
        ui.startingJob = true;
        ui.startError = null;
        ui.discardedBytes = null;
        ui.dismissedTerminal = true;
        draw();
        fetchFn(API_JOBS, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
          body: JSON.stringify({ mode: ui.mode, replace: false }),
        })
          .then((res) => res.json().then((body) => ({ ok: res.ok, body })))
          .then(({ ok, body }) => {
            if (ctx.disposed) return;
            ui.startingJob = false;
            if (!ok) {
              ui.startError = { code: (body && body.code) || 'prepare_failed' };
              draw();
              return;
            }
            return loadState({ authoritative: true }).then(() => {
              if (!ctx.disposed) {
                draw();
                schedulePoll();
              }
            });
          })
          .catch(() => {
            if (ctx.disposed) return;
            ui.startingJob = false;
            ui.startError = { code: 'prepare_failed' };
            draw();
          });
      },
      cancelPrepare(jobId) {
        ui.cancellingPrep = true;
        draw();
        fetchFn(`${API_JOBS}/${encodeURIComponent(jobId)}/cancel`, { method: 'POST' })
          .then(() => loadState({ authoritative: true }))
          .then(() => {
            if (!ctx.disposed) {
              draw();
              schedulePoll();
            }
          })
          .catch(() => {});
      },
      openDiscardConfirm() {
        ui.confirmDiscard = true;
        ui.discardConflict = false;
        draw();
      },
      closeDiscardConfirm() {
        ui.confirmDiscard = false;
        draw();
      },
      confirmDiscardNow(expectedExportId, expectedSizeBytes) {
        const ready = ui.state && ui.state.ready;
        if (!ready) return;
        const targetId = expectedExportId || ready.export_id;
        if (ready.export_id !== targetId) return;
        const sizeBytes = typeof expectedSizeBytes === 'number' ? expectedSizeBytes : ready.size_bytes;
        fetchFn(`/app/export/api/ready/${encodeURIComponent(targetId)}`, { method: 'DELETE' })
          .then((res) => {
            if (ctx.disposed) return;
            if (res.status === 409) {
              ui.discardConflict = true;
              ui.confirmDiscard = false;
              draw();
              return;
            }
            if (!res.ok) {
              ui.confirmDiscard = false;
              return loadState({ authoritative: true }).then(() => {
                if (!ctx.disposed) draw();
              });
            }
            ui.confirmDiscard = false;
            ui.discardConflict = false;
            ui.discardedBytes = sizeBytes;
            ui.nativeState = null;
            return Promise.all([loadState({ authoritative: true }), loadEstimate({ authoritative: true })]).then(() => {
              if (!ctx.disposed) {
                draw();
                schedulePoll();
              }
            });
          })
          .catch(() => {});
      },
      dismissSavedCleanup() {
        ui.dismissedSavedCleanup = true;
        draw();
      },
      markBrowserDownloadStarted() {
        ui.browserStarted = true;
        draw();
      },
      copyChecksum(sha) {
        if (global.navigator && global.navigator.clipboard && typeof global.navigator.clipboard.writeText === 'function') {
          global.navigator.clipboard.writeText(sha).catch(() => {});
        }
        ui.copiedChecksum = true;
        draw();
      },
      clearError() {
        ui.startError = null;
        ui.dismissedTerminal = true;
        draw();
      },
      clearErrorAndPrepare() {
        ui.startError = null;
        ui.dismissedTerminal = true;
        actions.startPrepare();
      },
      clearErrorAndSelectLight() {
        ui.startError = null;
        ui.dismissedTerminal = true;
        ui.mode = 'without_processed_media';
        draw();
      },
      startNativeSave(ready) {
        const contract = getExportContract();
        if (!contract) return;
        ui.dismissedSavedCleanup = false;
        sendNative(contract.start, {
          export_id: ready.export_id,
          download_path: ready.download_path,
          filename: ready.filename,
          size_bytes: ready.size_bytes,
          sha256: ready.sha256,
          etag: ready.etag,
          mode: ready.mode,
        });
      },
      retryNativeSave() {
        const ready = ui.state && ui.state.ready;
        ui.nativeState = null;
        if (ready) {
          actions.startNativeSave(ready);
        } else {
          draw();
        }
      },
      refreshCurrentCopy() {
        if (ui.refreshingCopy) return;
        clearPollTimer();
        ui.refreshingCopy = true;
        draw();
        Promise.all([loadState({ authoritative: true }), loadEstimate({ authoritative: true })])
          .then(([stateOk]) => {
            if (ctx.disposed) return;
            ui.refreshingCopy = false;
            if (stateOk) {
              ui.nativeState = null;
              ui.startError = null;
              ui.dismissedTerminal = false;
            }
            draw();
            schedulePoll();
          })
          .catch(() => {
            if (ctx.disposed) return;
            ui.refreshingCopy = false;
            draw();
            schedulePoll();
          });
      },
      pauseNative() {
        const contract = getExportContract();
        if (!contract || !contract.pause) return;
        sendNative(contract.pause, { export_id: (ui.nativeState && ui.nativeState.export_id) || (ui.state && ui.state.ready && ui.state.ready.export_id) });
      },
      resumeNative() {
        const contract = getExportContract();
        if (!contract || !contract.resume) return;
        sendNative(contract.resume, { export_id: (ui.nativeState && ui.nativeState.export_id) || (ui.state && ui.state.ready && ui.state.ready.export_id) });
      },
      cancelNative() {
        const contract = getExportContract();
        if (!contract || !contract.cancel) return;
        sendNative(contract.cancel, { export_id: (ui.nativeState && ui.nativeState.export_id) || (ui.state && ui.state.ready && ui.state.ready.export_id) });
      },
      revealNative() {
        const contract = getExportContract();
        if (!contract || !contract.reveal) return;
        sendNative(contract.reveal, { export_id: (ui.nativeState && ui.nativeState.export_id) || (ui.state && ui.state.ready && ui.state.ready.export_id) });
      },
    };

    function draw() {
      const active = doc.activeElement;
      const focusKey = active && typeof active.getAttribute === 'function'
        ? active.getAttribute('data-export-focus')
        : null;
      const readiness = render(doc, root, ui, actions);
      if (focusKey) {
        const again = Array.from(root.querySelectorAll('[data-export-focus]'))
          .find((node) => node.getAttribute('data-export-focus') === focusKey);
        if (again && typeof again.focus === 'function') again.focus();
      }
      if (!reported) {
        reported = true;
        ctx.ready(readiness);
      }
    }

    const contract = getExportContract();
    if (contract) {
      ctx.on(global, contract.event, (event) => {
        if (event && event.detail) applyNativeStatus(event.detail);
      });
      if (getNativeExportHost()) {
        sendNative(contract.status, {});
        if (contract.deviceSpace) {
          const pair = getNativeExportHost();
          if (pair && pair.host.types.includes(contract.deviceSpace)) {
            sendNative(contract.deviceSpace, {});
          }
        }
      }
    }

    ctx.onDispose(() => {
      if (pollTimer) {
        global.clearTimeout(pollTimer);
        pollTimer = null;
      }
    });

    Promise.all([loadEstimate(), loadState()]).then(() => {
      if (ctx.disposed) return;
      draw();
      schedulePoll();
    });
  }

  const api = {
    formatBytes,
    getNativeExportHost,
    isOlderPairedHost,
    render,
    mount,
  };

  if (typeof module !== 'undefined' && module.exports) module.exports = api;
  if (global.SolViews && typeof global.SolViews.register === 'function') {
    global.SolViews.register('export', { mount });
  }
})(typeof window !== 'undefined' ? window : globalThis);
