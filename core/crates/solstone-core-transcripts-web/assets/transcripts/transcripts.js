// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Transcripts on the view contract: everything below runs when the view is
// mounted, and what it adds to the page goes when it is disposed. A day step
// is taken in place (`update`), so the view, its journal-wide reads and the
// date picker's calendar stay.
(() => {
  window.SolViews.register('transcripts', { mount: mountTranscripts });

  // A delete's receipt can stay up after this view is gone, or after the day
  // has changed. The button looks the view up when it is clicked. It does not
  // keep the view. While that day is showing, the view puts the segment back.
  // Otherwise the same recover runs and only the receipt is shown.
  let transcriptHooks = null;

  function transcriptsShowing(day) {
    const current = window.SolViews && window.SolViews.current && window.SolViews.current();
    if (!current || current.name !== 'transcripts' || !current.ctx || current.ctx.disposed) return false;
    if (!transcriptHooks || transcriptHooks.shown() !== day) return false;
    const routeDay = current.ctx.route && current.ctx.route.day;
    return routeDay === day;
  }

  function goToSettingsTrash() {
    window.location.href = '/app/settings#trash';
  }

  function transcriptCount(name, amount, display = String(amount)) {
    return [[name + '_quantity', window.JournalI18n.cardinalQuantity(amount)], [name, display]];
  }

  // Owner copy for deleting a segment: the dialog, its erase step and the receipt.
  function transcriptTrashCopy() {
    return {
      select: window.JournalI18n.render('journal.window.transcripts.trash.copy.select.select'),
      selectAll: window.JournalI18n.render('journal.window.transcripts.trash.copy.selectall.select.all'),
      selected: ({ n }) => window.JournalI18n.render('journal.window.transcripts.trash.selection.count', transcriptCount('segments', n)),
      delete: window.JournalI18n.render('journal.window.transcripts.trash.copy.delete.delete'),
      done: window.JournalI18n.render('journal.window.transcripts.trash.copy.done.done'),
      pickRange: ({ start, end }) => window.JournalI18n.render('journal.window.transcripts.trash.selection.range', [['start_time', String(start ?? '')], ['end_time', String(end ?? '')]]),
      set: {
        moveTitle: ({ n }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.selection.move.title', transcriptCount('segments', n)),
        eraseTitle: ({ n }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.selection.erase.title', transcriptCount('segments', n)),
        more: ({ n }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.selection.more', transcriptCount('segments', n)),
        moveReach: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.movereach.they.leave.your.journal.now.and'),
        eraseReach: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.erasereach.they.leave.your.journal.now.and'),
        moveLead: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.movelead.this.moves.each.segment.s.own'),
        eraseLead: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.eraselead.this.erases.each.segment.s.own'),
        moveStays: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.movestays.activities.made.from.them.drop.them'),
        moveFound: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.movefound.detected.entities.drop.them.too.and'),
        eraseStays: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.erasestays.activities.made.from.them.drop.them'),
        eraseFound: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.erasefound.detected.entities.drop.them.too.and'),
        size: ({ size }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.media.size', [['media_size', String(size ?? '')]]),
      },
      notes: {
        busy: window.JournalI18n.render('journal.window.transcripts.trash.copy.busy.this.is.already.being.moved.or'),
        importing: window.JournalI18n.render('journal.window.transcripts.trash.copy.importing.this.wasn.t.moved.because.it'),
        other: window.JournalI18n.render('journal.window.transcripts.trash.copy.other.this.wasn.t.moved.try.again'),
        eraseImporting: window.JournalI18n.render('journal.window.transcripts.trash.copy.eraseimporting.this.wasn.t.erased.because.it'),
        eraseOther: window.JournalI18n.render('journal.window.transcripts.trash.copy.eraseother.this.wasn.t.erased.try.again'),
      },
      setReceipt: {
        moved: { title: ({ n }) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.segments.moved', transcriptCount('segments', n)), message: ({ date }) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.segments.erase.date', [['erase_date', String(date ?? '')]]) },
        partMoved: { title: ({ done, n }) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.partial.move', [...transcriptCount('processed', done), ...transcriptCount('segments', n)]) },
        still: (count) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.marked.remaining', transcriptCount('remaining', count)),
        noneMoved: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.couldn.t.move.those.to.the') },
        noneStill: (count) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.keep.remaining', transcriptCount('remaining', count)),
        erased: { title: ({ n }) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.segments.erased', transcriptCount('segments', n)) },
        partErased: { title: ({ done, n }) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.partial.erase', [...transcriptCount('processed', done), ...transcriptCount('segments', n)]) },
        partErasedMessage: (count) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.erase.partial.help', [['remaining_quantity', window.JournalI18n.cardinalQuantity(count)]]),
        noneErased: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.couldn.t.erase.those') },
        noneErasedMessage: (count) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.erase.none.help', [['remaining_quantity', window.JournalI18n.cardinalQuantity(count)]]),
        recovered: { title: ({ n }) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.segments.recovered', transcriptCount('segments', n)), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.they.re.back.in.your.journal') },
        partRecovered: { title: ({ done, n }) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.partial.recovery', [...transcriptCount('processed', done), ...transcriptCount('segments', n)]) },
        partRecoveredMessage: (count) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.recovery.partial.help', [['remaining_quantity', window.JournalI18n.cardinalQuantity(count)]]),
        recoverFailed: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.couldn.t.recover.those'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.they.re.still.in.the.trash') },
        noAnswerMove: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.your.journal.didn.t.answer'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.they.may.already.be.in.the') },
        noAnswerErase: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.your.journal.didn.t.answer'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.they.may.already.be.erased.reload') },
        noAnswerRecover: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.your.journal.didn.t.answer'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.they.may.already.be.back.in') },
      },
      move: {
        title: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.title.delete.segment'),
        reach: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.reach.it.leaves.your.journal.now.and.variant2'),
        lead: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.lead.this.moves.the.segment.s.own'),
        stays: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.stays.activities.made.from.it.drop.it'),
        found: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.found.detected.entities.drop.it.too.and'),
        size: ({ size }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.media.size', [['media_size', String(size ?? '')]]),
      },
      // A segment from an import: its last takes the import and the file it
      // kept; any other leaves them. Keyed on `kept`, the files the import's
      // own record holds.
      imports: {
        importGoes: {
          label: ({ label }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.import.move.named', [['import_name', String(label ?? '')]]),
          one: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.one.it.s.the.last.segment.from.variant2'),
          many: ({ n }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.import.move.files.count', transcriptCount('retained_files', n)),
          none: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.none.it.s.the.last.segment.from.variant2'),
          unknown: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.unknown.it.s.the.last.segment.from.variant2'),
        },
        importErased: {
          label: ({ label }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.import.erase.named', [['import_name', String(label ?? '')]]),
          one: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.one.it.s.the.last.segment.from'),
          many: ({ n }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.import.erase.files.count', transcriptCount('retained_files', n)),
          none: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.none.it.s.the.last.segment.from'),
          unknown: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.unknown.it.s.the.last.segment.from'),
        },
        waitingErased: {
          one: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.one.the.import.s.other.segment.in'),
          many: ({ w }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.import.pending.count', transcriptCount('waiting_segments', w)),
        },
        importStays: {
          label: ({ label }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.import.stay.named', [['import_name', String(label ?? '')]]),
          one: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.one.the.file.it.came.from.stays'),
          many: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.many.the.files.its.import.kept.stay'),
          unknown: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.unknown.any.file.its.import.kept.stays'),
        },
        openImport: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.openimport.open.the.import'),
        importMaybe: {
          move: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.move.if.it.s.the.last.segment'),
          erase: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.erase.if.it.s.the.last.segment'),
        },
        setTakes: {
          label: ({ label }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.import.set.move.named', [['import_name', String(label ?? '')]]),
          one: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.one.this.includes.the.last.of.the.variant2'),
          oneNone: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.onenone.this.includes.the.last.of.the.variant2'),
          many: ({ k }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.imports.set.move.count', transcriptCount('imports', k)),
          manyNone: ({ k }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.imports.set.move.without.files.count', transcriptCount('imports', k)),
        },
        setErases: {
          label: ({ label }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.import.set.erase.named', [['import_name', String(label ?? '')]]),
          one: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.one.this.includes.the.last.of.the'),
          oneNone: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.onenone.this.includes.the.last.of.the'),
          many: ({ k }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.imports.set.erase.count', transcriptCount('imports', k)),
          manyNone: ({ k }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.imports.set.erase.without.files.count', transcriptCount('imports', k)),
        },
        setWaiting: {
          oneOne: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.oneone.other.segment.from.that.import.in'),
          oneMany: ({ w }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.import.set.pending.count', transcriptCount('waiting_segments', w)),
          manyOne: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.manyone.other.segment.from.those.imports.in'),
          manyMany: ({ w }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.imports.set.pending.count', transcriptCount('waiting_segments', w)),
        },
        setStays: {
          label: ({ label }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.imports.stay.named', [['import_name', String(label ?? '')]]),
          one: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.one.the.file.import.kept.stays.in'),
          many: ({ j }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.imports.stay.count', transcriptCount('imports', j)),
        },
        setMaybe: {
          move: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.move.if.these.include.the.last.segments'),
          erase: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.erase.if.these.include.the.last.segments'),
        },
      },
      // Erasing a segment erases its original media waiting in the trash too.
      originalsGo: {
        one: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.one.its.original.media.in.the.trash'),
        many: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.many.their.original.media.in.the.trash'),
      },
      erase: {
        title: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.title.erase.this.segment.now'),
        reach: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.reach.it.leaves.your.journal.now.and'),
        lead: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.lead.this.erases.the.segment.s.own'),
        stays: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.stays.activities.made.from.it.drop.it'),
        found: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.found.detected.entities.drop.it.too.and.variant2'),
      },
      parts: {
        audio: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.audio.audio'),
        screen: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.screen.screen.frames'),
        transcript: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.transcript.transcript'),
        analysis: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.analysis.analysis.stored.with.it'),
        location: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.location.location'),
        // J8 in the design system's screenshots copy file: a screenshot's
        // segment holds its screenshot, not audio or frames.
        screenshot: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.screenshot.screenshot'),
        photo: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.photo.photo'),
        messages: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.messages.messages'),
        conversation: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.conversation.conversation'),
      },
      buttons: {
        eraseNow: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.erasenow.erase.now'),
        cancel: window.JournalI18n.render('journal.window.transcripts.trash.copy.cancel.cancel'),
        moveToTrash: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.trash.copy.movetotrash.move.to.trash'),
        back: window.JournalI18n.render('journal.window.transcripts.trash.copy.back.back'),
        recover: window.JournalI18n.render('journal.window.transcripts.trash.copy.recover.recover'),
        openTrash: window.JournalI18n.render('journal.window.transcripts.trash.copy.opentrash.open.trash'),
      },
      receipt: {
        moved: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.moved.to.the.trash'), message: ({ date }) => window.JournalI18n.render('journal.window.transcripts.trash.receipt.segment.erase.date', [['erase_date', String(date ?? '')]]) },
        recovered: { title: window.JournalI18n.render('journal.window.transcripts.trash.receipt.segment.recovered'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.it.s.back.in.your.journal') },
        erased: { title: window.JournalI18n.render('journal.window.transcripts.trash.receipt.segment.erased') },
        recoverFailed: { title: window.JournalI18n.render('journal.window.transcripts.trash.receipt.segment.recovery.failed'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.it.s.still.in.the.trash') },
        moveRefused: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.couldn.t.move.that.to.the'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.it.s.still.in.your.journal') },
        eraseRefused: { title: window.JournalI18n.render('journal.window.transcripts.trash.receipt.segment.erase.failed'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.it.s.still.in.your.journal') },
        eraseFailed: { title: window.JournalI18n.render('journal.window.transcripts.trash.receipt.segment.erase.failed'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.it.s.in.the.trash.you') },
        noAnswerMove: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.your.journal.didn.t.answer'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.it.may.already.be.in.the') },
        noAnswerErase: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.your.journal.didn.t.answer'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.it.may.already.be.erased.reload') },
        noAnswerRecover: { title: window.JournalI18n.render('journal.window.transcripts.trash.copy.title.your.journal.didn.t.answer'), message: window.JournalI18n.render('journal.window.transcripts.trash.copy.message.it.may.already.be.back.in') },
      },
    };
  }

  function plainSegment(seg) {
    if (!seg || typeof seg !== 'object') return seg;
    const copy = {};
    Object.keys(seg).forEach((key) => {
      const value = seg[key];
      if (typeof value === 'function') return;
      if (value && typeof value === 'object' && typeof value.nodeType === 'number') return;
      copy[key] = value;
    });
    return copy;
  }

  function renderTranscriptCopy(message, values) {
    return typeof message === 'function' ? message(values) : message;
  }

  function showTranscriptNotice(notice) {
    window.AppServices.notifications.show(notice);
  }

  function awayTrashReceipt(copy, kind, fill = {}, buttons = []) {
    const card = copy.receipt[kind];
    const settled = kind === 'moved' || kind === 'recovered' || kind === 'erased';
    const notice = {
      app: 'transcripts',
      icon: settled ? (kind === 'recovered' ? 'undo-2' : 'trash-2') : 'triangle-alert',
      title: renderTranscriptCopy(card.title, fill),
      dismissible: true,
      buttons,
    };
    if (card.message && (kind !== 'moved' || fill.date)) notice.message = renderTranscriptCopy(card.message, fill);
    if (settled) notice.autoDismiss = 10000;
    showTranscriptNotice(notice);
  }

  function awaySetReceipt(copy, card, fill = {}, buttons = [], options = {}) {
    const notice = {
      app: 'transcripts',
      icon: options.icon || (options.settled ? 'trash-2' : 'triangle-alert'),
      title: renderTranscriptCopy(card.title, fill),
      dismissible: true,
      buttons,
    };
    const message = options.message !== undefined ? options.message : card.message && renderTranscriptCopy(card.message, fill);
    if (message) notice.message = message;
    if (options.settled) notice.autoDismiss = 10000;
    showTranscriptNotice(notice);
  }

  function awayOpenTrash(copy) {
    return { label: copy.buttons.openTrash, onClick: goToSettingsTrash };
  }

  function segmentRecoverClick(trashId, seg, segDay, copy) {
    const snapshot = plainSegment(seg);
    return async function () {
      if (transcriptHooks && transcriptsShowing(segDay)) {
        await transcriptHooks.recoverSegment(trashId, snapshot, segDay);
        return;
      }
      await recoverSegmentAway(trashId, copy);
    };
  }

  async function recoverSegmentAway(trashId, copy) {
    let response;
    try {
      response = await fetch('/app/settings/api/trash/recover', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ids: [trashId], page: 'transcripts' }),
      });
    } catch (_) {
      awayTrashReceipt(copy, 'noAnswerRecover');
      return;
    }
    const data = response.ok ? await response.json().catch(() => null) : null;
    if (response.ok && !data) {
      awayTrashReceipt(copy, 'noAnswerRecover');
      return;
    }
    if (Array.isArray(data?.done) && data.done.includes(trashId)) {
      awayTrashReceipt(copy, 'recovered');
      return;
    }
    awayTrashReceipt(copy, 'recoverFailed', {}, [awayOpenTrash(copy)]);
  }

  function setRecoverClick(items, segDay, copy) {
    const snapshot = items.map(({ seg, entry }) => ({
      seg: plainSegment(seg),
      entry: { trash: { id: entry && entry.trash && entry.trash.id } },
    }));
    return async function () {
      if (transcriptHooks && transcriptsShowing(segDay)) {
        await transcriptHooks.recoverSet(snapshot, segDay);
        return;
      }
      await recoverSetAway(snapshot, copy);
    };
  }

  async function recoverSetAway(moved, copy) {
    const cards = copy.setReceipt;
    const ids = moved.map(({ entry }) => entry.trash.id);
    const one = ids.length === 1;
    let response;
    try {
      response = await fetch('/app/settings/api/trash/recover', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ ids, page: 'transcripts' }),
      });
    } catch (_) {
      if (one) awayTrashReceipt(copy, 'noAnswerRecover');
      else awaySetReceipt(copy, cards.noAnswerRecover);
      return;
    }
    const data = response.ok ? await response.json().catch(() => null) : null;
    if (response.ok && !data) {
      if (one) awayTrashReceipt(copy, 'noAnswerRecover');
      else awaySetReceipt(copy, cards.noAnswerRecover);
      return;
    }
    const back = moved.filter(({ entry }) => Array.isArray(data?.done) && data.done.includes(entry.trash.id));
    if (back.length && back.length === ids.length) {
      if (one) awayTrashReceipt(copy, 'recovered');
      else awaySetReceipt(copy, cards.recovered, { n: ids.length }, [], { settled: true, icon: 'undo-2' });
    } else if (back.length) {
      const others = ids.length - back.length;
      awaySetReceipt(copy, cards.partRecovered, { done: back.length, n: ids.length }, [awayOpenTrash(copy)], {
        message: cards.partRecoveredMessage(others),
      });
    } else if (one) {
      awayTrashReceipt(copy, 'recoverFailed', {}, [awayOpenTrash(copy)]);
    } else {
      awaySetReceipt(copy, cards.recoverFailed, {}, [awayOpenTrash(copy)]);
    }
  }

  function mountTranscripts(ctx) {
    // The receipt of a delete looks this mount up by this object, and only
    // while it is still the one showing. A throw before it is published leaves
    // none, including one a previous mount published.
    transcriptHooks = null;
    let noticeHooks = null;
    ctx.onDispose(() => {
      if (transcriptHooks === noticeHooks) transcriptHooks = null;
    });

    // Timers, observers and reads belong to this mount.
    const setTimeout = ctx.setTimeout;
    const clearTimeout = ctx.clearTimeout;
    const setInterval = ctx.setInterval;
    const clearInterval = ctx.clearInterval;
    const fetch = ctx.fetch;
    const apiJson = ctx.read;
    const scopedObserver = (Kind) => (Kind ? function (callback, options) {
      return ctx.observe(new Kind(callback, options));
    } : Kind);
    const ResizeObserver = scopedObserver(window.ResizeObserver);
    const MutationObserver = scopedObserver(window.MutationObserver);
    const IntersectionObserver = scopedObserver(window.IntersectionObserver);

    // A read of the day on the page. After a day step its answer is never
    // drawn, and it is aborted. Each of those reads is its own pending
    // promise, so one shared promise cannot keep every day that moved on.
    let dayScope = window.SolStore.createAbort();
    let dayEpoch = 0;
    let nowTimer = null;
    const dayInit = (init) => Object.assign({}, init, {
      signal: window.SolStore.eitherSignal(init?.signal || null, dayScope.signal)
    });
    const unsettled = () => new Promise(() => {});
    const forTheDay = (request) => {
      const asked = dayEpoch;
      return request.then(
        (value) => (asked === dayEpoch ? value : unsettled()),
        (error) => (asked === dayEpoch ? Promise.reject(error) : unsettled())
      );
    };
    const dayFetch = (url, init) => forTheDay(fetch(url, dayInit(init)));
    const dayRead = (url, opts) => forTheDay(apiJson(url, dayInit(opts)));

    let reportTranscriptsReady = ctx.ready;

    const escapeHtml = (value) => window.AppServices.escapeHtml(value);
    // An audio line's words as captured, with each possible name the journal
    // suggests shown right after the words it may refer to, as tentative.
    // `at` is a UTF-16 offset into the words; the words themselves never change.
    const renderAudioText = (item) => {
      const text = item.markdown || '';
      const marks = Array.isArray(item.possible_names) ? item.possible_names : [];
      let html = '';
      let cursor = 0;
      marks.forEach((mark) => {
        const at = Math.min(text.length, Math.max(cursor, Number(mark.at) || 0));
        html += escapeHtml(text.slice(cursor, at));
        html += ` <span class="tr-possible-name">${window.JournalI18n.renderHtml('journal.window.transcripts.entry.possible.name', [['entity_name', String(mark.name || '')]])}</span>`;
        cursor = at;
      });
      return html + escapeHtml(text.slice(cursor));
    };
    // Timeline bounds - computed dynamically from content
    const DEFAULT_START = 8 * 60;   // 8:00 AM default
    const DEFAULT_END = 20 * 60;    // 8:00 PM default
    const MIN_SPAN = 12 * 60;       // Minimum 12-hour span
    const BUFFER = 30;              // 30-minute buffer on each side

    let timelineStart = DEFAULT_START;
    let timelineEnd = DEFAULT_END;

    const STEP = 15;
    const DEFAULT_LEN = 60;
    const MIN_LEN = 15;
    // .tr-timeline-label / .tr-zoom-timeline-label height. trGrid/trLabels/
    // trSegments/trBodyEvents (and zoom-rail equivalents) sit below it via
    // their own CSS top inset, but trSelWrap and .tr-now-marker render directly
    // against the rail's padding box and must add this offset themselves.
    const HEADER_HEIGHT = 16;

    // The day on the page. A day step changes it in place (changeDay).
    let day = ctx.route.day;
    let TR_COPY = {};

    // This view uses sentence case for these two headings on every day step.
    const dayHeading = document.querySelector('[data-date-nav-heading]');
    if (dayHeading) {
      const dayLabels = new Map([
        [window.JournalI18n.render('journal.window.date.today'), window.JournalI18n.render('journal.window.transcripts.date.today')],
        [window.JournalI18n.render('journal.window.date.yesterday'), window.JournalI18n.render('journal.window.transcripts.date.yesterday')]
      ]);
      const setDayHeadingLabel = () => {
        const text = dayHeading.textContent;
        const label = dayLabels.get(text);
        if (label && label !== text) dayHeading.textContent = label;
      };
      setDayHeadingLabel();
      new MutationObserver(setDayHeadingLabel).observe(dayHeading, { childList: true, characterData: true, subtree: true });
    }

    // Elements - main timeline
    const timeline = document.getElementById('trTimeline');
    const grid = document.getElementById('trGrid');
    const labels = document.getElementById('trLabels');
    const segmentsLane = document.getElementById('trSegments');
    const bodyEventsLane = document.getElementById('trBodyEvents');
    const selWrap = document.getElementById('trSelWrap');
    const sel = selWrap.querySelector('.tr-sel');

    // Elements - zoom timeline
    const zoom = document.getElementById('trZoom');
    const zoomGrid = document.getElementById('trZoomGrid');
    const zoomLabels = document.getElementById('trZoomLabels');
    const zoomSegments = document.getElementById('trZoomSegments');
    // The device heading each stream column of the detail rail.
    const zoomStreams = document.getElementById('trZoomStreams');
    // The card widens the detail rail for a day with several streams.
    const railCard = document.querySelector('.tr-card');
    const segmentPicker = document.getElementById('trSegmentPicker');
    const segmentPickerLabel = document.getElementById('trSegmentPickerLabel');
    const segmentChoiceId = seg => JSON.stringify([seg.stream, seg.key]);
    segmentPicker.addEventListener('change', () => {
      const seg = allSegments.find(item => segmentChoiceId(item) === segmentPicker.value);
      if (seg) selectSegment(seg);
      window.SolFocus.keepAfterPick(segmentPicker, ctx);
    });

    // Elements - content panel
    const titleEl = document.querySelector('.tr-title');
    const copyStreamBtn = document.getElementById('trCopyStreamBtn');
    const toastEl = document.getElementById('trToast');
    let toastTimer = null;
    function showToast(msg) {
      if (!toastEl) return;
      toastEl.textContent = msg;
      toastEl.hidden = false;
      clearTimeout(toastTimer);
      toastTimer = setTimeout(() => { toastEl.hidden = true; }, 2500);
    }

    async function clipboardWriteText(text) {
      if (!navigator.clipboard || typeof navigator.clipboard.writeText !== 'function') {
        return false;
      }
      try {
        await navigator.clipboard.writeText(text);
        return true;
      } catch (err) {
        window.logError?.(err, { context: 'transcripts: clipboard write failed' });
        return false;
      }
    }

    if (copyStreamBtn) {
      copyStreamBtn.addEventListener('click', async () => {
        const stream = (selectedSegment && selectedSegment.stream) || (titleEl ? titleEl.title : '') || '';
        if (!stream) return;
        const ok = await clipboardWriteText(stream);
        showToast(ok ? window.JournalI18n.render('journal.window.transcripts.owner.status.or.action.stream.id.copied') : window.JournalI18n.render('journal.window.transcripts.owner.status.or.action.couldn.t.copy.stream.id'));
      });
    }
    const rangeText = document.getElementById('trRangeText');
    const tabsContainer = document.getElementById('trTabs');
    tabsContainer.addEventListener('keydown', e => {
      if (e.key !== 'ArrowLeft' && e.key !== 'ArrowRight') return;
      e.preventDefault();
      const tabs = [...tabsContainer.querySelectorAll('.tr-tab')];
      const idx = tabs.indexOf(e.target);
      if (idx < 0) return;
      const next = e.key === 'ArrowRight'
        ? tabs[(idx + 1) % tabs.length]
        : tabs[(idx - 1 + tabs.length) % tabs.length];
      activateTab(next.dataset.tab);
      next.focus();
    });
    const panel = document.getElementById('trPanel');
    const warningNotice = document.getElementById('trWarningNotice');
    const warningText = document.getElementById('trWarningText');
    const warningDetails = document.getElementById('trWarningDetails');
    const warningCaret = document.getElementById('trWarningCaret');
    const deleteBtn = document.getElementById('trDeleteBtn');
    // Select mode: several segments in this range chosen together.
    const contentEl = document.getElementById('trContent');
    const selectBtn = document.getElementById('trSelectBtn');
    const pickPanel = document.getElementById('trPickPanel');
    const pickBar = document.getElementById('trPickBar');
    const pickIdle = document.getElementById('trPickIdle');
    const pickSome = document.getElementById('trPickSome');
    const pickAll = document.getElementById('trPickAll');
    const pickAllLabel = document.getElementById('trPickAllLabel');
    const pickCount = document.getElementById('trPickCount');
    const pickDelete = document.getElementById('trPickDelete');
    const pickDoneIdle = document.getElementById('trPickDoneIdle');
    const pickDoneSome = document.getElementById('trPickDoneSome');
    const pickList = document.getElementById('trPickList');
    // Whether select mode is on, the chosen segments by choice id, and the note
    // a refused one carries.
    const segPick = { on: false, chosen: new Map(), notes: new Map() };
    const bodyPanel = document.getElementById('trBodyPanel');
    const bodyPanelTitle = document.getElementById('trBodyPanelTitle');
    const bodyPanelRange = document.getElementById('trBodyPanelRange');
    const bodyPanelBody = document.getElementById('trBodyPanelBody');
    const bodyPanelClose = document.getElementById('trBodyPanelClose');
    const deleteSegmentModal = document.getElementById('trDeleteSegmentModal');
    const deleteSegmentModalBody = document.getElementById('trDeleteSegmentModalBody');
    const deleteSegmentModalHeader = document.getElementById('trDeleteSegmentModalHeader');
    const deleteSegmentModalTitle = document.getElementById('trDeleteSegmentModalTitle');
    const deleteSegmentModalClose = document.getElementById('trDeleteSegmentModalClose');
    const deleteSegmentModalMoveFoot = document.getElementById('trDeleteSegmentModalMoveFoot');
    const deleteSegmentModalEraseFoot = document.getElementById('trDeleteSegmentModalEraseFoot');
    const deleteSegmentModalErase = document.getElementById('trDeleteSegmentModalErase');
    const deleteSegmentModalCancel = document.getElementById('trDeleteSegmentModalCancel');
    const deleteSegmentModalConfirm = document.getElementById('trDeleteSegmentModalConfirm');
    const deleteSegmentModalBack = document.getElementById('trDeleteSegmentModalBack');
    const deleteSegmentModalEraseConfirm = document.getElementById('trDeleteSegmentModalEraseConfirm');
    // The "pick a time of day" invitation is for a day that has loaded.
    // While the day is read the panel says so; on a slow link the invitation
    // read as an empty day.
    const dayInvitationHtml = window.SurfaceState.empty({
      icon: window.ConveyIcons.svg('clock'),
      heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.your.day.at.a.glance'),
      desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.pick.part.of.the.day.on')
    });

    function emptyIconForModality(modality) {
      switch (modality) {
        case 'audio':
          return window.ConveyIcons.svg('volume-2');
        case 'screen':
          return window.ConveyIcons.svg('monitor');
        case 'image':
          return window.ConveyIcons.svg('eye');
        case 'signals':
          return window.ConveyIcons.svg('activity');
        case 'transcript':
        default:
          return window.ConveyIcons.svg('file-text');
      }
    }

    const BTN_LABEL_PENDING = window.JournalI18n.render('journal.window.transcripts.btn.label.pending.label.analyze.now');
    const BTN_LABEL_FAILED = window.JournalI18n.render('journal.window.transcripts.btn.label.failed.label.try.again');
    const ANALYZING_LABEL = window.JournalI18n.render('journal.window.transcripts.analyzing.label.label.analyzing');
    const AWAITING_THINKING_LABEL = window.JournalI18n.render('journal.window.transcripts.awaiting.thinking.label.label.awaiting.thinking');
    const BTN_TITLE = modality => ({
      image: window.JournalI18n.render('journal.window.transcripts.btn.title.image.describe.this.segment.s.photos.and'),
      audio: window.JournalI18n.render('journal.window.transcripts.btn.title.audio.transcribe.this.segment.s.audio.and'),
      screen: window.JournalI18n.render('journal.window.transcripts.btn.title.screen.describe.this.segment.s.screen.and'),
    })[modality] || window.JournalI18n.render('journal.window.transcripts.btn.title.label.process.this.segment.and.add.the');
    // A Granola meeting's own strings. Granola's notes sit apart from its
    // speech, and its speaker labels are shown as Granola's, never as a voice
    // the journal recognised.
    const GRANOLA_COPY = Object.freeze({
      heading: window.JournalI18n.render('journal.window.transcripts.granola.copy.heading.granola.s.notes.on.this.meeting'),
      summary: window.JournalI18n.render('journal.window.transcripts.granola.copy.summary.summary'),
      privateNotes: window.JournalI18n.render('journal.window.transcripts.granola.copy.privatenotes.your.notes'),
      attendees: window.JournalI18n.render('journal.window.transcripts.granola.copy.attendees.attendees'),
      calendar: window.JournalI18n.render('journal.window.transcripts.granola.copy.calendar.calendar.event'),
      open: window.JournalI18n.render('journal.window.transcripts.granola.copy.open.open.in.granola'),
      speakerAssistive: window.JournalI18n.render('journal.window.transcripts.granola.copy.speakerassistive.as.granola.labelled.the.speaker'),
      noTranscript: window.JournalI18n.render('journal.window.transcripts.granola.copy.notranscript.granola.has.no.transcript.for.this'),
    });

    // The screenshots source's owner strings, keyed to their ids in the design
    // system's screenshots copy file. Owner copy: voice-checked there.
    const SCREENSHOT_COPY = Object.freeze({
      J1: window.JournalI18n.render('journal.window.transcripts.screenshot.copy.j1.screenshot'),
      J3: window.JournalI18n.render('journal.window.transcripts.screenshot.copy.j3.the.text.in.this.screenshot.isn'),
      J4: window.JournalI18n.render('journal.window.transcripts.screenshot.copy.j4.screenshots'),
      J5: window.JournalI18n.render('journal.window.transcripts.screenshot.copy.j5.screenshot'),
      J6: window.JournalI18n.render('journal.window.transcripts.screenshot.copy.j6.thinking.isn.t.set.up.yet'),
      J6_ACTION: window.JournalI18n.render('journal.window.transcripts.screenshot.copy.j6.action.set.up.thinking'),
      J7: window.JournalI18n.render('journal.window.transcripts.screenshot.copy.j7.the.text.in.this.screenshot.couldn'),
    });
    const SCREENSHOT_GLYPH = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M4 8.5V5.5A1.5 1.5 0 0 1 5.5 4h3M15.5 4h3A1.5 1.5 0 0 1 20 5.5v3M20 15.5v3a1.5 1.5 0 0 1-1.5 1.5h-3M8.5 20h-3A1.5 1.5 0 0 1 4 18.5v-3"/><circle cx="12" cy="12" r="2.2"/></svg>';

    // Shared / imported photo owner copy. Owner copy: voice-checked.
    const PHOTO_COPY = Object.freeze({
      label: window.JournalI18n.render('journal.window.transcripts.photo.copy.label.photo'),
      viewerLabel: window.JournalI18n.render('journal.window.transcripts.photo.copy.viewerlabel.photo.viewer'),
      captionLabel: window.JournalI18n.render('journal.window.transcripts.photo.copy.captionlabel.your.note'),
      descriptionLabel: window.JournalI18n.render('journal.window.transcripts.photo.copy.descriptionlabel.generated.description'),
      noProcessing: window.JournalI18n.render('journal.window.transcripts.photo.copy.noprocessing.thinking.isn.t.set.up.yet'),
      noProcessingAction: window.JournalI18n.render('journal.window.transcripts.photo.copy.noprocessingaction.set.up.thinking'),
      unavailable: window.JournalI18n.render('journal.window.transcripts.photo.copy.unavailable.this.photo.couldn.t.be.described'),
      uploadFallback: window.JournalI18n.render('journal.window.transcripts.photo.copy.uploadfallback.no.date.in.photo.placed.at'),
    });
    const PHOTO_GLYPH = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><rect x="3.5" y="5" width="17" height="14" rx="2"/><circle cx="9" cy="10" r="1.5"/><path d="M20.5 16l-4.5-4.5-6.5 6.5"/></svg>';

    const screenshotStreamKind = window.JournalI18n.renderEnglish('journal.window.transcripts.screenshot.copy.j4.screenshots');

    // A screenshots stream, by what its label says.
    function isScreenshotsStream(stream) {
      return Boolean(stream) && window.JournalFormat.streamSays(stream).what === screenshotStreamKind;
    }

    // A photo moment: an imported image segment or an image-only zero-duration segment.
    function isPhotoSegment(seg) {
      if (!seg) return false;
      if (seg.stream === 'import.image') return true;
      const types = seg.types || [];
      return types.includes('image') && !types.includes('audio') && !types.includes('screen') && seg.start === seg.end;
    }

    function isMomentSegment(seg) {
      return Boolean(seg) && (isScreenshotsStream(seg.stream) || isPhotoSegment(seg));
    }

    // The device a screenshot came from, by the name the journal already
    // shows for its stream, without the stream's own "· screenshots".
    function screenshotDevice(stream) {
      const label = humanizeStreamName(stream);
      const suffix = ' · ' + screenshotStreamKind;
      return label.endsWith(suffix) ? label.slice(0, -suffix.length) : label;
    }

    const ANALYZE_POLL_INTERVAL_MS = 3000;
    const ANALYZE_POLL_MAX_MS = 30 * 60 * 1000;

    function dataStateCopy(modality, state) {
      if (state !== 'pending' && state !== 'analyzing' && state !== 'failed') return '';
      if (modality === 'audio' || modality === 'screen' || modality === 'image') {
        return window.JournalI18n.render(`journal.window.transcripts.analysis.${state}.${modality}`);
      }
      return window.JournalI18n.render(`journal.window.transcripts.analysis.${state}.other`, [['modality', String(modality)]]);
    }

    function warningTimestampLabel(ts) {
      const parsed = new Date(ts);
      if (Number.isNaN(parsed.getTime())) return window.JournalI18n.render('journal.window.transcripts.warning.time.unknown');
      return window.JournalI18n.render('journal.window.transcripts.warning.age', [['age', relativeTime(Date.now() - parsed.getTime())]]);
    }

    function renderWarningNotice(data) {
      const count = data.warnings || 0;
      const details = Array.isArray(data.warning_details) ? data.warning_details : [];
      warningDetails.innerHTML = '';
      warningNotice.setAttribute('aria-expanded', 'false');
      warningDetails.hidden = true;
      warningCaret.textContent = '▾';
      if (count <= 0) {
        warningNotice.classList.remove('visible');
        return;
      }
      warningText.textContent = details.some(detail => detail.type === 'audio_capture')
        ? window.JournalI18n.render('journal.window.transcripts.visible.text.audio.may.be.incomplete.for.this')
        : details.some(detail => detail.type === 'audio_capture_history')
        ? window.JournalI18n.render('journal.window.transcripts.visible.text.an.earlier.audio.copy.attempt.had')
        : window.JournalI18n.render('journal.window.transcripts.warning.processing.count', transcriptCount('warnings', count));
      if (details.length === 0) {
        const item = document.createElement('li');
        item.textContent = window.JournalI18n.render('journal.window.transcripts.visible.text.warning.details.unavailable');
        warningDetails.appendChild(item);
      } else {
        for (const detail of details) {
          const item = document.createElement('li');
          const type = detail.type;
          const file = detail.file || window.JournalI18n.render('journal.window.transcripts.warning.file.unknown');
          if (type === 'audio_capture' || type === 'audio_capture_history') {
            item.textContent = window.JournalI18n.render('journal.window.transcripts.warning.audio.source', [['detail', String(detail.message || window.JournalI18n.render('journal.window.transcripts.window.explanation.audio.may.be.incomplete.for.this'))], ['source_file', String(file)]]);
          } else if (type === 'desktop_activity' && detail.message) {
            item.textContent = `${detail.message}: ${file}`;
          } else {
            item.textContent = `${type || window.JournalI18n.render('journal.window.transcripts.warning.detail.label')}: ${file} (${warningTimestampLabel(detail.ts)})`;
          }
          warningDetails.appendChild(item);
        }
      }
      warningNotice.classList.add('visible');
    }

    function toggleWarningDetails() {
      const expanded = warningNotice.getAttribute('aria-expanded') === 'true';
      warningNotice.setAttribute('aria-expanded', String(!expanded));
      warningDetails.hidden = expanded;
      warningCaret.textContent = expanded ? '▾' : '▴';
    }

    const warningSummary = warningNotice.querySelector('.tr-warning-summary');
    warningSummary.addEventListener('click', toggleWarningDetails);
    warningSummary.addEventListener('keydown', (event) => {
      if (event.key !== 'Enter' && event.key !== ' ') return;
      event.preventDefault();
      toggleWarningDetails();
    });

    // State
    let height = timeline.clientHeight;
    let ppm = height / (timelineEnd - timelineStart);
    let range = { start: 9 * 60, end: 10 * 60 };
    let drag = null;
    let allSegments = [];
    let locationSegments = [];
    let locationSectionExpanded = false;
    let locationLoad = 'pending';
    let dayLoad = 'pending';
    let dayBodyWindow = null;
    let selectedSegment = null;
    let selectedSegmentZone = null;
    let updateNowPosition = null;
    // Today's day read again while the page is open: the one in flight, and
    // what the last answer drawn held.
    let todayRead = null;
    let drawnDayShape = '';
    let deleteModalReturnFocus = null;
    let deleteModalPrevOverflow = '';

    // Zoom state
    let zoomHeight = zoom.clientHeight;
    let zoomPpm = 0;

    // Modal state - all screen frames for navigation
    let allScreenFrames = [];
    let currentFrameIndex = -1;

    // ========================================
    // FrameCapture - Client-side thumbnail + on-demand full frame decoder
    // ========================================
    class FrameCapture {
      constructor() {
        // Map of video URL -> { video, ready, width, height, thumbs }
        this.videos = new Map();
        // Pending thumbnail promises per frame
        this.pendingThumbs = new Map();
        this.warnedUrls = new Set();
        this.unsupportedPlaceholderCanvases = new Set();
        // Map of `${videoUrl}|${frameId}` -> the frame's time in its video, in seconds
        this.frameTimes = new Map();
      }

      noteFrameTime(videoUrl, frameId, seconds) {
        if (!videoUrl || !frameId || !Number.isFinite(seconds) || seconds < 0) return;
        this.frameTimes.set(`${videoUrl}|${frameId}`, seconds);
      }

      // Where a frame sits in its video. Frame ids count every decoded frame, so
      // they only match seconds for a video recorded at one frame per second.
      frameSeconds(videoUrl, frameId) {
        const seconds = this.frameTimes.get(`${videoUrl}|${frameId}`);
        return seconds === undefined ? Math.max(0, frameId - 1) : seconds;
      }

      // Consume the initial presentation before any queued seek. A newly
      // registered frame callback can otherwise report an older presentation.
      loadVideo(url) {
        if (this.videos.has(url)) return this.videos.get(url).promise;
        const video = document.createElement('video');
        video.preload = 'auto';
        video.muted = true;
        video.playsInline = true;
        video.crossOrigin = 'anonymous';
        const entry = {
          url, video, ready: false, unsupported: false, errorCode: null,
          width: 0, height: 0, thumbs: new Map(), queue: Promise.resolve(),
          confirmedTime: null, confirmedTarget: null, presentedFrames: 0, disposed: false, cancelPending: null
        };
        this.videos.set(url, entry);
        entry.promise = this._waitForFrame(entry, null, () => { video.src = url; })
          .then(() => {
            entry.ready = true;
            entry.width = video.videoWidth;
            entry.height = video.videoHeight;
            return entry;
          }).catch((error) => {
            if (!entry.disposed && video.error?.code === MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED) {
              entry.unsupported = true;
              entry.errorCode = video.error.code;
              entry.ready = true;
              if (!this.warnedUrls.has(url)) {
                this.warnedUrls.add(url);
                console.warn(`Video format not supported by this browser: ${url}`);
              }
              return entry;
            }
            this._retireVideo(entry);
            throw error;
          });
        return entry.promise;
      }

      async _withVideoQueue(videoUrl, task) {
        const entry = await this.loadVideo(videoUrl);
        const admitted = () => {
          if (entry.disposed) throw new Error('Video capture cancelled');
          return task();
        };
        const run = entry.queue.then(admitted, admitted);
        entry.queue = run.catch(() => {});
        return run;
      }

      _waitForFrame(entry, timestamp, start) {
        const video = entry.video;
        const initial = timestamp === null;
        if (entry.disposed) return Promise.reject(new Error('Video capture cancelled'));
        // The browser may round a requested time. Keep that request separate
        // from its confirmed readback so reopening it does not seek again.
        if (!initial && entry.confirmedTarget === timestamp && video.currentTime === entry.confirmedTime
            && !video.seeking && video.readyState >= 2) return Promise.resolve();
        entry.confirmedTime = null;
        entry.confirmedTarget = null;
        return new Promise((resolve, reject) => {
          let seekComplete = initial;
          let presented = typeof video.requestVideoFrameCallback !== 'function';
          let callbackId = null;
          let settled = false;
          const baseline = entry.presentedFrames;
          const cleanup = () => {
            clearTimeout(timer);
            for (const name of ['loadedmetadata', 'loadeddata']) video.removeEventListener(name, check);
            video.removeEventListener('seeked', onSeeked);
            video.removeEventListener('error', onError);
            if (callbackId !== null) video.cancelVideoFrameCallback(callbackId);
            entry.cancelPending = null;
          };
          const finish = (error) => {
            if (settled) return;
            settled = true;
            cleanup();
            if (error) {
              // A late presentation from a failed seek must never satisfy the
              // next request. Retire this decoder, including its queued work.
              if (!initial) this._retireVideo(entry);
              reject(error);
            } else {
              entry.confirmedTime = video.currentTime;
              entry.confirmedTarget = initial ? video.currentTime : timestamp;
              resolve();
            }
          };
          const check = () => {
            if (seekComplete && presented && video.readyState >= 2 && !video.seeking) finish();
          };
          const onSeeked = () => { seekComplete = true; check(); };
          const onError = () => finish(new Error('Video frame could not be decoded'));
          const onFrame = (_now, metadata) => {
            callbackId = null;
            if (settled) return;
            if (metadata.presentedFrames > baseline) {
              entry.presentedFrames = metadata.presentedFrames;
              presented = true;
              check();
            } else callbackId = video.requestVideoFrameCallback(onFrame);
          };
          const timer = setTimeout(() => finish(new Error('Video frame timed out')), 30000);
          entry.cancelPending = () => finish(new Error('Video capture cancelled'));
          for (const name of ['loadedmetadata', 'loadeddata']) video.addEventListener(name, check);
          video.addEventListener('seeked', onSeeked);
          video.addEventListener('error', onError);
          if (!presented) callbackId = video.requestVideoFrameCallback(onFrame);
          try {
            if (initial) start();
            else video.currentTime = timestamp;
            check();
          } catch (error) { finish(error); }
        });
      }

      async captureThumbnail(videoUrl, frameId, width, height) {
        const entry = await this.loadVideo(videoUrl);
        if (entry.unsupported) return null;
        const cacheKey = `${videoUrl}|${frameId}`;

        if (entry.thumbs.has(frameId)) {
          return entry.thumbs.get(frameId);
        }

        if (this.pendingThumbs.has(cacheKey)) {
          return this.pendingThumbs.get(cacheKey);
        }

        const pending = this._withVideoQueue(videoUrl, async () => {
          const video = entry.video;
          const timestamp = this.frameSeconds(videoUrl, frameId);
          await this._waitForFrame(entry, timestamp);
          if (entry.disposed) throw new Error('Video capture cancelled');

          const canvas = this._snapshot(video, width, height);
          entry.thumbs.set(frameId, canvas);
          return canvas;
        });

        this.pendingThumbs.set(cacheKey, pending);
        try {
          return await pending;
        } finally {
          if (this.pendingThumbs.get(cacheKey) === pending) this.pendingThumbs.delete(cacheKey);
        }
      }

      async captureFullFrame(videoUrl, frameId) {
        const entry = await this.loadVideo(videoUrl);
        if (entry.unsupported) return null;
        return this._withVideoQueue(videoUrl, async () => {
          const video = entry.video;
          const timestamp = this.frameSeconds(videoUrl, frameId);
          await this._waitForFrame(entry, timestamp);
          if (entry.disposed) throw new Error('Video capture cancelled');
          return this._snapshot(video);
        });
      }

      _snapshot(video, width, height) {
        // Draw only after frame readiness; seeked alone can still expose old
        // pixels on Android, and createImageBitmap can retain an older image.
        const canvas = document.createElement('canvas');
        canvas.width = width || video.videoWidth;
        canvas.height = height || video.videoHeight;
        const ctx = canvas.getContext('2d');
        ctx.imageSmoothingQuality = 'high';
        ctx.drawImage(video, 0, 0, canvas.width, canvas.height);
        return canvas;
      }

      async prefetchThumbnails(videoUrl, frameIds, onProgress = null, signal = null) {
        if (!frameIds || frameIds.length === 0) return null;

        const sorted = [...new Set(frameIds)].sort((a, b) => a - b);
        for (let i = 0; i < sorted.length; i += 1) {
          if (signal?.aborted) return this.videos.get(videoUrl);
          const frameId = sorted[i];
          await this.captureThumbnail(videoUrl, frameId, 120, 68);
          if (onProgress) onProgress(i + 1, sorted.length);
        }

        return this.videos.get(videoUrl);
      }

      _retireVideo(entry) {
        if (entry.disposed) return;
        entry.disposed = true;
        entry.cancelPending?.();
        entry.thumbs.clear();
        if (this.videos.get(entry.url) === entry) this.videos.delete(entry.url);
        entry.video.src = '';
        entry.video.load();
      }

      // Clear all loaded videos and cached thumbnails
      clear() {
        for (const canvas of [...this.unsupportedPlaceholderCanvases]) {
          this._clearUnsupportedPlaceholder(canvas);
        }
        for (const entry of this.videos.values()) {
          this._retireVideo(entry);
        }
        this.videos.clear();
        this.pendingThumbs.clear();
        this.warnedUrls.clear();
        this.frameTimes.clear();
      }

      _drawUnsupportedPlaceholder(canvas, width, height, errorCode = null) {
        canvas.width = width;
        canvas.height = height;

        const ctx = canvas.getContext('2d');
        ctx.fillStyle = getComputedStyle(canvas).getPropertyValue('--cream-bright').trim() || '#FEFCF8';
        ctx.fillRect(0, 0, width, height);

        canvas.__unsupportedPlaceholderState = { errorCode };
        if (!canvas.__unsupportedPlaceholderRedraw) {
          const media = matchMedia('(prefers-color-scheme: dark)');
          const listener = () => {
            const state = canvas.__unsupportedPlaceholderState;
            if (!state) return;
            if (!canvas.isConnected) {
              this._clearUnsupportedPlaceholder(canvas);
              return;
            }
            this._drawUnsupportedPlaceholder(canvas, canvas.width, canvas.height, state.errorCode);
          };
          canvas.__unsupportedPlaceholderRedraw = { media, listener };
          ctx.on(media, 'change', listener);
          this.unsupportedPlaceholderCanvases.add(canvas);
        }

        const minDim = Math.min(width, height);
        const iconWidth = minDim * 0.42;
        const iconHeight = iconWidth * 0.68;
        const radius = Math.max(4, iconWidth * 0.08);
        const strokeWidth = Math.max(2, minDim * 0.03);
        const standHeight = Math.max(6, iconHeight * 0.22);
        const textSize = Math.max(10, Math.min(height > 120 ? 24 : 14, width / (height > 120 ? 9 : 12)));
        const gap = Math.max(10, height * 0.06);
        const totalHeight = iconHeight + standHeight + gap + textSize;
        const iconX = (width - iconWidth) / 2;
        const iconY = Math.max(height * 0.14, (height - totalHeight) / 2);

        ctx.strokeStyle = getComputedStyle(canvas).getPropertyValue('--ink-faint').trim() || '#6E6453';
        ctx.lineWidth = strokeWidth;
        ctx.lineCap = 'round';
        ctx.lineJoin = 'round';

        ctx.beginPath();
        ctx.moveTo(iconX + radius, iconY);
        ctx.lineTo(iconX + iconWidth - radius, iconY);
        ctx.quadraticCurveTo(iconX + iconWidth, iconY, iconX + iconWidth, iconY + radius);
        ctx.lineTo(iconX + iconWidth, iconY + iconHeight - radius);
        ctx.quadraticCurveTo(iconX + iconWidth, iconY + iconHeight, iconX + iconWidth - radius, iconY + iconHeight);
        ctx.lineTo(iconX + radius, iconY + iconHeight);
        ctx.quadraticCurveTo(iconX, iconY + iconHeight, iconX, iconY + iconHeight - radius);
        ctx.lineTo(iconX, iconY + radius);
        ctx.quadraticCurveTo(iconX, iconY, iconX + radius, iconY);
        ctx.stroke();

        const standY = iconY + iconHeight + standHeight * 0.45;
        const standWidth = iconWidth * 0.28;
        ctx.beginPath();
        ctx.moveTo(width / 2, iconY + iconHeight);
        ctx.lineTo(width / 2, standY);
        ctx.moveTo(width / 2 - standWidth / 2, standY);
        ctx.lineTo(width / 2 + standWidth / 2, standY);
        ctx.stroke();

        ctx.beginPath();
        ctx.moveTo(iconX + iconWidth * 0.18, iconY + iconHeight * 0.2);
        ctx.lineTo(iconX + iconWidth * 0.82, iconY + iconHeight * 0.84);
        ctx.stroke();

        ctx.fillStyle = getComputedStyle(canvas).getPropertyValue('--ink-soft').trim() || '#5B5246';
        ctx.font = `${textSize}px sans-serif`;
        ctx.textAlign = 'center';
        ctx.textBaseline = 'top';
        ctx.fillText(window.JournalI18n.render('journal.window.transcripts.media.audio.format.unsupported'), width / 2, standY + gap * 0.55);
        if (errorCode !== null) {
          ctx.font = '10px ui-monospace, SFMono-Regular, Menlo, monospace';
          ctx.fillText(
            window.JournalI18n.render('journal.window.transcripts.media.video.error.code', [['code', String(errorCode)]]),
            width / 2,
            standY + gap * 0.55 + textSize + 2
          );
        }

        canvas.classList.remove('loading');
      }

      _clearUnsupportedPlaceholder(canvas) {
        const redraw = canvas.__unsupportedPlaceholderRedraw;
        if (redraw) redraw.media.removeEventListener('change', redraw.listener);
        this.unsupportedPlaceholderCanvases.delete(canvas);
        delete canvas.__unsupportedPlaceholderRedraw;
        delete canvas.__unsupportedPlaceholderState;
      }

      // Draw thumbnail to canvas (no overlays - those are only for full frame view)
      async drawThumbnail(canvas, videoUrl, frameId, options = {}) {
        const { width, height } = options;

        try {
          const thumb = await this.captureThumbnail(videoUrl, frameId, width, height);
          if (!thumb) {
            this._drawUnsupportedPlaceholder(
              canvas,
              width || 120,
              height || 68,
              this.videos.get(videoUrl)?.errorCode ?? null
            );
            return false;
          }

          canvas.width = width || thumb.width;
          canvas.height = height || thumb.height;

          this._clearUnsupportedPlaceholder(canvas);
          const ctx = canvas.getContext('2d');
          ctx.drawImage(thumb, 0, 0, canvas.width, canvas.height);

          canvas.classList.remove('loading');
          return true;
        } catch (err) {
          canvas.classList.remove('loading');
          console.warn('Thumbnail draw failed:', err);
          return false;
        }
      }

      // Draw full-resolution frame to canvas (no caching)
      async drawFull(canvas, videoUrl, frameId, options = {}) {
        const { boxCoords, participants, objects } = options;
        let frame;

        try {
          frame = await this.captureFullFrame(videoUrl, frameId);
          if (!frame) {
            this._drawUnsupportedPlaceholder(
              canvas,
              640,
              360,
              this.videos.get(videoUrl)?.errorCode ?? null
            );
            return false;
          }

          canvas.width = frame.width;
          canvas.height = frame.height;

          this._clearUnsupportedPlaceholder(canvas);
          const ctx = canvas.getContext('2d');
          ctx.drawImage(frame, 0, 0, canvas.width, canvas.height);

          this._applyOverlays(ctx, canvas, frame.width, frame.height, { boxCoords, participants, objects });

          canvas.classList.remove('loading');
          return true;
        } catch (err) {
          canvas.classList.remove('loading');
          console.warn('Full frame draw failed:', err);
          return false;
        } finally {
          // This full-size snapshot is temporary; only thumbnails are cached.
          if (frame) { frame.width = 0; frame.height = 0; }
        }
      }

      // Draw a still photo with its overlays; used when there are boxes to show.
      async drawStill(canvas, imageUrl, options = {}) {
        try {
          const img = new Image();
          img.src = imageUrl;
          await img.decode();
          canvas.width = img.naturalWidth;
          canvas.height = img.naturalHeight;
          const ctx = canvas.getContext('2d');
          ctx.drawImage(img, 0, 0);
          this._applyOverlays(ctx, canvas, img.naturalWidth, img.naturalHeight, options);
          canvas.classList.remove('loading');
          return true;
        } catch (err) {
          canvas.classList.remove('loading');
          console.warn('Still draw failed:', err);
          return false;
        }
      }

      _applyOverlays(ctx, canvas, sourceWidth, sourceHeight, options = {}) {
        const { boxCoords, participants, objects } = options;
        const scaleX = canvas.width / sourceWidth;
        const scaleY = canvas.height / sourceHeight;

        // Detected objects: boxes are in the detector's image pixels, which can
        // differ from the decoded frame's size. When the shapes disagree (a photo
        // the browser rotated from its orientation tag), the boxes would land in
        // the wrong place, so none are drawn. Neutral ink, not a hue.
        const items = objects?.items || [];
        const detectorWidth = objects?.image?.width || sourceWidth;
        const detectorHeight = objects?.image?.height || sourceHeight;
        const sameShape = Math.abs(detectorWidth / detectorHeight - canvas.width / canvas.height)
          <= 0.02 * (canvas.width / canvas.height);
        if (items.length > 0 && sameShape) {
          const objectScaleX = canvas.width / detectorWidth;
          const objectScaleY = canvas.height / detectorHeight;
          const ink = '#6E6453';
          ctx.font = '12px system-ui, sans-serif';
          for (const item of items) {
            const [xMin, yMin, xMax, yMax] = item.bbox || [];
            if (![xMin, yMin, xMax, yMax].every(Number.isFinite)) continue;
            const x = xMin * objectScaleX;
            const y = yMin * objectScaleY;
            ctx.strokeStyle = ink;
            ctx.lineWidth = 2;
            ctx.strokeRect(x, y, (xMax - xMin) * objectScaleX, (yMax - yMin) * objectScaleY);
            const label = String(item.label || '');
            const textWidth = ctx.measureText(label).width;
            const labelTop = Math.max(0, y - 18);
            ctx.fillStyle = ink;
            ctx.fillRect(x, labelTop, textWidth + 8, 18);
            ctx.fillStyle = '#fff';
            ctx.fillText(label, x + 4, labelTop + 13);
          }
        }

        if (boxCoords && boxCoords.length === 4) {
          const [xMin, yMin, xMax, yMax] = boxCoords;
          ctx.strokeStyle = '#ef4444';
          ctx.lineWidth = 3;
          ctx.strokeRect(
            xMin * scaleX,
            yMin * scaleY,
            (xMax - xMin) * scaleX,
            (yMax - yMin) * scaleY
          );
        }

        if (participants && participants.length > 0) {
          for (const p of participants) {
            const x = (p.left / 100) * canvas.width;
            const y = (p.top / 100) * canvas.height;
            const w = (p.width / 100) * canvas.width;
            const h = (p.height / 100) * canvas.height;

            const colors = {
              speaking: '#fbbf24',
              active: '#4ade80',
              muted: '#f87171',
              presenting: '#6E6453', /* --ink-faint (G1-110): neutral, not a hue, per the no-hue-for-informational rule */
              unknown: '#9ca3af'
            };
            ctx.strokeStyle = colors[p.status] || colors.unknown;
            ctx.lineWidth = 2;
            ctx.setLineDash([5, 5]);
            ctx.strokeRect(x, y, w, h);
            ctx.setLineDash([]);

            const labelY = y + h + 16;
            ctx.font = '11px system-ui, sans-serif';
            const textWidth = ctx.measureText(p.name).width;
            ctx.fillStyle = colors[p.status] || colors.unknown;
            ctx.fillRect(x, y + h + 2, textWidth + 8, 16);

            ctx.fillStyle = '#000';
            ctx.fillText(p.name, x + 4, labelY - 3);
          }
        }
      }
    }

    // Global frame capture instance
    let frameCapture = new FrameCapture();

    // Utilities
    const y = (m) => (m - timelineStart) * ppm;
    const mFromY = (py) => Math.max(timelineStart, Math.min(timelineEnd, Math.round(py / ppm) + timelineStart));
    const snap = (m) => Math.round(m / STEP) * STEP;
    const hhmm = (m) => String(Math.floor(m / 60)).padStart(2, '0') + ':' + String(m % 60).padStart(2, '0');
    // The day axis can run to minute 1440, which is the end of the day rather
    // than a 24th hour. "24:00" is not a clock time anyone reads, so name it --
    // and one instant carries one name, so minute 0 is the same midnight and
    // reads the same at both ends. S-12.
    const clockLabel = (m) => (m % 1440 === 0 ? window.JournalI18n.render('journal.window.transcripts.timeline.midnight') : hhmm(m));

    // Zoom utilities - map minutes to pixels within the zoomed range
    const zoomY = (m) => (m - range.start) * zoomPpm;

    function parseTime(timeStr) {
      if (!timeStr) return 0;
      const [hh, mm] = timeStr.split(':').map(Number);
      return hh * 60 + mm;
    }

    function dayParts() {
      return {
        year: Number(day.slice(0, 4)),
        month: Number(day.slice(4, 6)) - 1,
        date: Number(day.slice(6, 8))
      };
    }

    function minuteForIso(value, offsetSeconds = null) {
      let origin;
      if (typeof offsetSeconds === 'number') {
        const parts = dayParts();
        origin = Date.UTC(parts.year, parts.month, parts.date, 0, 0, 0) - offsetSeconds * 1000;
      } else if (window.JournalClock) {
        origin = window.JournalClock.dayStart(day);
      } else {
        const parts = dayParts();
        origin = new Date(parts.year, parts.month, parts.date, 0, 0, 0, 0).getTime();
      }
      const moment = new Date(value);
      return Math.round((moment.getTime() - origin) / 60000);
    }

    function bodyWindowUrl(fromIso, toIso) {
      const params = new URLSearchParams({ from: fromIso, to: toIso });
      return `/app/body/api/window?${params.toString()}`;
    }

    async function fetchBodyWindow(fromIso, toIso, signal = null) {
      const response = await dayFetch(bodyWindowUrl(fromIso, toIso), { signal });
      if (!response.ok) {
        throw new Error('body window failed: ' + response.status);
      }
      return response.json();
    }

    function fetchDayBodyWindow(signal = null) {
      let windowSpan;
      if (window.JournalClock) {
        windowSpan = window.JournalClock.span(day, 0, 1440, null);
      } else {
        const parts = dayParts();
        windowSpan = {
          fromMs: new Date(parts.year, parts.month, parts.date, 0, 0, 0, 0).getTime(),
          toMs: new Date(parts.year, parts.month, parts.date + 1, 0, 0, 0, 0).getTime(),
        };
      }
      return fetchBodyWindow(
        new Date(windowSpan.fromMs).toISOString(),
        new Date(windowSpan.toMs).toISOString(),
        signal
      );
    }

    // `captureZone` is the segment's own, from its response: the segment's
    // times are that device's clock, so its body readings are fetched there.
    function fetchSegmentBodyWindow(seg, signal = null, captureZone = null) {
      if (!seg || !seg.start || !seg.end) {
        return Promise.resolve({ has_data: false });
      }
      const startMin = parseTime(seg.start);
      const endMin = parseTime(seg.end);
      const offset = typeof captureZone?.utc_offset_seconds === 'number'
        ? captureZone.utc_offset_seconds
        : null;
      let windowSpan;
      if (window.JournalClock) {
        windowSpan = window.JournalClock.span(day, startMin, endMin, offset);
      } else {
        const parts = dayParts();
        windowSpan = {
          fromMs: new Date(parts.year, parts.month, parts.date, 0, startMin, 0, 0).getTime(),
          toMs: new Date(parts.year, parts.month, parts.date, 0, endMin, 0, 0).getTime(),
        };
      }
      return fetchBodyWindow(
        new Date(windowSpan.fromMs).toISOString(),
        new Date(windowSpan.toMs).toISOString(),
        signal
      );
    }

    function isBodySummarySegment(seg = selectedSegment) {
      return seg?.stream === 'import.apple_health';
    }

    // The panel states counts, never a value: what the window holds, by kind.
    function bodyWindowParts(payload, limit = 4) {
      if (!payload || !payload.has_data) return [];
      const parts = (payload.signals || [])
        .filter(signal => signal.label && signal.count_label)
        .map(signal => `${signal.label} · ${signal.count_label}`);
      return parts.slice(0, limit);
    }

    function bodyHourSummary(hour) {
      const summary = (hour?.summary || []).filter(Boolean);
      if (summary.length) return summary.slice(0, 3).join(' · ');
      if (hour?.has_data && hour?.entry_total_label) return window.JournalI18n.render('journal.window.transcripts.body.entries.held', transcriptCount('entries', Number(hour.entry_total), String(hour.entry_total_label)));
      return '';
    }

    function bodyHourLevel(hour, maxEntries) {
      if (!hour?.has_data) return 0;
      const count = Number(hour.entry_total || 0);
      if (!maxEntries || count <= 0) return 1;
      const ratio = Math.log1p(count) / Math.log1p(maxEntries);
      return Math.max(1, Math.min(4, Math.ceil(ratio * 4)));
    }

    function renderBodyHourList(payload, limit = 12) {
      const hours = (payload?.hourly || []).filter(hour => hour.has_data);
      if (!hours.length) return '';
      const visible = hours.slice(0, limit);
      const more = hours.length > visible.length
        ? `<li><time></time><span>${window.JournalI18n.renderHtml('journal.window.transcripts.body.hours.remaining', transcriptCount('hours', hours.length - visible.length))}</span></li>`
        : '';
      return '<ul class="tr-body-hour-list">'
        + visible.map(hour => {
          const summary = bodyHourSummary(hour) || window.JournalI18n.render('journal.window.transcripts.visible.text.body.data.present');
          return `<li><time>${escapeHtml(hour.label || '')}</time><span>${escapeHtml(summary)}</span></li>`;
        }).join('\n')
        + more
        + '</ul>';
    }

    // Span gates for the day-rhythm block: the hourly strip only reads at day
    // scale, and hourly rows only add information beyond the brief when the
    // window would produce at least two rows.
    const BODY_RHYTHM_STRIP_MIN_HOURS = 6;
    const BODY_HOURLY_ROWS_MIN_HOURS = 1;

    function bodyWindowSpanHours(payload) {
      const start = Date.parse(payload?.from || '');
      const end = Date.parse(payload?.to || '');
      if (!Number.isFinite(start) || !Number.isFinite(end) || end <= start) return 0;
      return (end - start) / 3600000;
    }

    function renderBodyRhythm(payload, options = {}) {
      const hours = payload?.hourly || [];
      if (!hours.length) return '';
      const spanHours = bodyWindowSpanHours(payload);
      const showStrip = spanHours >= BODY_RHYTHM_STRIP_MIN_HOURS;
      const showRows = Boolean(options.rows) && spanHours > BODY_HOURLY_ROWS_MIN_HOURS;
      if (!showStrip && !showRows) return '';
      let grid = '';
      if (showStrip) {
        const maxEntries = hours.reduce((max, hour) => Math.max(max, Number(hour.entry_total || 0)), 0);
        const cells = hours.map((hour, index) => {
          const summary = bodyHourSummary(hour);
          const level = bodyHourLevel(hour, maxEntries);
          const classes = ['tr-body-hour', `tr-body-hour-level-${level}`];
          if (!hour.has_data) classes.push('tr-body-hour-empty');
          const hourLabel = index % 6 === 0
            ? `<span class="tr-body-hour-label">${escapeHtml(hour.label || '')}</span>`
            : '';
          const hourLabelText = hour.range_label || hour.label || window.JournalI18n.render('journal.window.transcripts.body.hour');
          const title = summary
            ? window.JournalI18n.render('journal.window.transcripts.body.hour.with.data', [['hour', hourLabelText], ['summary', summary]])
            : window.JournalI18n.render('journal.window.transcripts.body.hour.without.data', [['hour', hourLabelText]]);
          const actionAttrs = hour.has_data
            ? ` data-action="open-body-hour" data-from="${escapeHtml(hour.start || '')}" data-to="${escapeHtml(hour.end || '')}" data-range="${escapeHtml(hour.range_label || '')}"`
            : ' disabled';
          return `<button type="button" class="${classes.join(' ')}" title="${escapeHtml(title)}" aria-label="${escapeHtml(title)}"${actionAttrs}>${hourLabel}</button>`;
        }).join('\n');
        grid = `<div class="tr-body-rhythm-grid">${cells}</div>`;
      }
      const rows = showRows ? renderBodyHourList(payload, options.limit || 12) : '';
      const heading = options.title === false
        ? ''
        : `<div class="tr-body-rhythm-title">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.day.rhythm')}</div>`;
      return '<div class="tr-body-rhythm">'
        + heading
        + grid
        + rows
        + '</div>';
    }

    function renderBodyPanelSection(title, innerHtml) {
      if (!innerHtml) return '';
      return `<section class="tr-body-panel-section"><h4>${escapeHtml(title)}</h4>\n${innerHtml}\n</section>`;
    }

    // "1,234" -> "1,234 entries"; a count_label that already carries a noun
    // (e.g. "3 readings") passes through untouched. Never a bare number.
    function bodyCountText(item) {
      const label = String(item?.count_label ?? item?.count ?? '').trim();
      if (!label) return '';
      if (!/^[\p{Nd}\p{Zs}\p{P}]+$/u.test(label)) return label;
      return window.JournalI18n.render('journal.window.transcripts.body.entry.count', transcriptCount('entries', Number(item?.count), label));
    }

    function renderBodyPanelFooter() {
      return '<div class="tr-body-context-actions">'
        + `<a class="tr-body-link" href="/app/body/${day}">${window.JournalI18n.renderHtml('journal.window.transcripts.link.label.open.full.body.day')}</a>`
        + '</div>';
    }

    // Focus management mirrors the screenshot modal: opening the docked panel
    // moves focus to its close button; closing returns focus to the trigger.
    let bodyPanelTrigger = null;

    function openBodyPanelSurface() {
      if (!bodyPanel.classList.contains('visible')) {
        bodyPanelTrigger = document.activeElement;
      }
      bodyPanel.classList.add('visible');
      bodyPanel.setAttribute('aria-hidden', 'false');
      bodyPanelClose.focus();
    }

    function renderBodyPanel(payload, title, rangeLabel) {
      bodyPanelTitle.textContent = title || window.JournalI18n.render('journal.window.transcripts.visible.text.body.context');
      bodyPanelRange.textContent = rangeLabel || '';

      if (!payload || !payload.has_data) {
        bodyPanelBody.innerHTML = `<p>${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.no.body.context.in.this.window')}</p>`
          + renderBodyPanelFooter();
        openBodyPanelSurface();
        return;
      }

      const briefRows = bodyWindowParts(payload, 10)
        .map(row => `<li>${escapeHtml(row)}</li>`)
        .join('\n');
      const briefExtra = (payload.signals || []).length - 10;
      const briefOverflow = briefExtra > 0
        ? `<li class="tr-body-panel-more"><a href="/app/body/${day}">${window.JournalI18n.renderHtml('journal.window.transcripts.body.panel.more.count', [['quantity', window.JournalI18n.cardinalQuantity(briefExtra)], ['count', String(briefExtra)]])}</a></li>`
        : '';
      const sources = (payload.sources?.chips || [])
        .map(source => `<span class="tr-signal-chip">${escapeHtml(source.name)} · ${escapeHtml(bodyCountText(source))}</span>`)
        .join('\n');
      const events = (payload.events || [])
        .map(event => `<li>${escapeHtml(event.label)} · ${escapeHtml(event.start_label)}–${escapeHtml(event.end_label)}</li>`)
        .join('\n');
      const rhythm = renderBodyRhythm(payload, { rows: true, limit: 24, title: false });

      bodyPanelBody.innerHTML = ''
        + renderBodyPanelSection(window.JournalI18n.render('journal.window.transcripts.body.section.brief'), `<ul class="tr-body-panel-list">${briefRows || `<li>${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.body.data.present')}</li>`}${briefOverflow}</ul>`)
        + renderBodyPanelSection(window.JournalI18n.render('journal.window.transcripts.visible.text.day.rhythm'), rhythm)
        + renderBodyPanelSection(window.JournalI18n.render('journal.window.transcripts.body.section.events'), events ? `<ul class="tr-body-panel-list">${events}</ul>` : '')
        + renderBodyPanelSection(window.JournalI18n.render('journal.window.transcripts.body.section.sources'), sources ? `<div class="tr-body-chip-row">${sources}</div>` : '')
        + renderBodyPanelFooter();
      attachBodyWindowButtons(bodyPanelBody);
      openBodyPanelSurface();
    }

    function closeBodyPanel() {
      bodyPanel.classList.remove('visible');
      bodyPanel.setAttribute('aria-hidden', 'true');
      if (bodyPanelTrigger && document.contains(bodyPanelTrigger)) {
        bodyPanelTrigger.focus();
      }
      bodyPanelTrigger = null;
    }

    bodyPanelClose.addEventListener('click', closeBodyPanel);
    bodyPanelClose.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); closeBodyPanel(); }
    });

    function openBodyWindow(payload, title, rangeLabel) {
      renderBodyPanel(payload, title, rangeLabel);
    }

    async function openBodyWindowFromIso(fromIso, toIso, title, rangeLabel) {
      bodyPanelTitle.textContent = title || window.JournalI18n.render('journal.window.transcripts.visible.text.body.context');
      bodyPanelRange.textContent = rangeLabel || '';
      bodyPanelBody.innerHTML = window.SurfaceState.loading({ text: window.JournalI18n.render('journal.window.transcripts.text.loading.body.context'), busy: false });
      openBodyPanelSurface();
      try {
        const payload = await fetchBodyWindow(fromIso, toIso);
        renderBodyPanel(payload, title, rangeLabel);
      } catch (_) {
        bodyPanelBody.innerHTML = window.SurfaceState.error({
          heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.couldn.t.load.body.context'),
          desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.try.again.in.a.moment')
        });
      }
    }

    // Sleep/workout event counts ("1 sleep session") for windows whose
    // compact summary is empty but which still hold body events.
    function bodyEventParts(payload) {
      const events = payload?.events || [];
      const counts = { sleep: 0, workout: 0 };
      events.forEach(event => {
        if (event.kind === 'sleep') counts.sleep += 1;
        else if (event.kind === 'workout') counts.workout += 1;
      });
      const parts = [];
      if (counts.sleep) {
        parts.push(window.JournalI18n.render('journal.window.transcripts.body.sleep.count', transcriptCount('sessions', counts.sleep)));
      }
      if (counts.workout) {
        parts.push(window.JournalI18n.render('journal.window.transcripts.body.workout.count', transcriptCount('workouts', counts.workout)));
      }
      return parts;
    }

    function renderBodyStrip(payload) {
      let parts = bodyWindowParts(payload, 4);
      if (parts.length === 0 && payload?.has_data) {
        parts = bodyEventParts(payload).slice(0, 4);
      }
      if (parts.length === 0) return '';
      return '<div class="tr-body-strip">'
        + `<span class="tr-body-strip-label">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.body.context')}</span>`
        + '\n'
        + parts.map(part => `<span>${escapeHtml(part)}</span>`).join('<span class="tr-body-strip-separator" aria-hidden="true"> · </span>')
        + '\n'
        + `<button type="button" class="tr-body-strip-button" data-action="open-body-window">${window.JournalI18n.renderHtml('journal.window.transcripts.button.label.open.details')}</button>`
        + '</div>';
    }

    // The owner hid this day's body data: the card says so, and keeps only the
    // way to the Body page, where its unhide is.
    const BODY_HIDDEN_COPY = { hidden: window.JournalI18n.render('journal.window.transcripts.body.hidden.copy.hidden.you.hid.this.day.s.body') };

    function renderBodyContextCard(data) {
      const payload = data.body_window;
      if (payload && payload.hidden === true) {
        return '<div class="tr-body-context-card">'
          + `<h3>${window.JournalI18n.renderHtml('journal.window.transcripts.section.heading.body.context')}</h3>`
          + '\n'
          + `<p data-copy-key="body-hidden">${escapeHtml(BODY_HIDDEN_COPY.hidden)}</p>`
          + '\n'
          + '<div class="tr-body-context-actions">'
          + `<a class="tr-body-link" href="/app/body/${day}">${window.JournalI18n.renderHtml('journal.window.transcripts.link.label.open.full.body.day')}</a>`
          + '</div>'
          + '</div>';
      }
      const parts = bodyWindowParts(payload, 6);
      let summary = window.JournalI18n.render('journal.window.transcripts.window.explanation.body.context.unavailable.for.this.day');
      if (payload) {
        summary = payload.has_data
          ? window.JournalI18n.render('journal.window.transcripts.window.explanation.body.data.present.for.this.day')
          : window.JournalI18n.render('journal.window.transcripts.window.explanation.no.compact.body.context.in.this');
      }
      const facts = parts.length
        ? '<div class="tr-body-context-facts">'
          + parts.map(part => `<span class="tr-body-context-fact">${escapeHtml(part)}</span>`).join('\n')
          + '</div>'
        : `<p>${escapeHtml(summary)}</p>`;
      const rhythm = payload ? renderBodyRhythm(payload) : '';
      const detailsButton = payload
        ? `<button type="button" class="tr-body-context-button" data-action="open-body-window">${window.JournalI18n.renderHtml('journal.window.transcripts.button.label.open.details')}</button>`
        : '';
      return '<div class="tr-body-context-card">'
        + `<h3>${window.JournalI18n.renderHtml('journal.window.transcripts.section.heading.body.context')}</h3>`
        + '\n'
        + facts
        + rhythm
        + '\n'
        + '<div class="tr-body-context-actions">'
        + detailsButton
        + `<a class="tr-body-link" href="/app/body/${day}">${window.JournalI18n.renderHtml('journal.window.transcripts.link.label.open.full.body.day')}</a>`
        + '</div>'
        + '</div>';
    }

    function attachBodyWindowButtons(targetEl) {
      targetEl.querySelectorAll('[data-action="open-body-window"]').forEach(button => {
        button.addEventListener('click', () => {
          const payload = segmentData?.body_window;
          if (payload) {
            const title = isBodySummarySegment() ? window.JournalI18n.render('journal.window.transcripts.body.context') : window.JournalI18n.render('journal.window.transcripts.window.explanation.body.during.this.segment');
            const label = isBodySummarySegment()
              ? window.JournalI18n.render('journal.window.transcripts.body.full.day')
              : `${selectedSegment?.start || ''}–${selectedSegment?.end || ''}`;
            openBodyWindow(payload, title, label);
          }
        });
      });
      targetEl.querySelectorAll('[data-action="open-body-hour"]').forEach(button => {
        button.addEventListener('click', () => {
          if (!button.dataset.from || !button.dataset.to) return;
          openBodyWindowFromIso(
            button.dataset.from,
            button.dataset.to,
            window.JournalI18n.render('journal.window.transcripts.window.explanation.body.during.this.hour'),
            button.dataset.range || ''
          );
        });
      });
    }

    // Format bytes as human-readable size (KB, MB, GB)
    function formatSize(bytes) {
      if (!bytes) return '';
      if (bytes < 1024) return bytes + ' B';
      if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(0) + ' KB';
      if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB';
      return (bytes / (1024 * 1024 * 1024)).toFixed(2) + ' GB';
    }

    // seg.stream is a raw internal identifier (e.g. "device_watch-audio",
    // "iphone.mobile") — never show it verbatim in owner-facing text.
    function humanizeStreamName(stream) {
      if (!stream) return '';
      return window.JournalFormat.stream(stream);
    }

    const segmentLabels = new Map([
      ['audio', window.JournalI18n.render('journal.window.transcripts.visible.text.audio')],
      ['screen', window.JournalI18n.render('journal.window.transcripts.visible.text.screen')],
      ['import', window.JournalI18n.render('journal.window.transcripts.visible.text.import')],
      ['pages', window.JournalI18n.render('journal.window.transcripts.kind.pages')],
      ['location', window.JournalI18n.render('journal.window.transcripts.kind.location')],
      ['photo', PHOTO_COPY.label],
      ['desktop activity', window.JournalI18n.render('journal.window.transcripts.kind.desktop.activity')],
      ['audio + screen', window.JournalI18n.render('journal.window.transcripts.kind.audio.plus.screen')]
    ]);

    function segmentTypeLabel(kind) {
      return kind === screenshotStreamKind ? SCREENSHOT_COPY.J4 : (segmentLabels.get(kind) ?? kind);
    }

    // Don't say the type twice: omit a content type the stream's label already
    // says, either as its what word ("screen" beside a screen label) or as an
    // import. "audio + screen" beside a screen label keeps the type, since the
    // label says only half of it.
    function shouldOmitStreamType(stream, typeDesc) {
      if (!stream) return false;
      const says = window.JournalFormat.streamSays(stream);
      if (says.imported) return true;
      return Boolean(says.what) && says.what === String(typeDesc || '').trim().toLowerCase();
    }

    // A saved conversation says what it is: spoken words (transcripts),
    // written ones (messages), or a conversation the journal can't place.
    // The journal decides that per file; the stream name can't.
    function conversationItem(seg) {
      const items = Array.isArray(seg?.conversation_items) ? seg.conversation_items : [];
      return items[0] || null;
    }

    function conversationKind(seg) {
      const item = conversationItem(seg);
      return item ? window.JournalFormat.conversationKind(item) : null;
    }

    function isWrittenOrUnplaced(seg) {
      const kind = conversationKind(seg);
      return kind === 'messages' || kind === 'conversation';
    }

    // Spoken transcripts keep the registry's label. Messages and a
    // conversation the journal can't place take their own word.
    function conversationLabel(seg) {
      return isWrittenOrUnplaced(seg) ? window.JournalFormat.conversationItem(conversationItem(seg)) : '';
    }

    // When a written item was sent, from the saved source. An item with no
    // send time is listed by its date, like a note: the time it was filed is
    // never shown as a send time.
    // An instant as this page writes times: the journal's clock, HH:MM.
    function clockOf(instant) {
      const when = new Date(instant);
      if (Number.isNaN(when.getTime())) return '';
      if (window.JournalClock) {
        const p = window.JournalClock.parts(when);
        return `${p.hour}:${p.minute}`;
      }
      return `${String(when.getHours()).padStart(2, '0')}:${String(when.getMinutes()).padStart(2, '0')}`;
    }

    function conversationWhen(seg) {
      if (!isWrittenOrUnplaced(seg)) return null;
      const item = conversationItem(seg);
      if (item.untimed) return null;
      if (item.first_sent_at) {
        const first = clockOf(item.first_sent_at);
        const last = item.last_sent_at ? clockOf(item.last_sent_at) : first;
        return first === last ? first : `${first} – ${last}`;
      }
      if (item.shared_at) return clockOf(item.shared_at);
      return null;
    }

    function importItemLabel(seg) {
      const label = conversationLabel(seg);
      if (label) return label;
      return seg?.item ? window.JournalFormat.importItem(seg.item) : '';
    }

    function isDateOnlyImport(seg) {
      if (isWrittenOrUnplaced(seg)) return conversationWhen(seg) === null;
      return Boolean(seg?.item) && window.JournalFormat.isDateOnlyImport(seg.item);
    }

    function importItemWhen(seg) {
      if (isDateOnlyImport(seg)) return window.JournalFormat.filedDay(day);
      const sent = conversationWhen(seg);
      if (sent) return sent;
      return isMomentSegment(seg) ? seg.start : `${seg.start} – ${seg.end}`;
    }

    function datedImportLabel(label, date) {
      return window.JournalI18n.render('journal.window.shared.import.item.dated.label', [
        ['label', label], ['date', date]
      ]);
    }

    // Build range text with total media size
    // A segment's range is its recording device's own wall clock. When that
    // device reported a zone other than the one this journal shows times in,
    // name it beside the range so the times read as what they are.
    function segmentRangeLabel(seg) {
      if (seg.locationRow) return seg.start;
      if (isDateOnlyImport(seg)) return window.JournalFormat.filedDay(day);
      const sent = conversationWhen(seg);
      if (sent) return sent;
      // A screenshot or photo is a moment, so its time alone.
      const range = isMomentSegment(seg) ? seg.start : `${seg.start} - ${seg.end}`;
      if (!selectedSegmentZone?.differs || !selectedSegmentZone.label) return range;
      const note = window.JournalClock
        ? window.JournalClock.noteText(selectedSegmentZone.label)
        : '';
      return note ? `${range} · ${note}` : range;
    }

    function updateRangeText() {
      if (!selectedSegment || !segmentData) return;
      const seg = selectedSegment;
      if (isDateOnlyImport(seg)) {
        rangeText.textContent = segmentRangeLabel(seg);
        return;
      }
      let parts = [escapeHtml(segmentRangeLabel(seg))];


      const sizes = segmentData.media_sizes;
      if (sizes) {
        let total = 0;
        const breakdown = [];
        if (sizes.audio) {
          total += sizes.audio;
          breakdown.push(window.JournalI18n.render('journal.window.transcripts.media.size.audio', [['size', formatSize(sizes.audio)]]));
        }
        if (sizes.screen) {
          total += sizes.screen;
          breakdown.push(window.JournalI18n.render('journal.window.transcripts.media.size.screen', [['size', formatSize(sizes.screen)]]));
        }
        if (sizes.image) {
          total += sizes.image;
          breakdown.push(window.JournalI18n.render('journal.window.transcripts.media.size.photo', [['size', formatSize(sizes.image)]]));
        }
        if (total) {
          const title = breakdown.join(', ');
          parts.push(`<span title="${escapeHtml(title)}">${formatSize(total)}</span>`);
        }
      }

      rangeText.innerHTML = parts.join(' · ');
    }

    function computeTimelineBounds(ranges) {
      // Compute dynamic timeline bounds from content ranges
      // Returns {start, end} in minutes, snapped to hours
      const allRanges = [...(ranges.audio || []), ...(ranges.screen || []), ...(ranges.segments || [])];

      if (allRanges.length === 0) {
        return { start: DEFAULT_START, end: DEFAULT_END };
      }

      // Find min/max times across all ranges
      let minTime = Infinity;
      let maxTime = -Infinity;
      for (const rg of allRanges) {
        const s = parseTime(rg.start);
        const e = parseTime(rg.end);
        if (s < minTime) minTime = s;
        if (e > maxTime) maxTime = e;
      }

      // Add buffer and snap to hours
      let start = Math.floor((minTime - BUFFER) / 60) * 60;
      let end = Math.ceil((maxTime + BUFFER) / 60) * 60;

      // Enforce minimum span - extend end if needed
      if (end - start < MIN_SPAN) {
        end = start + MIN_SPAN;
      }

      // Clamp to valid day range (0:00 - 24:00)
      start = Math.max(0, start);
      end = Math.min(24 * 60, end);

      return { start, end };
    }

    function markdownOnlySegments(segments) {
      return (segments || []).filter(seg => {
        const types = seg.types || [];
        return !isDateOnlyImport(seg) && types.includes('markdown') && !types.includes('audio') && !types.includes('screen');
      });
    }

    // Messages and a conversation the journal can't place, at their send
    // time. One with no send time is listed by date and not drawn here.
    function writtenSegments(segments) {
      return (segments || []).filter(seg => {
        const types = seg.types || [];
        return isWrittenOrUnplaced(seg) && !isDateOnlyImport(seg) && !types.includes('markdown') && !types.includes('audio') && !types.includes('screen');
      });
    }

    function browserOnlySegments(segments) {
      return (segments || []).filter(seg => {
        const types = seg.types || [];
        return types.includes('browser') && !types.includes('audio') && !types.includes('screen');
      });
    }

    function desktopActivityOnlySegments(segments) {
      return (segments || []).filter(seg => {
        const types = seg.types || [];
        return types.includes('desktop_activity') && !types.includes('audio') && !types.includes('screen');
      });
    }

    function imageOnlySegments(segments) {
      return (segments || []).filter(seg => {
        const types = seg.types || [];
        return types.includes('image') && !types.includes('audio') && !types.includes('screen');
      });
    }

    function addSegmentIndicator(type, startMin, endMin, column, streams = [], state = 'analyzed', think = null, segment = null) {
      const el = document.createElement('div');
      el.className = 'tr-seg ' + (
        type === 'screen' || type === 'desktop_activity'
          ? 'tr-seg-screen'
          : type === 'image'
            ? 'tr-seg-image'
            : type === 'markdown'
              ? 'tr-seg-markdown'
              : type === 'browser'
                ? 'tr-seg-browser'
                : 'tr-seg-audio'
      );
      if (state === 'pending') {
        el.classList.add('tr-seg-pending');
      } else if (state === 'analyzing') {
        el.classList.add('tr-seg-analyzing');
      } else if (think === 'awaiting') {
        el.classList.add('tr-seg-awaiting');
      }
      el.dataset.startMinute = startMin;
      el.dataset.endMinute = endMin;
      el.style.top = y(startMin) + 'px';
      const minHeight = type === 'image' ? 5 : 2;
      el.style.height = Math.max(minHeight, y(endMin) - y(startMin)) + 'px';
      el.classList.add(column === 1 ? 'tr-seg-col-1' : 'tr-seg-col-0');
      // Zero-padded HH:MM label
      const _h = Math.floor(startMin / 60);
      const _m = startMin % 60;
      const time = `${String(_h).padStart(2, '0')}:${String(_m).padStart(2, '0')}`;
      const _label = window.JournalI18n.render('journal.window.transcripts.timeline.segment.start', [['time', time]]);
      el.setAttribute('role', 'button');
      el.setAttribute('tabindex', '0');
      el.setAttribute('aria-label', _label);
      const sortedStreams = [...streams].sort();
      const humanizedStreams = sortedStreams.map(humanizeStreamName);
      const omitType = sortedStreams.length > 0 && sortedStreams.every(s => shouldOmitStreamType(s, type));
      const typeDisplay = segmentTypeLabel(type === 'desktop_activity' ? 'desktop activity' : type === 'image' ? 'photo' : type);
      const typePart = omitType ? '' : ', ' + typeDisplay;
      const labelPart = humanizedStreams.length ? ', ' + humanizedStreams.join(', ') : '';
      const rawSuffix = sortedStreams.length ? ' (' + sortedStreams.join(', ') + ')' : '';
      const stateSuffix = state === 'pending' ? ' · ' + window.JournalI18n.render('journal.window.transcripts.analysis.state.pending') : state === 'analyzing' ? ' · ' + window.JournalI18n.render('journal.window.transcripts.analysis.state.analyzing') : (think === 'awaiting' ? ' · ' + AWAITING_THINKING_LABEL : '');
      const isZeroLengthMoment = startMin === endMin && type === 'image';
      el.title = isZeroLengthMoment
        ? hhmm(startMin) + ' (' + typeDisplay + labelPart + stateSuffix + ')' + rawSuffix
        : hhmm(startMin) + ' – ' + hhmm(endMin) + ' (' + window.JournalI18n.render('journal.window.transcripts.timeline.duration.minute', [['quantity', window.JournalI18n.cardinalQuantity(endMin - startMin)], ['minutes', String(endMin - startMin)]]) + typePart + labelPart + stateSuffix + ')' + rawSuffix;
      const activate = () => {
        if (segment) {
          selectSegment(segment);
          return;
        }
        const midMin = (startMin + endMin) / 2;
        const start = snap(midMin - DEFAULT_LEN / 2);
        range = { start, end: start + DEFAULT_LEN };
        renderTimeline();
        updateZoom();
        // Picking a stretch on the DAY column narrows the DETAIL rail but
        // does not itself select a block — acknowledge step one is done so
        // the instructions don't read as if the click had no effect.
        if (!selectedSegment) {
          refreshUnselectedLocation(window.SurfaceState.empty({
            icon: window.ConveyIcons.svg('clock'),
            heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.your.day.at.a.glance'),
            desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.now.pick.a.block.in.the')
          }));
        }
      };
      el.addEventListener('click', e => {
        e.stopPropagation();
        activate();
      });
      el.addEventListener('keydown', e => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          activate();
        }
      });
      segmentsLane.appendChild(el);
      return el;
    }

    function renderBodyEvents(payload) {
      bodyEventsLane.innerHTML = '';
      if (!payload || !payload.has_data || !(payload.events || []).length) {
        return;
      }
      (payload.events || []).forEach(event => {
        const startMin = minuteForIso(event.start);
        const endMin = minuteForIso(event.end);
        const visStart = Math.max(startMin, timelineStart);
        const visEnd = Math.min(endMin, timelineEnd);
        if (visEnd <= visStart) {
          return;
        }
        const block = document.createElement('div');
        block.className = 'tr-body-event tr-body-event-' + (event.kind === 'sleep' ? 'sleep' : 'workout');
        block.dataset.startMinute = visStart;
        block.dataset.endMinute = visEnd;
        block.style.top = y(visStart) + 'px';
        block.style.height = Math.max(4, y(visEnd) - y(visStart)) + 'px';
        block.setAttribute('role', 'button');
        block.setAttribute('tabindex', '0');
        const title = `${event.label} · ${event.start_label}–${event.end_label}`;
        block.setAttribute('aria-label', title);
        block.title = title;
        const open = () => openBodyWindowFromIso(event.start, event.end, window.JournalI18n.render('journal.window.transcripts.body.context'), title);
        block.addEventListener('click', open);
        block.addEventListener('keydown', e => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            open();
          }
        });
        bodyEventsLane.appendChild(block);
      });
    }

    function buildGrid() {
      grid.innerHTML = '';
      labels.innerHTML = '';

      for (let h = timelineStart / 60; h <= timelineEnd / 60; h++) {
        const hourLine = document.createElement('div');
        hourLine.className = 'tr-grid-hour';
        hourLine.style.top = y(h * 60) + 'px';
        grid.appendChild(hourLine);

        const lab = document.createElement('div');
        lab.className = 'tr-label';
        lab.style.top = y(h * 60) + 'px';
        lab.textContent = clockLabel(h * 60);
        labels.appendChild(lab);

        if (h < timelineEnd / 60) {
          [15, 30, 45].forEach(m => {
            const q = document.createElement('div');
            q.className = 'tr-grid-quarter';
            q.style.top = y(h * 60 + m) + 'px';
            grid.appendChild(q);
          });
        }
      }
    }

    function renderTimeline() {
      selWrap.style.top = (HEADER_HEIGHT + y(range.start)) + 'px';
      selWrap.style.height = (y(range.end) - y(range.start)) + 'px';
    }

    function buildZoomGrid() {
      zoomGrid.innerHTML = '';
      zoomLabels.innerHTML = '';

      const rangeLen = range.end - range.start;
      // Determine label interval based on range length
      let labelInterval = 5; // default 5 min
      if (rangeLen > 120) labelInterval = 15;
      else if (rangeLen > 60) labelInterval = 10;

      for (let m = range.start; m <= range.end; m++) {
        const yPos = zoomY(m);

        if (m % 60 === 0) {
          const line = document.createElement('div');
          line.className = 'tr-grid-hour';
          line.style.top = yPos + 'px';
          zoomGrid.appendChild(line);
        } else if (m % 15 === 0) {
          const line = document.createElement('div');
          line.className = 'tr-grid-quarter';
          line.style.top = yPos + 'px';
          zoomGrid.appendChild(line);
        }

        if (m % labelInterval === 0) {
          const lab = document.createElement('div');
          lab.className = 'tr-zoom-label';
          lab.style.top = yPos + 'px';
          lab.textContent = clockLabel(m);
          zoomLabels.appendChild(lab);
        }
      }
    }

    function filterSegmentsInRange() {
      return allSegments.filter(seg => {
        const segStart = parseTime(seg.start);
        const segEnd = parseTime(seg.end);
        // Zero-duration moments (e.g. a shared photo at 14:30) still belong to
        // a window starting at 14:30.
        if (segStart === segEnd) {
          return segStart >= range.start && segStart < range.end;
        }
        return segEnd > range.start && segStart < range.end;
      });
    }

    // What a pill on the detail rail is drawn as. The day rail's blocks and
    // the key above the rails use the same names, so a kind has one colour
    // wherever it is drawn.
    function pillKind(seg) {
      const types = seg.types || [];
      const hasAudio = types.includes('audio');
      const hasScreen = types.includes('screen');
      if (hasAudio && hasScreen) return 'both';
      if (hasAudio) return 'audio';
      if (types.includes('markdown') || (isWrittenOrUnplaced(seg) && !hasScreen)) return 'markdown';
      if (types.includes('browser')) return 'browser';
      if (types.includes('image') && !hasScreen) return 'image';
      return 'screen';
    }

    // Each stream column on the detail rail is headed by its device's label.
    // The label can be cut short, so its whole text and the stream's own name
    // stay on hover.
    function renderStreamHeads(streams) {
      if (!zoomStreams) return;
      zoomStreams.innerHTML = '';
      const colWidth = 100 / Math.max(1, streams.length);
      streams.forEach((stream, idx) => {
        const head = document.createElement('div');
        head.className = 'tr-zoom-stream';
        const label = humanizeStreamName(stream);
        head.textContent = label;
        head.title = label + ' (' + stream + ')';
        head.dataset.streamHead = stream;
        if (streams.length > 1) {
          head.style.left = (idx * colWidth) + '%';
          head.style.right = 'auto';
          head.style.width = colWidth + '%';
        }
        zoomStreams.appendChild(head);
      });
    }

    function buildZoomSegments() {
      const filtered = filterSegmentsInRange();
      const timed = filtered.filter(seg => !isDateOnlyImport(seg));
      const dateOnly = allSegments.filter(isDateOnlyImport);
      const choices = [...dateOnly, ...timed];
      const streams = [...new Set(timed.map(s => s.stream))].sort();
      const colCount = streams.length;
      // The rail's width follows the day's streams, not the range's, so moving
      // the range never reflows the reading pane.
      if (railCard) {
        const dayStreams = new Set(allSegments.filter(seg => !isDateOnlyImport(seg)).map(seg => seg.stream));
        railCard.dataset.zoomStreams = String(Math.min(4, dayStreams.size));
      }
      renderStreamHeads(streams);
      zoomSegments.innerHTML = '';
      segmentPicker.innerHTML = `<option value="">${window.JournalI18n.renderHtml('journal.window.transcripts.option.label.choose.a.segment')}</option>`;
      segmentPicker.disabled = choices.length === 0;
      if (segmentPickerLabel) {
        const labelId = dateOnly.length
          ? 'journal.window.transcripts.accessible.name.items'
          : 'journal.window.transcripts.field.label.segments.in.this.range';
        segmentPickerLabel.textContent = window.JournalI18n.render(labelId);
      }
      choices.forEach(seg => {
        const option = document.createElement('option');
        option.value = segmentChoiceId(seg);
        const itemLabel = importItemLabel(seg);
        if (itemLabel) option.textContent = datedImportLabel(itemLabel, importItemWhen(seg));
        else option.textContent = `${isMomentSegment(seg) ? seg.start : `${seg.start} – ${seg.end}`} · ${humanizeStreamName(seg.stream)}`;
        option.title = seg.stream;
        segmentPicker.appendChild(option);
      });
      segmentPicker.value = selectedSegment && !selectedSegment.locationRow ? segmentChoiceId(selectedSegment) : '';
      renderSegPick(filtered);

      if (timed.length === 0) {
        zoomSegments.innerHTML = `<div class="tr-zoom-empty">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.nothing.in.this.range')}</div>`;
        return;
      }

      // Same-stream moment pills (e.g. burst photos taken in the same minute)
      // subdivide their stream column so none sit unreachable underneath another.
      const momentSlots = new Map();
      for (const stream of streams) {
        const moments = timed
          .filter(seg => seg.stream === stream && isMomentSegment(seg))
          .map(seg => ({ seg, top: zoomY(Math.max(parseTime(seg.start), range.start)) }))
          .sort((a, b) => a.top - b.top || String(a.seg.key).localeCompare(String(b.seg.key)));
        let i = 0;
        while (i < moments.length) {
          let j = i + 1;
          let maxBottom = moments[i].top + 20;
          while (j < moments.length && moments[j].top < maxBottom) {
            maxBottom = Math.max(maxBottom, moments[j].top + 20);
            j += 1;
          }
          const count = j - i;
          if (count > 1) {
            for (let k = 0; k < count; k += 1) {
              momentSlots.set(moments[i + k].seg, { idx: k, count });
            }
          }
          i = j;
        }
      }

      timed.forEach(seg => {
        const segStart = parseTime(seg.start);
        const segEnd = parseTime(seg.end);

        // Clamp to visible range
        const visStart = Math.max(segStart, range.start);
        const visEnd = Math.min(segEnd, range.end);

        const pill = document.createElement('div');
        pill.className = 'tr-zoom-pill';
        pill.setAttribute('role', 'button');
        pill.setAttribute('tabindex', '0');

        const slot = momentSlots.get(seg);
        if (colCount > 1 || slot) {
          const colIdx = streams.indexOf(seg.stream);
          const colWidth = 100 / colCount;
          const subIdx = slot ? slot.idx : 0;
          const subCount = slot ? slot.count : 1;
          const subWidth = colWidth / subCount;
          const gap = 0.5;
          pill.style.left = (colIdx * colWidth + subIdx * subWidth + gap / 2) + '%';
          pill.style.right = 'auto';
          pill.style.width = (subWidth - gap) + '%';
        }

        // Determine pill type based on content
        const hasAudio = seg.types.includes('audio');
        const hasScreen = seg.types.includes('screen');
        const hasMarkdown = seg.types.includes('markdown');
        const hasBrowser = seg.types.includes('browser');
        const hasImage = seg.types.includes('image');
        const isWritten = isWrittenOrUnplaced(seg) && !hasAudio && !hasScreen;
        const isPhotoOnly = hasImage && !hasAudio && !hasScreen && !hasMarkdown && !hasBrowser;
        const isShotOnly = seg.item?.kind === 'screen_text' || isScreenshotsStream(seg.stream);
        const isActivityOnly = seg.types.includes('desktop_activity') && !hasAudio && !hasScreen && !hasMarkdown && !hasBrowser && !hasImage;
        const dataState = seg.data_state || {};
        const hasPendingAdvertised = (seg.types || []).some(
          type => dataState[type] && dataState[type] !== 'analyzed'
        );
        const hasAnalyzingAdvertised = (seg.types || []).some(
          type => dataState[type] === 'analyzing'
        );
        pill.classList.add('tr-zoom-pill-' + pillKind(seg));
        if (hasAnalyzingAdvertised) {
          pill.classList.add('tr-zoom-pill-analyzing');
        } else if (hasPendingAdvertised) {
          pill.classList.add('tr-zoom-pill-pending');
        } else if (seg.think === 'awaiting') {
          pill.classList.add('tr-zoom-pill-awaiting');
        }
        const isMoment = isShotOnly || isPhotoOnly;
        if (isShotOnly) {
          pill.classList.add('tr-zoom-pill-shot');
          pill.innerHTML = SCREENSHOT_GLYPH;
        } else if (isPhotoOnly) {
          pill.classList.add('tr-zoom-pill-photo');
          pill.innerHTML = PHOTO_GLYPH;
        }

        // In select mode the ring marks what is chosen, not what is open.
        if (segPick.on) {
          const picked = segPick.chosen.has(segmentChoiceId(seg));
          pill.classList.toggle('tr-active', picked);
          pill.classList.toggle('tr-picked', picked);
        } else if (selectedSegment && selectedSegment.key === seg.key && selectedSegment.stream === seg.stream) {
          pill.classList.add('tr-active');
        }

        pill.style.top = zoomY(visStart) + 'px';
        pill.style.height = Math.max(isMoment ? 18 : 4, zoomY(visEnd) - zoomY(visStart)) + 'px';
        const duration = Math.round(visEnd - visStart);
        const typeDesc = isShotOnly ? screenshotStreamKind : (hasAudio && hasScreen) ? 'audio + screen' : hasAudio ? 'audio' : (hasMarkdown || isWritten) ? 'import' : hasBrowser ? 'pages' : isPhotoOnly ? 'photo' : isActivityOnly ? 'desktop activity' : 'screen';
        const typeLabel = hasAudio && hasScreen && !isShotOnly
          ? window.JournalI18n.render('journal.window.transcripts.kind.audio.and.screen') : segmentTypeLabel(typeDesc);
        const segmentStatusLabel = hasAnalyzingAdvertised ? window.JournalI18n.render('journal.window.transcripts.analysis.state.analyzing') : hasPendingAdvertised ? window.JournalI18n.render('journal.window.transcripts.analysis.state.pending') : seg.think === 'awaiting' ? AWAITING_THINKING_LABEL : '';
        const statusSuffix = segmentStatusLabel ? ' · ' + segmentStatusLabel : '';
        const ariaStatusSuffix = segmentStatusLabel ? ', ' + segmentStatusLabel : '';
        const itemLabel = importItemLabel(seg);
        const label = itemLabel || humanizeStreamName(seg.stream);
        const omitType = Boolean(itemLabel) || shouldOmitStreamType(seg.stream, typeDesc);
        const typeTitlePart = omitType ? '' : ' · ' + segmentTypeLabel(typeDesc);
        const typeAriaPart = omitType ? '' : ', ' + typeLabel;
        // A screenshot or photo is a moment, not a stretch: its time alone, no length.
        const titleSpan = isMoment ? seg.start : seg.start + ' – ' + seg.end + ' · ' + window.JournalI18n.render('journal.window.shared.date.format.duration.minute.count', [['quantity', window.JournalI18n.cardinalQuantity(duration)], ['minutes', String(duration)]]);
        const ariaSpan = isMoment ? seg.start : seg.start + ' \u2013 ' + seg.end;
        pill.title = itemLabel ? datedImportLabel(itemLabel, titleSpan) + statusSuffix :
          titleSpan + typeTitlePart + ' · ' + label + statusSuffix + ' (' + seg.stream + ')';
        if (itemLabel) pill.setAttribute('aria-label', datedImportLabel(itemLabel, ariaSpan) + ariaStatusSuffix);
        else pill.setAttribute('aria-label', window.JournalI18n.render('journal.window.transcripts.timeline.segment.accessible', [['details', ariaSpan + typeAriaPart + ', ' + label + ariaStatusSuffix]]));
        pill.dataset.key = seg.key;
        pill.dataset.stream = seg.stream;
        pill.dataset.showKind = showKind(seg);
        if (dayShowsSwitch() && showFilter !== 'everything' && !shownUnder(seg)) pill.classList.add('tr-show-dim');

        pill.addEventListener('click', () => (segPick.on ? toggleSegPick(seg) : selectSegment(seg)));
        pill.addEventListener('keydown', e => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            if (segPick.on) toggleSegPick(seg);
            else selectSegment(seg);
          }
        });
        zoomSegments.appendChild(pill);
      });
    }

    // A link to one second of a recording: `#<key>/<tab>/<HHMMSS>`, the clock
    // its line shows. The moment stays in the address while that recording is
    // open, and its line is cued once, without playing.
    let linkedMoment = null;

    function hashLocation() {
      const [segKey, tab, moment] = window.location.hash.slice(1).split('/', 3);
      return { segKey, tab, moment: /^\d{6}$/.test(moment || '') ? moment : null };
    }

    function linkMoment(seg, clock) {
      linkedMoment = clock ? { key: seg.key, stream: seg.stream, clock, cued: false } : null;
    }

    function momentFor(seg) {
      return linkedMoment && linkedMoment.key === seg.key && linkedMoment.stream === seg.stream
        ? linkedMoment
        : null;
    }

    function segmentLocation(seg, tab) {
      const url = new URL(window.location.href);
      url.searchParams.set('stream', seg.stream);
      const moment = momentFor(seg);
      url.hash = moment ? `${seg.key}/${tab}/${moment.clock}` : `${seg.key}/${tab}`;
      return url.pathname + url.search + url.hash;
    }

    function linkedSegment(key) {
      const stream = new URLSearchParams(window.location.search).get('stream');
      return allSegments.find(segment => segment.key === key && (!stream || segment.stream === stream));
    }

    function selectSegment(seg, updateHash = true, requestedTabId = null) {
      const previous = selectedSegment;
      selectedSegment = seg;
      segmentPicker.value = segmentChoiceId(seg);

      // Keep the detail zoom rail describing the selected segment: when the
      // segment falls outside the current range (e.g. a 00:00 import selected
      // while the rail shows midday), recenter the range to contain it.
      const segStart = parseTime(seg.start);
      const segEnd = parseTime(seg.end);
      if (!isDateOnlyImport(seg) && (segStart < range.start || segEnd > range.end)) {
        const rangeLen = range.end - range.start;
        const segMid = (segStart + segEnd) / 2;
        let newStart = snap(Math.max(timelineStart, segMid - rangeLen / 2));
        newStart = Math.min(newStart, timelineEnd - rangeLen);
        range = { start: newStart, end: newStart + rangeLen };
        renderTimeline();
        updateZoom();
      }

      // Choosing a segment opens another page. Choosing the same one again refines this page.
      if (updateHash) {
        const tab = activeTab || 'transcript';
        const samePair = previous
          && previous.key === seg.key
          && previous.stream === seg.stream;
        // Choosing another recording leaves the linked moment behind.
        if (!samePair) linkedMoment = null;
        const url = segmentLocation(seg, tab);
        if (samePair) ctx.history.replace(url);
        else ctx.history.push(url);
      }

      // Update active state in zoom view
      zoomSegments.querySelectorAll('.tr-zoom-pill').forEach(pill => {
        pill.classList.toggle('tr-active', pill.dataset.key === seg.key && pill.dataset.stream === seg.stream);
      });

      titleEl.textContent = importItemLabel(seg) || humanizeStreamName(seg.stream);
      titleEl.title = seg.stream;
      if (copyStreamBtn) copyStreamBtn.style.display = '';
      selectedSegmentZone = null;
      updateSegmentEntryCount(null);

      // Show delete button when segment is selected
      deleteBtn.classList.add('visible');

      // Load transcript content
      loadSegmentContent(seg, requestedTabId);
    }

    // The pane header carries the selected block's time range; when a pane knows
    // how many entries it rendered, that count sits beside the range so the size
    // of the block is legible before scrolling it.
    function updateSegmentEntryCount(count) {
      if (!selectedSegment) return;
      const rangeLabel = segmentRangeLabel(selectedSegment);
      rangeText.textContent = !isDateOnlyImport(selectedSegment) && Number.isFinite(count) && count > 0
        ? window.JournalI18n.render('journal.window.transcripts.segment.range.entries', [
          ...transcriptCount('entries', count), ['range', rangeLabel],
        ])
        : rangeLabel;
    }

    function setPaneEntryCount(targetEl, count) {
      const tabId = targetEl?.dataset?.tab;
      if (tabId) tabEntryCounts[tabId] = count;
      updateSegmentEntryCount(count);
    }

    // Step through segments with [ ] keys
    function navigateSegment(delta) {
      if (selectedSegment && selectedSegment.locationRow) return;
      if (allSegments.length === 0) return;
      let currentIdx = selectedSegment
        ? allSegments.findIndex(s => s.key === selectedSegment.key && s.stream === selectedSegment.stream)
        : -1;
      let nextIdx = currentIdx === -1
        ? (delta > 0 ? 0 : allSegments.length - 1)
        : currentIdx + delta;
      if (nextIdx < 0 || nextIdx >= allSegments.length) return;
      // selectSegment recenters the detail zoom range when needed.
      selectSegment(allSegments[nextIdx]);
    }

    // Unified timeline state
    let segmentData = null;
    let currentVideoFiles = {}; // screen jsonl filename -> video URL mapping
    let currentImageFiles = {}; // raw image filename -> image URL mapping
    let groupEntriesByIdx = new Map();
    let activeTab = null;
    let inFlightSegmentRequest = null;
    let currentSegmentAbort = null;
    let tabPanes = {}; // tabId -> pane element
    let tabEntryCounts = {}; // tabId -> entries rendered in that pane (for the header)
    let screenDecoded = false;
    let analyzePollTimer = null;
    let analyzePollStartedAt = 0;
    let analyzePollModalities = new Set();
    let knownSpeakersPromise = null;
    let knownSpeakers = [];
    let journalPeoplePromise = null;
    let journalPeople = [];
    let activeSpeakerPicker = null;

    function selectLocationSegment(seg, updateHash = true) {
      const previous = selectedSegment;
      selectedSegment = { ...seg, locationRow: true };
      segmentPicker.value = '';

      if (updateHash) {
        const tab = activeTab || 'transcript';
        const samePair = previous
          && previous.key === seg.key
          && previous.stream === seg.stream;
        // Choosing another recording leaves the linked moment behind.
        if (!samePair) linkedMoment = null;
        const url = segmentLocation(seg, tab);
        if (samePair) ctx.history.replace(url);
        else ctx.history.push(url);
      }

      zoomSegments.querySelectorAll('.tr-zoom-pill').forEach(pill => {
        pill.classList.remove('tr-active');
      });

      titleEl.textContent = importItemLabel(seg) || humanizeStreamName(seg.stream);
      titleEl.title = seg.stream;
      if (copyStreamBtn) copyStreamBtn.style.display = '';
      selectedSegmentZone = null;
      rangeText.textContent = seg.start;
      deleteBtn.classList.add('visible');

      loadSegmentContent(selectedSegment);
    }

    function loadSegmentContent(seg, requestedTabId = null) {
      currentSegmentAbort?.abort();
      stopAnalyzePoller();
      currentSegmentAbort = new AbortController();
      const signal = currentSegmentAbort.signal;
      const segmentToken = seg.key;

      // Clear old data, videos, tabs, and show loading message immediately
      segmentData = null;
      currentVideoFiles = {};
      currentImageFiles = {};
      tabPanes = {};
      tabEntryCounts = {};
      activeTab = null;
      inFlightSegmentRequest = {
        key: seg.key,
        stream: seg.stream,
        tab: requestedTabId || 'transcript',
      };
      screenDecoded = false;
      frameCapture.clear();
      tabsContainer.classList.remove('visible');
      tabsContainer.innerHTML = '';
      warningNotice.classList.remove('visible');
      panel.innerHTML = window.SurfaceState.loading({ text: window.JournalI18n.render('journal.window.transcripts.text.loading.segment'), busy: false });
      panel.classList.add('loading');

      return dayRead(`/app/transcripts/api/segment/${day}/${seg.stream}/${seg.key}`, { signal })
        .then(data => {
          const bodyPromise = isBodySummarySegment(seg)
            ? fetchDayBodyWindow(signal)
            : fetchSegmentBodyWindow(seg, signal, data.capture_zone);
          return bodyPromise
            .then(bodyWindow => ({ ...data, body_window: bodyWindow }))
            .catch(() => data);
        })
        .then(data => {
          if (!selectedSegment || selectedSegment.key !== segmentToken || selectedSegment.stream !== seg.stream) {
            return;
          }
          panel.classList.remove('loading');
          if (data && data.trash && !data.media) {
            panel.innerHTML = segmentInTrashHtml(data.trash);
            panel.querySelector('[data-open-trash]')?.addEventListener('click', goToSettingsTrash);
            return;
          }
          renderLoadedSegmentData(data, requestedTabId);
          if (!isLocationOnlySegment(data)) {
            const analyzing = currentAnalyzingModalities(data.data_state || {});
            if (analyzing.length > 0) {
              startAnalyzePoller(analyzing);
            }
          }
        })
        .catch(err => {
          if (err.name === 'AbortError') {
            return;
          }
          if (!selectedSegment || selectedSegment.key !== segmentToken || selectedSegment.stream !== seg.stream) {
            return;
          }
          panel.classList.remove('loading');
          tabsContainer.classList.remove('visible');
          tabsContainer.innerHTML = '';
          panel.innerHTML = window.SurfaceState.error({
            heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.couldn.t.load.this.segment'),
            desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.something.went.wrong.loading.the.transcript')
          });
        });
    }

    function prepareScreenFrames(data, targetEl, segmentToken, signal) {
      if (screenDecoded) {
        return Promise.resolve();
      }

      const isStaleSegment = () => !selectedSegment || selectedSegment.key !== segmentToken;
      if (isStaleSegment()) {
        return Promise.resolve();
      }

      currentVideoFiles = data.video_files || {};
      currentImageFiles = data.image_files || {};

      const nonBasicByVideo = new Map();
      (data.chunks || []).forEach(chunk => {
        if (chunk.type !== 'screen') return;
        frameCapture.noteFrameTime(getVideoUrlForChunk(chunk), chunk.source_ref?.frame_id, chunk.source_ref?.offset);
        if (chunk.basic === true) return;
        const filename = chunk.source_ref?.filename;
        const frameId = chunk.source_ref?.frame_id;
        if (!filename || !frameId) return;
        if (!nonBasicByVideo.has(filename)) {
          nonBasicByVideo.set(filename, new Set());
        }
        nonBasicByVideo.get(filename).add(frameId);
      });

      const totalFrames = Array.from(nonBasicByVideo.values()).reduce(
        (sum, frames) => sum + frames.size,
        0
      );
      const perVideoProgress = new Map();
      let lastStatusUpdate = 0;

      const updateLoadingStatus = (done) => {
        if (isStaleSegment()) return;
        const statusEl = targetEl.querySelector('[data-role="loading-status"]');
        if (!statusEl) return;
        if (!totalFrames) {
          statusEl.textContent = window.JournalI18n.render('journal.window.transcripts.visible.text.loading.screen.entries');
          return;
        }
        const decoded = Array.from(perVideoProgress.values()).reduce((sum, count) => sum + count, 0);
        const pct = Math.min(100, Math.round((decoded / totalFrames) * 100));
        statusEl.textContent = done
          ? window.JournalI18n.render('journal.window.transcripts.visible.text.rendering.screen.entries')
          : window.JournalI18n.render('journal.window.transcripts.media.frames.decoding', [...transcriptCount('decoded', decoded), ...transcriptCount('total', totalFrames), ['percent', String(pct)]]);
      };

      const makeProgressHandler = (videoUrl) => (count) => {
        if (isStaleSegment()) return;
        const now = Date.now();
        perVideoProgress.set(videoUrl, count);
        if (now - lastStatusUpdate > 150) {
          lastStatusUpdate = now;
          updateLoadingStatus(false);
        }
      };

      updateLoadingStatus(false);

      const decodeJobs = [];
      Object.entries(currentVideoFiles).forEach(([filename, url]) => {
        const frameIds = Array.from(nonBasicByVideo.get(filename) || []);
        if (frameIds.length > 0) {
          decodeJobs.push(
            frameCapture.prefetchThumbnails(
              url,
              frameIds,
              makeProgressHandler(url),
              signal
            )
          );
        }
      });

      if (decodeJobs.length === 0) {
        screenDecoded = true;
        return Promise.resolve();
      }

      return Promise.all(decodeJobs)
        .then(() => {
          if (isStaleSegment()) return;
          screenDecoded = true;
          updateLoadingStatus(true);
        })
        .catch(() => {
          if (isStaleSegment()) return;
          screenDecoded = true;
          updateLoadingStatus(true);
        });
    }

    function buildTabBar(data) {
      tabsContainer.innerHTML = '';
      tabsContainer.setAttribute('role', 'tablist');
      tabsContainer.classList.remove('visible');
      panel.innerHTML = '';
      tabPanes = {};
      tabEntryCounts = {};
      activeTab = null;
      screenDecoded = false;

      const addTab = (tabId, label) => {
        const btn = document.createElement('button');
        btn.type = 'button';
        btn.className = 'tr-tab';
        btn.dataset.tab = tabId;
        btn.textContent = label;
        btn.setAttribute('role', 'tab');
        btn.id = 'tr-tab-' + tabId;
        btn.setAttribute('aria-selected', 'false');
        btn.setAttribute('aria-controls', 'tr-tabpanel-' + tabId);
        btn.setAttribute('tabindex', '-1');
        btn.addEventListener('click', () => activateTab(tabId));
        tabsContainer.appendChild(btn);
      };

      const dataState = data.data_state || {};
      const isStandalonePhoto = isPhotoSegment(selectedSegment)
        && !dataState.audio
        && !dataState.screen
        && !dataState.browser
        && (data.signals?.events || []).length === 0
        && Object.keys(data.md_files || {}).length === 0;
      if (isStandalonePhoto) {
        tabsContainer.classList.remove('visible');
        return;
      }
      const readsAsItem = isDateOnlyImport(selectedSegment) || isWrittenOrUnplaced(selectedSegment);
      addTab('transcript', readsAsItem ? importItemLabel(selectedSegment) : window.JournalI18n.render('journal.window.transcripts.tab.transcript'));
      // A Granola meeting brings words, not sound: there is no audio to show.
      if (dataState.audio && !data.granola) {
        addTab('audio', window.JournalI18n.render('journal.window.transcripts.visible.text.audio'));
      }
      if (dataState.screen) {
        addTab('screen', window.JournalI18n.render('journal.window.transcripts.visible.text.screen'));
      }
      if (dataState.image) {
        addTab('image', window.JournalI18n.render('journal.window.transcripts.kind.photos'));
      }
      if (dataState.browser) {
        addTab('browser', window.JournalI18n.render('journal.window.transcripts.kind.pages'));
      }
      if ((data.signals?.events || []).length > 0) {
        addTab('signals', window.JournalI18n.render('journal.window.transcripts.tab.signals'));
      }

      const mdStems = Object.keys(data.md_files || {}).sort((a, b) => a.localeCompare(b));
      mdStems.forEach(stem => {
        const label = stem === 'sense' && data.sense_display
          ? window.JournalI18n.render('journal.window.transcripts.tab.names')
          : stem === 'screen_text' ? window.JournalI18n.render('journal.window.transcripts.tab.screen.text') : stem;
        addTab(`md-${stem}`, label);
      });

      if (!readsAsItem || tabsContainer.children.length > 1) {
        tabsContainer.classList.add('visible');
      }
    }

    function activateTab(tabId) {
      if (!segmentData || tabId === activeTab) {
        return;
      }

      tabsContainer.querySelectorAll('.tr-tab').forEach(tab => {
        const isActive = tab.dataset.tab === tabId;
        tab.classList.toggle('active', isActive);
        tab.setAttribute('aria-selected', String(isActive));
        tab.setAttribute('tabindex', isActive ? '0' : '-1');
      });

      Object.values(tabPanes).forEach(pane => pane.classList.remove('active'));

      let pane = tabPanes[tabId];
      if (!pane) {
        pane = document.createElement('div');
        pane.className = 'tr-tab-pane';
        pane.dataset.tab = tabId;
        pane.setAttribute('role', 'tabpanel');
        pane.setAttribute('tabindex', '0');
        pane.id = 'tr-tabpanel-' + tabId;
        if (tabsContainer.querySelector(`#tr-tab-${tabId}`)) {
          pane.setAttribute('aria-labelledby', 'tr-tab-' + tabId);
        }
        panel.appendChild(pane);
        tabPanes[tabId] = pane;

        if (tabId === 'transcript') {
          const readsAsItem = isDateOnlyImport(selectedSegment) || isWrittenOrUnplaced(selectedSegment);
          if (readsAsItem && !tabsContainer.classList.contains('visible')) {
            pane.removeAttribute('role');
            pane.removeAttribute('aria-labelledby');
          }
          // Messages and an unplaced conversation keep their turns even
          // when they read by date; a note or document is filed text.
          if (isDateOnlyImport(selectedSegment) && !isWrittenOrUnplaced(selectedSegment)) {
            renderFiledText(segmentData, pane);
          } else {
            renderSegmentTimeline(segmentData, true, true, pane, true, true);
          }
        } else if (tabId === 'audio') {
          renderSegmentTimeline(segmentData, true, false, pane, false, false);
        } else if (tabId === 'screen') {
          const segmentToken = selectedSegment?.key;
          pane.innerHTML = window.SurfaceState.loading({ text: window.JournalI18n.render('journal.window.transcripts.text.loading.screen.entries'), busy: false });
          prepareScreenFrames(
            segmentData,
            pane,
            segmentToken,
            currentSegmentAbort?.signal
          )
            .then(() => {
              if (!selectedSegment || selectedSegment.key !== segmentToken) {
                return;
              }
              if (tabPanes[tabId] !== pane) {
                return;
              }
              renderSegmentTimeline(segmentData, false, true, pane, false, false);
            })
            .catch(() => {
              if (!selectedSegment || selectedSegment.key !== segmentToken) {
                return;
              }
              if (tabPanes[tabId] !== pane) {
                return;
              }
              pane.innerHTML = window.SurfaceState.error({
                heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.couldn.t.load.screen.entries'),
                desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.something.went.wrong.decoding.the.screen')
              });
            });
        } else if (tabId === 'image') {
          renderSegmentTimeline(segmentData, false, false, pane, false, true);
        } else if (tabId === 'browser') {
          renderSegmentTimeline(segmentData, false, false, pane, true, false);
        } else if (tabId === 'signals') {
          renderSignalsTab(segmentData, pane);
        } else if (tabId.startsWith('md-')) {
          const stem = tabId.slice(3);
          const content = (stem === 'sense' && segmentData.sense_display) || (segmentData.md_files || {})[stem] || '';
          pane.innerHTML = `<div class="tr-md-content">${window.AppServices.renderMarkdown(content, { literalValues: stem === 'screen_text' })}</div>`;
        }
      }

      pane.classList.add('active');
      collapseLongPreBlocks(pane);
      // A link's moment was cued while this pane was still hidden.
      if (linkedMoment?.reveal?.pane === pane) {
        linkedMoment.reveal.entry.scrollIntoView({ block: 'center' });
        linkedMoment.reveal = null;
      }
      // A cached pane is not re-rendered, so restore the count it reported.
      updateSegmentEntryCount(tabEntryCounts[tabId] ?? null);
      activeTab = tabId;
      if (selectedSegment) {
        ctx.history.replace(segmentLocation(selectedSegment, tabId));
      }
    }

    function formatAudioDuration(seconds) {
      const totalSeconds = Math.round(Number(seconds) || 0);
      if (totalSeconds <= 0) return '';

      const secs = totalSeconds % 60;
      const minutes = Math.floor(totalSeconds / 60) % 60;
      const hours = Math.floor(totalSeconds / 3600);
      if (hours > 0) {
        return `${hours}:${String(minutes).padStart(2, '0')}:${String(secs).padStart(2, '0')}`;
      }
      return `${minutes}:${String(secs).padStart(2, '0')}`;
    }

    function renderAnalyzeButton(modality, label) {
      const title = BTN_TITLE(modality);
      return `<button type="button" class="tr-analyze-action" data-modality="${escapeHtml(modality)}" aria-label="${escapeHtml(title)}" title="${escapeHtml(title)}">${escapeHtml(label)}</button>`;
    }

    function renderFailedDataState(modality) {
      return `<div class="surface-state surface-state--error" role="alert">`
        + `<div class="surface-state-icon" aria-hidden="true">${emptyIconForModality(modality)}</div>`
        + `<h2 class="surface-state-heading">${escapeHtml(dataStateCopy(modality, 'failed'))}</h2>`
        + `<p class="surface-state-desc">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.analysis.errored.for.this.segment')}</p>`
        + `<div class="surface-state-action">${renderAnalyzeButton(modality, BTN_LABEL_FAILED)}</div>`
        + `</div>`;
    }

    // A missing original a trash item holds: the reason says so, and the way to
    // the trash goes with it, where it can be recovered until it's erased.
    const MEDIA_TRASH_COPY = Object.freeze({
      link: window.JournalI18n.render('journal.window.transcripts.media.trash.copy.link.open.trash'),
      desc: window.JournalI18n.render('journal.window.transcripts.media.trash.copy.desc.you.can.recover.it.there.until')
    });

    function mediaInTrash(data, modality) {
      return data?.media_removal?.[modality + '_in_trash'] === true;
    }

    function mediaTrashLink() {
      return `<a href="/app/settings#trash" data-role="media-open-trash">${escapeHtml(MEDIA_TRASH_COPY.link)}</a>`;
    }

    function renderDataStateAffordance(modality, state, data) {
      const icon = emptyIconForModality(modality);
      if (state === 'pending') {
        const metadata = [];
        if (modality === 'audio') {
          const duration = formatAudioDuration(data.duration);
          if (duration) metadata.push(window.JournalI18n.render('journal.window.transcripts.media.pending.duration', [['value', duration]]));
        }
        const size = formatSize(data.media_sizes?.[modality]);
        if (size) metadata.push(window.JournalI18n.render('journal.window.transcripts.media.pending.raw.media', [['value', size]]));
        return window.SurfaceState.empty({
          icon,
          heading: dataStateCopy(modality, 'pending'),
          desc: metadata.join(' · '),
          action: renderAnalyzeButton(modality, BTN_LABEL_PENDING)
        });
      }
      if (state === 'analyzing') {
        const label = modality === 'image' ? dataStateCopy(modality, 'analyzing') : ANALYZING_LABEL;
        return `<div class="surface-state surface-state--loading tr-analyzing-state" role="status">`
          + `<div class="surface-state-spinner" aria-hidden="true"></div>`
          + `<span class="surface-state-text">${escapeHtml(label)}</span>`
          + `<span class="surface-state-still" data-role="still-working" aria-hidden="true">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.still.working')}</span>`
          + `</div>`;
      }
      if (state === 'failed' || state === 'failed_final') {
        return renderFailedDataState(modality);
      }
      if (state === 'purged') {
        const inTrash = mediaInTrash(data, modality);
        return window.SurfaceState.empty({
          icon,
          heading: data.media_removal[modality],
          desc: inTrash ? MEDIA_TRASH_COPY.desc : '',
          action: inTrash ? mediaTrashLink() : ''
        });
      }
      return '';
    }

    function tabExists(tabId) {
      return [...tabsContainer.querySelectorAll('.tr-tab')].some(tab => tab.dataset.tab === tabId);
    }

    function isLocationOnlySegment(data) {
      if (selectedSegment && selectedSegment.locationRow) return true;
      if (!data || data.holds_location !== true) return false;
      const ds = data.data_state || {};
      return !ds.audio && !ds.screen && !ds.image && !ds.markdown && !ds.browser;
    }

    function renderLocationOnlyDetail(data) {
      const hasReadings = typeof data?.location_readings === 'number';
      const readings = hasReadings ? data.location_readings : null;
      let desc = window.JournalI18n.render('journal.window.transcripts.window.explanation.this.segment.holds.location.only');
      if (readings > 0) {
        desc = window.JournalI18n.render('journal.window.transcripts.location.only.readings', transcriptCount('readings', readings));
      }
      const countAttr = (hasReadings && readings > 0) ? ` data-location-count="${readings}"` : '';
      panel.innerHTML = `
        <div class="tr-location-only-detail" data-location-only="true">
          <div class="surface-state surface-state--empty">
            <div class="surface-state-icon" aria-hidden="true">${window.ConveyIcons.svg('map-pin')}</div>
            <p class="surface-state-desc"${countAttr}>${escapeHtml(desc)}</p>
          </div>
        </div>
      `;
    }

    function renderFiledText(data, target) {
      const parts = (data.chunks || [])
        .filter(chunk => chunk.type === 'markdown')
        .map(chunk => window.AppServices.renderMarkdown(chunk.markdown))
        .filter(Boolean);
      if (!parts.length) {
        target.textContent = window.JournalI18n.render('journal.window.shared.import.item.text.unavailable');
        return;
      }
      target.innerHTML = `<div class="tr-md-content">${parts.join('')}</div>`;
    }

    function renderLoadedSegmentData(data, requestedTabId = null) {
      segmentData = data;
      // Still images need no decoding, so any tab can show them at once.
      currentImageFiles = data.image_files || {};
      selectedSegmentZone = data.capture_zone || null;
      TR_COPY = data.transcripts_copy || {};
      updateRangeText();

      if (isLocationOnlySegment(data)) {
        tabsContainer.innerHTML = '';
        tabsContainer.classList.remove('visible');
        stopAnalyzePoller();
        renderLocationOnlyDetail(data);
        renderWarningNotice(data);
        return;
      }

      if (selectedSegment && data.item) {
        selectedSegment = { ...selectedSegment, item: data.item };
        titleEl.textContent = importItemLabel(selectedSegment) || humanizeStreamName(selectedSegment.stream);
      }
      if (data.granola?.title) {
        titleEl.textContent = data.granola.title;
      }
      buildTabBar(data);
      const nextTab = requestedTabId && tabExists(requestedTabId) ? requestedTabId : 'transcript';
      activateTab(nextTab);
      renderWarningNotice(data);
    }

    function currentAnalyzingModalities(dataState) {
      return Object.keys(dataState || {}).filter(modality => dataState[modality] === 'analyzing');
    }

    function stopAnalyzePoller() {
      if (analyzePollTimer) {
        clearInterval(analyzePollTimer);
      }
      analyzePollTimer = null;
      analyzePollStartedAt = 0;
      analyzePollModalities.clear();
    }

    function updateSelectedSegmentDataState(dataState) {
      if (!selectedSegment) return;
      selectedSegment.data_state = { ...(selectedSegment.data_state || {}), ...dataState };
      const matchingSegment = allSegments.find(
        seg => seg.key === selectedSegment.key && seg.stream === selectedSegment.stream
      );
      if (matchingSegment) {
        matchingSegment.data_state = { ...(matchingSegment.data_state || {}), ...dataState };
      }
    }

    function applySegmentDataState(dataState) {
      if (!dataState) return;
      updateSelectedSegmentDataState(dataState);
      buildZoomSegments();
      refreshSegmentIndicators();
    }

    function rerenderSegmentAfterAnalysis(data, preferredTab = activeTab) {
      currentVideoFiles = {};
      currentImageFiles = {};
      tabPanes = {};
      tabEntryCounts = {};
      activeTab = null;
      screenDecoded = false;
      frameCapture.clear();
      renderLoadedSegmentData(data, preferredTab);
    }

    function showAnalyzeError(button, message) {
      const action = button.closest('.surface-state-action') || button.parentElement;
      if (!action) return;
      action.parentElement?.querySelectorAll('.tr-analyze-error').forEach(el => el.remove());
      const errorEl = document.createElement('p');
      errorEl.className = 'tr-analyze-error';
      errorEl.setAttribute('role', 'alert');
      errorEl.textContent = message || window.JournalI18n.render('journal.window.transcripts.visible.text.analysis.request.failed');
      action.insertAdjacentElement('afterend', errorEl);
    }

    function renderAnalyzeStateInPlace(button, modality, state) {
      const surface = button.closest('.surface-state');
      if (!surface || !segmentData) return;
      const replacement = renderDataStateAffordance(modality, state, segmentData);
      if (replacement) {
        surface.outerHTML = replacement;
      }
    }

    async function performAnalyzeNow(button, modality) {
      if (!selectedSegment || !modality) return;
      const seg = selectedSegment;
      button.disabled = true;
      button.setAttribute('aria-busy', 'true');
      button.closest('.surface-state')?.querySelectorAll('.tr-analyze-error').forEach(el => el.remove());

      try {
        const response = await fetch(
          `/app/transcripts/api/segment/${day}/${seg.stream}/${seg.key}/reprocess`,
          {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ modality })
          }
        );
        const data = await response.json().catch(() => ({}));
        if (!response.ok) {
          throw new Error(data.error || window.JournalI18n.render('journal.window.transcripts.analysis.request.failed'));
        }

        segmentData = { ...(segmentData || {}), data_state: data.data_state || {} };
        applySegmentDataState(segmentData.data_state);
        renderAnalyzeStateInPlace(button, modality, 'analyzing');
        startAnalyzePoller([modality]);
      } catch (err) {
        button.disabled = false;
        button.removeAttribute('aria-busy');
        showAnalyzeError(button, err.message);
      }
    }

    async function pollAnalyzeState() {
      if (!selectedSegment || analyzePollModalities.size === 0) {
        stopAnalyzePoller();
        return;
      }
      if (Date.now() - analyzePollStartedAt > ANALYZE_POLL_MAX_MS) {
        stopAnalyzePoller();
        return;
      }

      const seg = selectedSegment;
      try {
        const response = await dayFetch(`/app/transcripts/api/segment/${day}/${seg.stream}/${seg.key}`);
        if (!response.ok) return;
        const data = await response.json();
        if (!selectedSegment || selectedSegment.key !== seg.key || selectedSegment.stream !== seg.stream) {
          return;
        }
        const dataState = data.data_state || {};
        segmentData = data;
        applySegmentDataState(dataState);

        const watched = [...analyzePollModalities];
        const complete = watched.every(modality => (
          dataState[modality] === 'analyzed'
          || dataState[modality] === 'empty'
          || dataState[modality] === 'failed'
          || dataState[modality] === 'failed_final'
        ));
        if (complete) {
          const preferredTab = activeTab;
          stopAnalyzePoller();
          rerenderSegmentAfterAnalysis(data, preferredTab);
        }
      } catch (err) {
        console.debug('transcripts: analysis poll failed', err);
      }
    }

    function startAnalyzePoller(modalities) {
      modalities.forEach(modality => analyzePollModalities.add(modality));
      if (analyzePollTimer) return;
      analyzePollStartedAt = Date.now();
      analyzePollTimer = setInterval(pollAnalyzeState, ANALYZE_POLL_INTERVAL_MS);
    }

    function signalTimestampLabel(value) {
      if (!value) return '';
      if (window.JournalClock && typeof window.JournalClock.formatInstant === 'function') {
        const formatted = window.JournalClock.formatInstant(value);
        if (formatted) return formatted;
      }
      const parsed = new Date(value);
      if (Number.isNaN(parsed.getTime())) return String(value);
      return parsed.toLocaleString([], {
        month: 'short',
        day: 'numeric',
        hour: 'numeric',
        minute: '2-digit',
        second: '2-digit'
      });
    }

    function signalValue(value) {
      if (value === null || value === undefined || value === '') return '-';
      if (typeof value === 'number') {
        return Number.isInteger(value) ? String(value) : value.toFixed(6).replace(/0+$/, '').replace(/\.$/, '');
      }
      if (typeof value === 'boolean') return value ? window.JournalI18n.render('journal.window.transcripts.signals.yes') : window.JournalI18n.render('journal.window.transcripts.signals.no');
      return String(value);
    }

    function renderSignalRow(label, value) {
      return `<div class="tr-signal-row"><div class="tr-signal-label">${escapeHtml(label)}</div><div class="tr-signal-value">${escapeHtml(signalValue(value))}</div></div>`;
    }

    function renderSignalSection(title, bodyHtml) {
      if (!bodyHtml) return '';
      return `<section class="tr-signal-section"><h3>${escapeHtml(title)}</h3><div class="tr-signal-body">${bodyHtml}</div></section>`;
    }

    function signalEventsOf(data, eventType) {
      return (data.signals?.events || []).filter(event => event.event_type === eventType);
    }

    function latestSignal(data, eventType) {
      const events = signalEventsOf(data, eventType);
      return events.length ? events[events.length - 1] : null;
    }

    function renderSignalsTab(data, targetEl) {
      const signals = data.signals || {};
      const events = Array.isArray(signals.events) ? signals.events : [];
      const counts = signals.counts || {};
      if (events.length === 0) {
        targetEl.innerHTML = window.SurfaceState.empty({
          icon: window.ConveyIcons.svg('activity'),
          heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.no.signal.context'),
          desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.this.segment.has.no.imported.mentra')
        });
        return;
      }

      const countChips = Object.entries(counts)
        .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
        .map(([type, count]) => `<span class="tr-signal-chip"><strong>${count}</strong>${escapeHtml(type)}</span>`)
        .join('');

      let html = `<div class="tr-signals">`;
      html += `<div class="tr-signal-summary">${countChips}</div>`;

      const locationEvents = signalEventsOf(data, 'location_update');
      const latestLocation = latestSignal(data, 'location_update');
      if (latestLocation) {
        const payload = latestLocation.payload || {};
        html += renderSignalSection(
          window.JournalI18n.render('journal.window.transcripts.signals.section.location'),
          renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.updates'), locationEvents.length)
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.latest'), signalTimestampLabel(latestLocation.timestamp))
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.latitude'), payload.lat)
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.longitude'), payload.lng)
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.accuracy'), payload.accuracy ? `${payload.accuracy} m` : '')
        );
      }

      const latestBattery = latestSignal(data, 'glasses_battery');
      if (latestBattery) {
        const payload = latestBattery.payload || {};
        html += renderSignalSection(
          window.JournalI18n.render('journal.window.transcripts.signals.section.glasses.battery'),
          renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.latest'), signalTimestampLabel(latestBattery.timestamp))
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.level'), payload.level !== undefined ? `${payload.level}%` : '')
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.charging'), payload.charging)
        );
      }

      const photoTypes = [
        'button_press',
        'photo_request_started',
        'photo_request_accepted',
        'photo_request_succeeded',
        'photo_request_failed',
        'photo_request_skipped',
        'photo_input_ignored'
      ];
      const photoRows = photoTypes
        .filter(type => counts[type])
        .map(type => renderSignalRow(type, counts[type]))
        .join('');
      if (photoRows) {
        html += renderSignalSection(window.JournalI18n.render('journal.window.transcripts.window.explanation.photos.and.button.presses'), photoRows);
      }

      const capabilities = latestSignal(data, 'capabilities_update') || latestSignal(data, 'session_start');
      if (capabilities?.payload?.capabilities) {
        const caps = capabilities.payload.capabilities;
        html += renderSignalSection(
          window.JournalI18n.render('journal.window.transcripts.signals.section.device.capabilities'),
          renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.camera'), caps.hasCamera)
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.microphone'), caps.hasMicrophone)
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.speaker'), caps.hasSpeaker)
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.button'), caps.hasButton)
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.wifi'), caps.hasWifi)
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.camera.resolution'), caps.camera?.resolution ? `${caps.camera.resolution.width}x${caps.camera.resolution.height}` : '')
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.video.recording'), caps.camera?.video?.canRecord)
        );
      }

      const calendar = signals.calendar || {};
      const calendarEvents = Array.isArray(calendar.events) ? calendar.events : [];
      if (calendarEvents.length > 0) {
        const items = calendarEvents.slice(0, 12).map(event => {
          const when = `${signalTimestampLabel(event.dtStart)}${event.dtEnd ? ' - ' + signalTimestampLabel(event.dtEnd) : ''}`;
          const timing = event.seen_count > 1
            ? window.JournalI18n.render('journal.window.transcripts.signals.calendar.repeated', [
              ...transcriptCount('visits', event.seen_count), ['range', when],
            ])
            : when;
          return `<li><strong>${escapeHtml(event.title || window.JournalI18n.render('journal.window.transcripts.status.text.untitled.event'))}</strong><br><span class="tr-signal-label">${escapeHtml(timing)}</span></li>`;
        }).join('');
        const hiddenEvents = calendarEvents.length - 12;
        const more = hiddenEvents > 0
          ? `<div class="tr-signal-label">${window.JournalI18n.renderHtml('journal.window.transcripts.signals.calendar.hidden.count', [['quantity', window.JournalI18n.cardinalQuantity(hiddenEvents)], ['count', String(hiddenEvents)]])}</div>`
          : '';
        html += renderSignalSection(
          window.JournalI18n.render('journal.window.transcripts.signals.section.calendar.snapshot'),
          renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.raw.rows'), calendar.total || 0)
          + renderSignalRow(window.JournalI18n.render('journal.window.transcripts.signals.row.unique.events'), calendar.unique || calendarEvents.length)
          + `<ul class="tr-signal-list">${items}</ul>${more}`
        );
      }

      const latestRaw = events.slice(-12).reverse();
      html += `<details class="tr-signal-raw"><summary>${window.JournalI18n.renderHtml('journal.window.transcripts.disclosure.label.recent.raw.signal.rows')}</summary><pre>${escapeHtml(JSON.stringify(latestRaw, null, 2))}</pre></details>`;
      html += `</div>`;
      targetEl.innerHTML = html;
      const summary = targetEl.querySelector('.tr-signal-summary');
      if (summary && window.JournalClock && typeof window.JournalClock.placeNote === 'function') {
        window.JournalClock.placeNote(summary, { inside: false });
      }
    }

    function trCopy(key) {
      const values = {
        TR_SPEAKER_CHANGE_LABEL: TR_COPY.TR_SPEAKER_CHANGE_LABEL || '',
        TR_SPEAKER_ASSIGN_LABEL: TR_COPY.TR_SPEAKER_ASSIGN_LABEL || '',
        TR_SPEAKER_PICKER_TITLE: TR_COPY.TR_SPEAKER_PICKER_TITLE || '',
        TR_SPEAKER_PICKER_SEARCH_PLACEHOLDER: TR_COPY.TR_SPEAKER_PICKER_SEARCH_PLACEHOLDER || '',
        TR_SPEAKER_PICKER_OWNER: TR_COPY.TR_SPEAKER_PICKER_OWNER || '',
        TR_SPEAKER_OWNER_NAME: TR_COPY.TR_SPEAKER_OWNER_NAME || '',
        TR_SPEAKER_PICKER_EMPTY: TR_COPY.TR_SPEAKER_PICKER_EMPTY || '',
        TR_SPEAKER_SOMEONE_ELSE: TR_COPY.TR_SPEAKER_SOMEONE_ELSE || '',
        TR_SPEAKER_PICKER_NO_RESULTS: TR_COPY.TR_SPEAKER_PICKER_NO_RESULTS || '',
        TR_SPEAKER_UNKNOWN_CHIP: TR_COPY.TR_SPEAKER_UNKNOWN_CHIP || '',
        TR_SPEAKER_VOICE_ASSISTIVE: TR_COPY.TR_SPEAKER_VOICE_ASSISTIVE || '',
        TR_SPEAKER_VOICE_NAMED_ASSISTIVE: TR_COPY.TR_SPEAKER_VOICE_NAMED_ASSISTIVE || '',
        TR_SPEAKER_VOICE_SCOPE: TR_COPY.TR_SPEAKER_VOICE_SCOPE || '',
        TR_SPEAKER_VOICE_SCOPE_NOTE: TR_COPY.TR_SPEAKER_VOICE_SCOPE_NOTE || '',
        TR_SPEAKER_CONFIDENCE_HIGH: TR_COPY.TR_SPEAKER_CONFIDENCE_HIGH || '',
        TR_SPEAKER_CONFIDENCE_UNKNOWN: TR_COPY.TR_SPEAKER_CONFIDENCE_UNKNOWN || '',
        TR_SPEAKER_MARGIN_OWNER: TR_COPY.TR_SPEAKER_MARGIN_OWNER || '',
        TR_SPEAKER_MARGIN_ACOUSTIC: TR_COPY.TR_SPEAKER_MARGIN_ACOUSTIC || '',
        TR_SPEAKER_ACTION_UNAVAILABLE: TR_COPY.TR_SPEAKER_ACTION_UNAVAILABLE || '',
        TR_SPEAKER_NO_EMBEDDING: TR_COPY.TR_SPEAKER_NO_EMBEDDING || '',
        TR_SPEAKER_CORRECT_RETRY: TR_COPY.TR_SPEAKER_CORRECT_RETRY || '',
        TR_SPEAKER_CORRECT_BUSY: TR_COPY.TR_SPEAKER_CORRECT_BUSY || '',
        TR_SPEAKER_OWNER_TOO_CLOSE: TR_COPY.TR_SPEAKER_OWNER_TOO_CLOSE || '',
        TR_SPEAKER_OWNER_IDENTITY_REQUIRED: TR_COPY.TR_SPEAKER_OWNER_IDENTITY_REQUIRED || '',
        TR_SPEAKER_ALREADY_CORRECT: TR_COPY.TR_SPEAKER_ALREADY_CORRECT || '',
        TR_SPEAKER_PROPAGATION_DISMISS: TR_COPY.TR_SPEAKER_PROPAGATION_DISMISS || '',
        TR_SPEAKER_PICKER_UNKNOWN: TR_COPY.TR_SPEAKER_PICKER_UNKNOWN || '',
        TR_SPEAKER_PICKER_GUESS: TR_COPY.TR_SPEAKER_PICKER_GUESS || '',
        TR_SPEAKER_LINE_ONLY: TR_COPY.TR_SPEAKER_LINE_ONLY || '',
        TR_SPEAKER_OWNER_SET: TR_COPY.TR_SPEAKER_OWNER_SET || '',
        TR_SPEAKER_OWNER_UNKNOWN_CHIP: TR_COPY.TR_SPEAKER_OWNER_UNKNOWN_CHIP || '',
        TR_SPEAKER_LINE_REFUSED: TR_COPY.TR_SPEAKER_LINE_REFUSED || '',
        TR_SPEAKER_LINE_SOURCE: TR_COPY.TR_SPEAKER_LINE_SOURCE || '',
        TR_SPEAKER_LINE_CHANGED: TR_COPY.TR_SPEAKER_LINE_CHANGED || '',
        TR_SPEAKER_LINE_OWNER: TR_COPY.TR_SPEAKER_LINE_OWNER || '',
        TR_SPEAKER_LINE_PERSON: TR_COPY.TR_SPEAKER_LINE_PERSON || '',
        TR_SPEAKER_LINE_UNREADABLE: TR_COPY.TR_SPEAKER_LINE_UNREADABLE || ''
      };
      return values[key] || '';
    }

    function speakerNameForLabel(label) {
      if (!label) return '';
      if (label.is_owner) return trCopy('TR_SPEAKER_OWNER_NAME');
      return String(label.name || label.entity_id || '');
    }

    function speakerMarginText(label) {
      const parts = [];
      if (label?.owner_margin_declined) {
        parts.push(trCopy('TR_SPEAKER_MARGIN_OWNER'));
      }
      if (label?.acoustic_margin_declined) {
        parts.push(trCopy('TR_SPEAKER_MARGIN_ACOUSTIC'));
      }
      return parts.filter(Boolean).join(', ');
    }

    function speakerVoiceChipModel(voice) {
      if (voice.name) {
        return {
          text: window.JournalI18n.render('journal.window.transcripts.speaker.chip.probable.name', [['speaker_name', String(voice.name)]]),
          dotClass: 'tr-speaker-dot-medium',
          neutral: false,
          assistive: trCopy('TR_SPEAKER_VOICE_NAMED_ASSISTIVE')
        };
      }
      return {
        text: window.JournalI18n.render('journal.window.transcripts.speaker.chip.voice.number', [['voice_number', String(voice.voice_id ?? '')]]),
        dotClass: '',
        neutral: true,
        assistive: trCopy('TR_SPEAKER_VOICE_ASSISTIVE')
      };
    }

    function speakerChipModel(item) {
      const label = item.speaker_label || null;
      // A line carrying the owner's own answer shows that answer, marked as theirs.
      if (item.owner_line_label) {
        const named = item.owner_line_label.speaker && label;
        return {
          text: named ? speakerNameForLabel(label) : trCopy('TR_SPEAKER_OWNER_UNKNOWN_CHIP'),
          dotClass: '',
          neutral: !named,
          assistive: trCopy('TR_SPEAKER_OWNER_SET'),
          ownerSet: true
        };
      }
      if (!label && item.speaker_voice) {
        return speakerVoiceChipModel(item.speaker_voice);
      }
      if (!label && item.speaker_attribution) {
        return {
          text: item.speaker_attribution,
          dotClass: '',
          neutral: true,
          assistive: GRANOLA_COPY.speakerAssistive
        };
      }
      if (!label || label.confidence_state === 'unknown') {
        return {
          text: trCopy('TR_SPEAKER_UNKNOWN_CHIP'),
          dotClass: '',
          neutral: true,
          assistive: trCopy('TR_SPEAKER_CONFIDENCE_UNKNOWN')
        };
      }

      const name = speakerNameForLabel(label);
      const hasMargin = !!(label.owner_margin_declined || label.acoustic_margin_declined);
      if (hasMargin) {
        return {
          text: label.is_owner
            ? window.JournalI18n.render('journal.window.transcripts.speaker.chip.maybe.owner')
            : window.JournalI18n.render('journal.window.transcripts.speaker.chip.maybe.name', [['speaker_name', name]]),
          dotClass: 'tr-speaker-dot-medium',
          neutral: false,
          assistive: speakerMarginText(label)
        };
      }

      if (label.confidence_state === 'high') {
        return {
          text: name,
          dotClass: 'tr-speaker-dot-high',
          neutral: false,
          assistive: trCopy('TR_SPEAKER_CONFIDENCE_HIGH')
        };
      }

      return {
        text: label.is_owner
          ? window.JournalI18n.render('journal.window.transcripts.speaker.chip.probable.owner')
          : window.JournalI18n.render('journal.window.transcripts.speaker.chip.probable.name', [['speaker_name', name]]),
        dotClass: 'tr-speaker-dot-medium',
        neutral: false,
        assistive: ''
      };
    }

    function speakerCurrentEntityId(item) {
      return item.speaker_label?.entity_id || '';
    }

    function speakerActionUnavailableReason(item, data) {
      const labels = data.speaker_labels || {};
      if (!labels.present || !labels.loaded) {
        return trCopy('TR_SPEAKER_ACTION_UNAVAILABLE');
      }
      if (!item.has_embedding) {
        return trCopy('TR_SPEAKER_NO_EMBEDDING');
      }
      if (item.speaker_actionable !== true) {
        return trCopy('TR_SPEAKER_ACTION_UNAVAILABLE');
      }
      return '';
    }

    function renderSpeakerChipInner(model) {
      const dot = model.dotClass
        ? `<span class="tr-speaker-dot ${model.dotClass}" aria-hidden="true"></span>`
        : '';
      const assistive = model.assistive
        ? `<span class="sr-only">, ${escapeHtml(model.assistive)}</span>`
        : '';
      const ownerSet = model.ownerSet && window.ConveyIcons
        ? `<span class="tr-speaker-owner-set" aria-hidden="true">${window.ConveyIcons.svg('pencil')}</span>`
        : '';
      return `${dot}<span class="tr-speaker-name">${escapeHtml(model.text)}</span>${ownerSet}${assistive}`;
    }

    function ownerAnswersUnreadable(data) {
      return (data?.warning_details || []).some(warning => warning && warning.type === 'owner_line_labels');
    }

    // Whether the owner can say who spoke this one line: it has a statement id
    // in the source the segment says who spoke from, and the segment's earlier
    // answers can be read.
    function lineAnswerable(item, data) {
      const labels = data?.speaker_labels || {};
      return typeof item.sentence_id === 'number'
        && !!item.speaker_source
        && !ownerAnswersUnreadable(data)
        && !labels.ambiguous
        && (!labels.source || labels.source === item.speaker_source);
    }

    function voiceChipId(item) {
      const voice = item.speaker_voice;
      if (item.speaker_label || !voice) return '';
      const id = Number(voice.voice_id);
      return Number.isFinite(id) && id > 0 ? String(id) : '';
    }

    function renderSpeakerSlot(item, idx, data) {
      const model = speakerChipModel(item);
      const unavailableReason = speakerActionUnavailableReason(item, data);
      const currentSpeaker = speakerCurrentEntityId(item);
      const voiceId = voiceChipId(item);
      const answerable = lineAnswerable(item, data);
      const ownerSet = !!item.owner_line_label;
      if (unavailableReason && !voiceId && !answerable) {
        return `
          <span class="tr-speaker-disabled" title="${escapeHtml(unavailableReason)}">
            <span class="tr-speaker-chip${model.neutral ? ' tr-speaker-chip-neutral' : ''}">
              ${renderSpeakerChipInner(model)}
            </span>
            <span class="sr-only">${escapeHtml(unavailableReason)}</span>
          </span>
        `;
      }

      const actionLabel = currentSpeaker
        ? trCopy('TR_SPEAKER_CHANGE_LABEL')
        : trCopy('TR_SPEAKER_ASSIGN_LABEL');
      const sentenceId = Number(item.sentence_id);
      return `
        <button
          type="button"
          class="tr-speaker-button${model.neutral ? ' tr-speaker-chip-neutral' : ''}"
          data-action="speaker-picker"
          data-idx="${idx}"
          data-day="${escapeHtml(day)}"
          data-stream="${escapeHtml(selectedSegment?.stream || '')}"
          data-stream-layout="${escapeHtml(selectedSegment?.stream_layout || '')}"
          data-segment-key="${escapeHtml(selectedSegment?.key || '')}"
          data-sentence-id="${Number.isFinite(sentenceId) ? sentenceId : ''}"
          data-line-words="${escapeHtml(speakerLineWords(item.markdown))}"
          data-speaker-source="${escapeHtml(item.speaker_source || '')}"
          data-voice-id="${escapeHtml(voiceId)}"
          data-voice-only="${voiceId && unavailableReason && !answerable ? '1' : ''}"
          data-line-answerable="${answerable ? '1' : ''}"
          data-owner-set="${ownerSet ? '1' : ''}"
          data-teach="${!unavailableReason && !ownerSet ? '1' : ''}"
          aria-haspopup="dialog"
          aria-expanded="false"
          aria-label="${escapeHtml(actionLabel)}"
        >
          ${renderSpeakerChipInner(model)}
        </button>
      `;
    }

    function loadKnownSpeakers() {
      if (!knownSpeakersPromise) {
        knownSpeakersPromise = apiJson('/app/speakers/api/speakers/known')
          .then(payload => {
            knownSpeakers = Array.isArray(payload?.speakers) ? payload.speakers : [];
            return knownSpeakers;
          })
          .catch(err => {
            knownSpeakersPromise = null;
            throw err;
          });
      }
      return knownSpeakersPromise;
    }

    // Every person in the journal the owner can say spoke a line: admitted,
    // not hidden, and not the owner, who has their own choice.
    function loadJournalPeople() {
      if (!journalPeoplePromise) {
        journalPeoplePromise = apiJson('/app/entities/api/journal')
          .then(payload => {
            journalPeople = (Array.isArray(payload?.entities) ? payload.entities : [])
              .filter(entity => entity && entity.type === 'Person' && !entity.blocked && !entity.is_principal && entity.entity_id)
              .map(entity => ({ entity_id: String(entity.entity_id), name: String(entity.name || entity.entity_id) }))
              .sort((a, b) => a.name.localeCompare(b.name));
            return journalPeople;
          })
          .catch(err => {
            journalPeoplePromise = null;
            throw err;
          });
      }
      return journalPeoplePromise;
    }

    // A line's first words, without a leading "(mic)"-style source note.
    function speakerLineWords(text) {
      const words = String(text || '').replace(/^\s*\([^()]{1,24}\)\s*/, '').trim().split(/\s+/).filter(Boolean);
      return words.slice(0, 8).join(' ') + (words.length > 8 ? '…' : '');
    }

    function speakerPickerVoiceNaming(trigger) {
      const voiceScoped = !!trigger.dataset.voiceId;
      return voiceScoped && (trigger.dataset.voiceOnly === '1' || trigger.dataset.voiceScope !== 'off');
    }

    function speakerPickerOptions(people, query, options = {}) {
      const normalized = String(query || '').trim().toLowerCase();
      const filtered = people.filter(person => {
        if (!normalized) return true;
        return String(person.name || person.entity_id || '').toLowerCase().includes(normalized);
      });
      const lineChoices = !options.voiceNaming;
      // The line's own answers come first, then the people, and last the way
      // out to the speakers page, which leaves this page.
      return [
        ...(lineChoices ? [{ kind: 'owner', name: trCopy('TR_SPEAKER_PICKER_OWNER'), entity_id: '' }] : []),
        ...(lineChoices ? [{ kind: 'unknown', name: trCopy('TR_SPEAKER_PICKER_UNKNOWN'), entity_id: '' }] : []),
        ...(lineChoices && options.ownerSet ? [{ kind: 'guess', name: trCopy('TR_SPEAKER_PICKER_GUESS'), entity_id: '' }] : []),
        ...filtered.map(person => ({
          kind: 'known',
          name: String(person.name || person.entity_id || ''),
          entity_id: String(person.entity_id || '')
        })),
        ...(options.teach ? [{ kind: 'someone_else', name: trCopy('TR_SPEAKER_SOMEONE_ELSE'), entity_id: '', handoff: true }] : [])
      ];
    }

    function speakerPickerOptionHtml(option, trigger) {
      const data = `data-speaker-option="${escapeHtml(option.kind)}" data-speaker-id="${escapeHtml(option.entity_id)}" data-speaker-name="${escapeHtml(option.name)}"`;
      if (option.handoff) {
        return `<hr class="tr-speaker-picker-divider"><a class="tr-speaker-picker-option tr-speaker-picker-handoff" href="${escapeHtml(speakerStatementHandoffUrl(trigger))}" ${data} data-speaker-handoff="statement">${escapeHtml(option.name)}</a>`;
      }
      return `<button type="button" class="tr-speaker-picker-option" ${data}>${escapeHtml(option.name)}</button>`;
    }

    // The line a picker answers for: its time and first words, which head the
    // picker on a phone, where the line itself may be out of sight.
    function speakerPickerLine(trigger) {
      const time = trigger.closest('.tr-entry')?.querySelector('.tr-entry-time')?.textContent || '';
      const words = trigger.dataset.lineWords || '';
      return time && words ? { time, words } : null;
    }

    function speakerPickerTitleHtml(trigger) {
      const line = speakerPickerLine(trigger);
      const any = `<span class="tr-speaker-picker-title-any">${escapeHtml(trCopy('TR_SPEAKER_PICKER_TITLE'))}</span>`;
      if (!line) return `<div class="tr-speaker-picker-title">${any}</div>`;
      return `<div class="tr-speaker-picker-title tr-speaker-picker-title--line">${any}<span class="tr-speaker-picker-title-line"><span class="tr-speaker-picker-title-time">${escapeHtml(line.time)}</span> <span class="tr-speaker-picker-title-words">${escapeHtml(line.words)}</span></span></div>`;
    }

    function lineOnlyHtml() {
      const link = document.createElement('a');
      link.setAttribute('href', '/app/speakers/');
      link.dataset.copyKey = 'TR_SPEAKER_LINE_ONLY_LINK';
      const body = document.createElement('p');
      body.dataset.copyKey = 'TR_SPEAKER_LINE_ONLY_BODY';
      body.append(...window.JournalI18n.renderMarkup('journal.window.transcripts.speaker.line.only.help', [], { elements: { link } }));
      const bodyHtml = body.outerHTML;
      return `<div class="tr-speaker-picker-line-only" data-copy-key="TR_SPEAKER_LINE_ONLY">${window.Drawer.render({ id: 'speaker-line-only', label: trCopy('TR_SPEAKER_LINE_ONLY'), bodyHtml })}</div>`;
    }

    function positionSpeakerPicker(trigger, popover) {
      const rect = trigger.getBoundingClientRect();
      const margin = 12;
      const left = Math.min(
        Math.max(margin, rect.left),
        Math.max(margin, window.innerWidth - popover.offsetWidth - margin)
      );
      const preferredTop = rect.bottom + 6;
      const fallbackTop = rect.top - popover.offsetHeight - 6;
      const top = preferredTop + popover.offsetHeight + margin <= window.innerHeight
        ? preferredTop
        : Math.max(margin, fallbackTop);
      popover.style.left = `${left}px`;
      popover.style.top = `${top}px`;
    }

    function speakerPickerFocusable(popover) {
      return [...popover.querySelectorAll(
        'button:not([disabled]), input:not([disabled]), a[href], summary, [tabindex]:not([tabindex="-1"])'
      )];
    }

    function trapSpeakerPickerTab(e) {
      const popover = activeSpeakerPicker?.popover;
      if (!popover) return;
      const focusable = speakerPickerFocusable(popover);
      if (focusable.length === 0) return;
      const currentIndex = focusable.indexOf(document.activeElement);
      const nextIndex = e.shiftKey
        ? (currentIndex <= 0 ? focusable.length - 1 : currentIndex - 1)
        : (currentIndex >= focusable.length - 1 ? 0 : currentIndex + 1);
      e.preventDefault();
      focusable[nextIndex].focus();
    }

    function handleSpeakerPickerKeydown(e) {
      if (!activeSpeakerPicker) return;
      if (e.key === 'Escape') {
        e.preventDefault();
        closeSpeakerPicker();
        return;
      }
      if (e.key === 'Tab') {
        trapSpeakerPickerTab(e);
      }
    }

    function handleSpeakerPickerOutsidePointer(e) {
      if (!activeSpeakerPicker) return;
      const { popover, trigger } = activeSpeakerPicker;
      if (popover.contains(e.target) || trigger.contains(e.target)) return;
      closeSpeakerPicker({ restoreFocus: false });
    }

    function handleSpeakerPickerReposition() {
      if (!activeSpeakerPicker) return;
      positionSpeakerPicker(activeSpeakerPicker.trigger, activeSpeakerPicker.popover);
    }

    function closeSpeakerPicker(options = {}) {
      if (!activeSpeakerPicker) return;
      const { popover, trigger } = activeSpeakerPicker;
      activeSpeakerPicker = null;
      popover.remove();
      trigger.setAttribute('aria-expanded', 'false');
      document.removeEventListener('keydown', handleSpeakerPickerKeydown);
      document.removeEventListener('pointerdown', handleSpeakerPickerOutsidePointer, true);
      window.removeEventListener('resize', handleSpeakerPickerReposition, true);
      window.removeEventListener('scroll', handleSpeakerPickerReposition, true);
      if (options.restoreFocus !== false && document.contains(trigger)) {
        trigger.focus();
      }
    }

    function speakerStatementHandoffUrl(trigger) {
      const pairs = [
        ['voice_day', trigger.dataset.day || day || ''],
        ['voice_stream_layout', trigger.dataset.streamLayout || selectedSegment?.stream_layout || ''],
        ['voice_stream', trigger.dataset.stream || selectedSegment?.stream || ''],
        ['voice_segment_key', trigger.dataset.segmentKey || selectedSegment?.key || ''],
        ['voice_source', trigger.dataset.speakerSource || ''],
        ['voice_sentence_id', trigger.dataset.sentenceId || '']
      ];
      const query = pairs
        .map(([key, value]) => `${key}=${encodeURIComponent(value)}`)
        .join('&');
      return `/app/speakers/?${query}`;
    }

    function navigateSpeakerStatementHandoff(trigger) {
      closeSpeakerPicker({ restoreFocus: false });
      window.location.href = speakerStatementHandoffUrl(trigger);
    }

    function renderSpeakerPickerContent(popover, trigger, query = '') {
      const voiceScoped = !!trigger.dataset.voiceId;
      const voiceOnly = trigger.dataset.voiceOnly === '1';
      const voiceNaming = speakerPickerVoiceNaming(trigger);
      const ownerSet = trigger.dataset.ownerSet === '1';
      // Naming a voice everywhere offers the voices the journal knows; an answer
      // for this line alone can name anyone in the journal.
      const people = voiceNaming ? knownSpeakers : journalPeople;
      const showSearch = people.length > 7;
      const options = speakerPickerOptions(people, query, {
        voiceNaming,
        ownerSet,
        teach: !ownerSet && (voiceScoped || trigger.dataset.teach === '1')
      });
      const knownMatches = options.filter(option => option.kind === 'known');
      const emptyText = people.length === 0
        ? trCopy('TR_SPEAKER_PICKER_EMPTY')
        : (knownMatches.length === 0 ? trCopy('TR_SPEAKER_PICKER_NO_RESULTS') : '');
      popover.innerHTML = `
        <div class="tr-speaker-picker-head">
          ${speakerPickerTitleHtml(trigger)}
          <button type="button" class="tr-speaker-picker-close" aria-label="${escapeHtml(trCopy('TR_SPEAKER_PROPAGATION_DISMISS'))}">×</button>
        </div>
        ${voiceScoped && !ownerSet ? `<label class="tr-speaker-picker-scope"><input type="checkbox" class="tr-speaker-picker-scope-input"${voiceNaming ? ' checked' : ''}${voiceOnly ? ' disabled' : ''}> ${escapeHtml(trCopy('TR_SPEAKER_VOICE_SCOPE'))}</label>${voiceNaming ? `<div class="tr-speaker-picker-scope-note">${escapeHtml(trCopy('TR_SPEAKER_VOICE_SCOPE_NOTE'))}</div>` : ''}` : ''}
        ${voiceNaming ? '' : lineOnlyHtml()}
        ${showSearch ? `<input class="tr-speaker-picker-search" type="search" value="${escapeHtml(query)}" placeholder="${escapeHtml(trCopy('TR_SPEAKER_PICKER_SEARCH_PLACEHOLDER'))}">` : ''}
        <div class="tr-speaker-picker-list" role="group" aria-label="${escapeHtml(trCopy('TR_SPEAKER_PICKER_TITLE'))}">
          ${options.map(option => speakerPickerOptionHtml(option, trigger)).join('')}
        </div>
        <div class="tr-speaker-picker-tail">
          ${emptyText ? `<div>${escapeHtml(emptyText)}</div>` : ''}
        </div>
      `;

      popover.querySelector('.tr-speaker-picker-close')?.addEventListener('click', e => {
        e.preventDefault();
        e.stopPropagation();
        closeSpeakerPicker();
      });
      popover.querySelector('.tr-speaker-picker-scope-input')?.addEventListener('change', e => {
        trigger.dataset.voiceScope = e.target.checked ? 'on' : 'off';
        const search = popover.querySelector('.tr-speaker-picker-search');
        renderSpeakerPickerContent(popover, trigger, search ? search.value : query);
        positionSpeakerPicker(trigger, popover);
        popover.querySelector('.tr-speaker-picker-scope-input')?.focus();
      });
      popover.querySelector('.tr-speaker-picker-search')?.addEventListener('input', e => {
        renderSpeakerPickerContent(popover, trigger, e.target.value);
        positionSpeakerPicker(trigger, popover);
        popover.querySelector('.tr-speaker-picker-search')?.focus();
      });
      popover.querySelectorAll('.tr-speaker-picker-option').forEach(option => {
        option.addEventListener('click', e => {
          e.preventDefault();
          e.stopPropagation();
          if (option.dataset.speakerHandoff === 'statement') {
            navigateSpeakerStatementHandoff(trigger);
            return;
          }
          const kind = option.dataset.speakerOption;
          performSpeakerSelection(trigger, {
            kind: kind === 'known' ? 'person' : kind,
            entityId: option.dataset.speakerId || '',
            name: option.dataset.speakerName || ''
          });
        });
      });
    }

    function openSpeakerPicker(trigger) {
      if (activeSpeakerPicker?.trigger === trigger) {
        closeSpeakerPicker();
        return;
      }
      closeSpeakerPicker({ restoreFocus: false });
      const popover = document.createElement('div');
      popover.className = 'tr-speaker-popover';
      popover.setAttribute('role', 'dialog');
      popover.setAttribute('aria-label', trCopy('TR_SPEAKER_PICKER_TITLE'));
      popover.tabIndex = -1;
      document.body.appendChild(popover);
      activeSpeakerPicker = { popover, trigger };
      trigger.setAttribute('aria-expanded', 'true');
      renderSpeakerPickerContent(popover, trigger);
      positionSpeakerPicker(trigger, popover);
      ctx.on(document, 'keydown', handleSpeakerPickerKeydown);
      ctx.on(document, 'pointerdown', handleSpeakerPickerOutsidePointer, true);
      ctx.on(window, 'resize', handleSpeakerPickerReposition, true);
      ctx.on(window, 'scroll', handleSpeakerPickerReposition, true);
      Promise.all([
        trigger.dataset.voiceId ? loadKnownSpeakers() : Promise.resolve(knownSpeakers),
        trigger.dataset.lineAnswerable === '1' ? loadJournalPeople() : Promise.resolve(journalPeople)
      ])
        .then(() => {
          if (!activeSpeakerPicker || activeSpeakerPicker.popover !== popover) return;
          renderSpeakerPickerContent(popover, trigger);
          positionSpeakerPicker(trigger, popover);
          const first = popover.querySelector('.tr-speaker-picker-search, .tr-speaker-picker-option, .tr-speaker-picker-close');
          first?.focus();
        })
        .catch(err => {
          if (!activeSpeakerPicker || activeSpeakerPicker.popover !== popover) return;
          renderSpeakerStatus(
            trigger.closest('[data-speaker-slot]'),
            window.SurfaceState.serverMessageFrom(err) || trCopy('TR_SPEAKER_ACTION_UNAVAILABLE'),
            { error: true }
          );
        });
      requestAnimationFrame(() => {
        if (!activeSpeakerPicker || activeSpeakerPicker.popover !== popover) return;
        const first = popover.querySelector('.tr-speaker-picker-option, .tr-speaker-picker-close');
        first?.focus();
      });
    }

    function clearSpeakerRowMessages(trigger) {
      const row = trigger.closest('.tr-entry-audio');
      row?.querySelectorAll('.tr-speaker-status').forEach(el => el.remove());
    }

    function renderSpeakerStatus(slot, message, options = {}) {
      if (!slot) return;
      slot.parentElement?.querySelectorAll('.tr-speaker-status').forEach(el => el.remove());
      if (!message) return;
      const status = document.createElement('div');
      status.className = `tr-speaker-status${options.error ? ' tr-speaker-status-error' : ''}`;
      status.setAttribute('role', options.error ? 'alert' : 'status');
      if (options.copyKey) status.setAttribute('data-copy-key', options.copyKey);
      const text = document.createElement('span');
      text.textContent = message;
      status.appendChild(text);
      if (options.retry && typeof options.onRetry === 'function') {
        const retry = document.createElement('button');
        retry.type = 'button';
        retry.className = 'tr-speaker-status-retry';
        retry.textContent = options.actionLabel || trCopy('TR_SPEAKER_CORRECT_RETRY');
        retry.addEventListener('click', options.onRetry);
        status.appendChild(retry);
      }
      slot.insertAdjacentElement('afterend', status);
    }

    function speakerFailureMessage(err) {
      if (err.reasonCode === 'speaker_voiceprint_busy' || err.reasonCode === 'speaker_labels_busy') {
        return { message: trCopy('TR_SPEAKER_CORRECT_BUSY') || err.serverMessage, retry: true };
      }
      if (err.reasonCode === 'speaker_owner_voice_too_close') {
        return { message: trCopy('TR_SPEAKER_OWNER_TOO_CLOSE') || err.serverMessage, retry: false };
      }
      if (err.reasonCode === 'speaker_owner_identity_required') {
        return { message: trCopy('TR_SPEAKER_OWNER_IDENTITY_REQUIRED') || err.serverMessage, retry: false };
      }
      return { message: window.SurfaceState.serverMessageFrom(err) || trCopy('TR_SPEAKER_ACTION_UNAVAILABLE'), retry: false };
    }

    function speakerSlotFor(sentenceId, source) {
      const button = Array.from(document.querySelectorAll('[data-action="speaker-picker"]')).find(el => (
        el.dataset.sentenceId === String(sentenceId) && (el.dataset.speakerSource || '') === source
      ));
      return button?.closest('[data-speaker-slot]') || null;
    }

    // One request id per voice and person the owner picked, reused on retry, so a
    // retried tap resumes the same operation instead of starting another.
    const voiceRequestIds = new Map();

    async function performVoiceNaming(trigger, target) {
      const slot = trigger.closest('[data-speaker-slot]');
      const voiceId = Number(trigger.dataset.voiceId);
      const sentenceId = trigger.dataset.sentenceId;
      const source = trigger.dataset.speakerSource || '';
      const intent = `${voiceId}:${target.entityId}`;
      if (!voiceRequestIds.has(intent)) {
        // randomUUID exists only in a secure context; a plain-http page still needs an id.
        const unique = typeof crypto?.randomUUID === 'function'
          ? crypto.randomUUID()
          : `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
        voiceRequestIds.set(intent, `transcripts-voice-${unique}`);
      }
      trigger.disabled = true;
      trigger.setAttribute('aria-busy', 'true');
      try {
        const result = await apiJson('/app/speakers/api/voice/identify', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({
            voice_id: voiceId,
            entity_id: target.entityId,
            request_id: voiceRequestIds.get(intent),
            day: trigger.dataset.day || day,
            stream: trigger.dataset.stream || selectedSegment?.stream || '',
            segment_key: trigger.dataset.segmentKey || selectedSegment?.key || '',
            source,
            sentence_id: Number(sentenceId)
          })
        });
        const count = Number(result?.sentences_attributed ?? 0);
        const message = result?.status !== 'identified'
          ? trCopy('TR_SPEAKER_ALREADY_CORRECT')
          : window.JournalI18n.render('journal.window.transcripts.speaker.named.sentences', transcriptCount('sentences', count));
        await loadSegmentContent(selectedSegment, activeTab);
        renderSpeakerStatus(speakerSlotFor(sentenceId, source), message);
      } catch (err) {
        const failure = speakerFailureMessage(err);
        renderSpeakerStatus(slot, failure.message, {
          error: true,
          retry: failure.retry,
          onRetry: () => performVoiceNaming(trigger, target)
        });
      } finally {
        trigger.disabled = false;
        trigger.removeAttribute('aria-busy');
      }
    }

    const LINE_LABEL_REFUSALS = {
      owner_line_source_unsupported: 'TR_SPEAKER_LINE_SOURCE',
      owner_line_not_found: 'TR_SPEAKER_LINE_CHANGED',
      owner_line_person_unavailable: 'TR_SPEAKER_LINE_PERSON',
      owner_line_owner_not_admitted: 'TR_SPEAKER_LINE_OWNER',
      owner_line_answers_unreadable: 'TR_SPEAKER_LINE_UNREADABLE'
    };

    async function performSpeakerSelection(trigger, target) {
      const voiceNaming = speakerPickerVoiceNaming(trigger);
      closeSpeakerPicker();
      clearSpeakerRowMessages(trigger);
      if (voiceNaming && target.kind === 'person' && target.entityId) {
        return performVoiceNaming(trigger, target);
      }
      return performLineLabel(trigger, target);
    }

    // The owner's answer for who said this one line, or, for "back to your
    // journal's guess", taking that answer away. No voice is taught or changed.
    async function performLineLabel(trigger, target) {
      const slot = trigger.closest('[data-speaker-slot]');
      const remove = target.kind === 'guess';
      const segmentDay = trigger.dataset.day || day;
      const stream = trigger.dataset.stream || selectedSegment?.stream || '';
      const segmentKey = trigger.dataset.segmentKey || selectedSegment?.key || '';
      const route = `/app/transcripts/api/line-label/${encodeURIComponent(segmentDay)}/${encodeURIComponent(stream)}/${encodeURIComponent(segmentKey)}${remove ? '/remove' : ''}`;
      const body = {
        source: trigger.dataset.speakerSource || '',
        sentence_id: Number(trigger.dataset.sentenceId)
      };
      if (!remove) {
        body.speaker = target.kind === 'person' ? { kind: 'person', id: target.entityId } : { kind: target.kind };
      }
      trigger.disabled = true;
      trigger.setAttribute('aria-busy', 'true');
      try {
        await apiJson(route, {
          method: 'POST',
          headers: { 'Content-Type': 'application/json', 'x-solstone-actor': 'owner' },
          body: JSON.stringify(body)
        });
        await loadSegmentContent(selectedSegment, activeTab);
      } catch (err) {
        const copyKey = LINE_LABEL_REFUSALS[err?.reasonCode] || 'TR_SPEAKER_LINE_REFUSED';
        renderSpeakerStatus(slot, trCopy(copyKey) || window.SurfaceState.serverMessageFrom(err), { error: true, copyKey });
      } finally {
        trigger.disabled = false;
        trigger.removeAttribute('aria-busy');
      }
    }

    function attachSpeakerPickerHandlers(root) {
      root.querySelectorAll('[data-action="speaker-picker"]').forEach(button => {
        button.addEventListener('click', e => {
          e.preventDefault();
          e.stopPropagation();
          openSpeakerPicker(button);
        });
      });
    }

    // What Granola wrote about the meeting: context for the owner, kept apart
    // from the transcript so nothing in it reads as said aloud.
    function renderGranolaContext(note) {
      const parts = [];
      if (note.summary) {
        parts.push(`<details class="tr-granola-part"><summary>${escapeHtml(GRANOLA_COPY.summary)}</summary>`
          + `<div class="tr-granola-text">${window.AppServices.renderMarkdown(note.summary) || ''}</div></details>`);
      }
      if (note.private_notes) {
        parts.push(`<details class="tr-granola-part" open><summary>${escapeHtml(GRANOLA_COPY.privateNotes)}</summary>`
          + `<div class="tr-granola-text">${window.AppServices.renderMarkdown(note.private_notes) || ''}</div></details>`);
      }
      const facts = [];
      if (note.calendar_title) {
        facts.push(`<dt>${escapeHtml(GRANOLA_COPY.calendar)}</dt><dd>${escapeHtml(note.calendar_title)}</dd>`);
      }
      if (Array.isArray(note.attendees) && note.attendees.length > 0) {
        facts.push(`<dt>${escapeHtml(GRANOLA_COPY.attendees)}</dt><dd>${note.attendees.map(escapeHtml).join(', ')}</dd>`);
      }
      if (facts.length > 0) parts.push(`<dl class="tr-granola-facts">${facts.join('')}</dl>`);
      const link = note.web_url
        ? `<a class="tr-granola-open" href="${escapeHtml(note.web_url)}" target="_blank" rel="noopener noreferrer">${escapeHtml(GRANOLA_COPY.open)}</a>`
        : '';
      return `<section class="tr-granola" aria-label="${escapeHtml(GRANOLA_COPY.heading)}">`
        + `<div class="tr-granola-head"><span class="tr-granola-label">${escapeHtml(GRANOLA_COPY.heading)}</span>${link}</div>`
        + parts.join('')
        + '</section>';
    }

    // ----- Written messages -----
    // A message shows who sent it, to whom and when, and nothing a spoken
    // transcript carries. Fields the source didn't establish stay off.
    let utf8 = null;

    // Block offsets are half-open UTF-8 byte ranges in the message text.
    function utf8Slice(bytes, start, end) {
      return utf8.decoder.decode(bytes.subarray(Math.max(0, start), Math.min(bytes.length, end)));
    }

    function messageBlocks(message, text) {
      utf8 ??= { encoder: new TextEncoder(), decoder: new TextDecoder() };
      const bytes = utf8.encoder.encode(text);
      const blocks = Array.isArray(message.blocks) && message.blocks.length
        ? message.blocks
        : [{ kind: 'paragraph', start: 0, end: bytes.length }];
      return blocks.map(block => ({ kind: block.kind, start: block.start, end: block.end, text: utf8Slice(bytes, block.start, block.end) }));
    }

    function messageBlockHtml(block) {
      if (block.kind === 'code') {
        // A fenced block's fences are its delimiters, not its contents.
        const lines = block.text.split('\n');
        if (/^(```|~~~)/.test(lines[0] || '')) {
          lines.shift();
          if (/^(```|~~~)\s*$/.test(lines[lines.length - 1] || '')) lines.pop();
        }
        return `<pre class="tr-msg-code"><code>${escapeHtml(lines.join('\n'))}</code></pre>`;
      }
      const html = escapeHtml(block.text.trim()).replace(/\n/g, '<br>');
      return block.kind === 'quote' ? `<blockquote class="tr-msg-quote">${html}</blockquote>` : `<p>${html}</p>`;
    }

    // A message's own send time. On another day than the one shown, its date too.
    function messageSentLabel(sentAt) {
      const when = new Date(sentAt);
      if (Number.isNaN(when.getTime())) return '';
      const parts = window.JournalClock ? window.JournalClock.parts(when) : null;
      const sentDay = parts ? `${parts.year}${parts.month}${parts.day}` : '';
      return sentDay && sentDay !== day ? window.JournalFormat.timestamp(when) : clockOf(when);
    }

    // Recipients, when the source named them. A message to one person inside
    // a conversation addressed to everyone went to that person alone.
    function messageToLine(message, groupAudience) {
      const to = Array.isArray(message.to) ? message.to.filter(name => typeof name === 'string' && name.trim()) : [];
      if (!to.length) return '';
      if (to.length === 1 && to[0].trim().toLowerCase() === 'everyone') {
        return window.JournalI18n.render('journal.window.transcripts.message.to.everyone');
      }
      if (to.length === 1 && groupAudience) {
        return window.JournalI18n.render('journal.window.transcripts.message.to.only', [['recipient', to[0]]]);
      }
      const names = typeof Intl.ListFormat === 'function'
        ? new Intl.ListFormat(window.JournalI18n.formatLocale, { type: 'conjunction' }).format(to)
        : to.join(', ');
      return window.JournalI18n.render('journal.window.transcripts.message.to', [['recipients', names]]);
    }

    function messageAddressedToEveryone(message) {
      return Array.isArray(message?.to) && message.to.length === 1 && String(message.to[0]).trim().toLowerCase() === 'everyone';
    }

    // Where a search opened this item: the matched turn, and the byte offset
    // of the match inside it when the journal could find one.
    function linkedPassage() {
      const params = new URLSearchParams(window.location.search);
      const turn = params.get('turn');
      if (!turn) return null;
      const at = params.get('at');
      const offset = at !== null && /^\d+$/.test(at) ? Number(at) : null;
      return { turn, offset };
    }

    function wordCount(text) {
      return (text.match(/\S+/g) || []).length;
    }

    function renderMessageEntry(item, idx, groupAudience, passage) {
      const message = item.message;
      const text = String(item.text || '');
      const who = message.is_owner === true
        ? window.JournalI18n.render('journal.window.transcripts.message.you')
        : (message.author || '');
      const sent = message.sent_at ? messageSentLabel(message.sent_at) : '';
      const to = messageToLine(message, groupAudience);
      const blocks = messageBlocks(message, text);
      let fold = -1;
      if (passage && passage.turn === message.turn_id && passage.offset !== null && passage.offset > 0) {
        fold = blocks.findIndex(block => passage.offset >= block.start && passage.offset < block.end);
        if (fold <= 0) fold = -1;
      }
      let html = `<div class="tr-msg" data-idx="${idx}" data-type="message" data-turn-id="${escapeHtml(message.turn_id || '')}" role="listitem" tabindex="0">`;
      if (who || sent) {
        html += '<div class="tr-msg-who">';
        if (who) html += `<b>${escapeHtml(who)}</b>`;
        if (sent) html += `<time datetime="${escapeHtml(message.sent_at)}">${escapeHtml(sent)}</time>`;
        html += '</div>';
      }
      if (to) html += `<div class="tr-msg-to">${escapeHtml(to)}</div>`;
      html += '<div class="tr-msg-body">';
      if (fold > 0) {
        const before = blocks.slice(0, fold);
        const words = wordCount(before.map(block => block.text).join(' '));
        html += '<div class="tr-msg-fold">';
        html += `<span>${window.JournalI18n.renderHtml('journal.window.transcripts.message.words.before', [['words_quantity', window.JournalI18n.cardinalQuantity(words)], ['words', words.toLocaleString(window.JournalI18n.formatLocale)]])}</span> `;
        html += `<button type="button" class="tr-msg-unfold">${window.JournalI18n.renderHtml('journal.window.transcripts.message.show.from.start')}</button>`;
        html += '</div>';
        html += `<div class="tr-msg-earlier" hidden>${before.map(messageBlockHtml).join('')}</div>`;
        html += `<div class="tr-msg-passage">${blocks.slice(fold).map(messageBlockHtml).join('')}</div>`;
      } else {
        html += blocks.map(messageBlockHtml).join('');
      }
      html += '</div>';
      if (message.truncated === true) {
        html += `<p class="tr-msg-cut">${window.JournalI18n.renderHtml('journal.window.transcripts.message.truncated')}</p>`;
      }
      html += '</div>';
      return html;
    }

    // An older conversation the journal can't place says so, and why.
    function renderUnresolvedNotice(seg) {
      const item = conversationItem(seg);
      if (conversationKind(seg) !== 'conversation') return '';
      // Absence only when the journal confirmed it; no usable record of the
      // original is its own case; otherwise the kept file was inconclusive.
      const reason = {
        original_missing: 'journal.window.transcripts.conversation.reason.original.missing',
        origin_unknown: 'journal.window.transcripts.conversation.reason.origin.unknown',
      }[item.reason_code] || 'journal.window.transcripts.conversation.reason.insufficient.evidence';
      return '<div class="tr-unresolved">'
        + `<p>${window.JournalI18n.renderHtml('journal.window.transcripts.conversation.unresolved')}</p>`
        + `<details class="tr-unresolved-why"><summary>${window.JournalI18n.renderHtml('journal.window.transcripts.conversation.why')}</summary>`
        + `<p>${window.JournalI18n.renderHtml(reason)}</p></details></div>`;
    }

    // Other saved conversations whose time on the day overlaps this one: a
    // fact about time, not a claim they are one meeting. An item with no send
    // time has no time to overlap.
    function overlappingConversations(seg) {
      if (!conversationItem(seg) || isDateOnlyImport(seg)) return [];
      const spanOf = other => [parseTime(other.start), parseTime(other.end)];
      const [start, end] = spanOf(seg);
      return allSegments.filter(other => other !== seg
        && !(other.key === seg.key && other.stream === seg.stream)
        && conversationItem(other)
        && !isDateOnlyImport(other)
        && (() => { const [s0, e0] = spanOf(other); return s0 < end && e0 > start; })());
    }

    function renderAlsoAtThisTime(seg) {
      const others = overlappingConversations(seg);
      if (!others.length) return '';
      const links = others.map(other => {
        const label = window.JournalI18n.render('journal.window.transcripts.also.item', [
          ['label', importItemLabel(other) || humanizeStreamName(other.stream)], ['span', importItemWhen(other)],
        ]);
        return `<a href="${escapeHtml(segmentLocation(other, 'transcript'))}" data-also-key="${escapeHtml(other.key)}" data-also-stream="${escapeHtml(other.stream)}">${escapeHtml(label)}</a>`;
      });
      return `<p class="tr-also">${window.JournalI18n.renderHtml('journal.window.transcripts.also.at.this.time')} ${links.join(' · ')}</p>`;
    }

    function wireMessagePane(targetEl) {
      targetEl.querySelectorAll('.tr-msg-unfold').forEach(button => {
        button.addEventListener('click', () => {
          const body = button.closest('.tr-msg-body');
          body.querySelector('.tr-msg-earlier').hidden = false;
          button.closest('.tr-msg-fold').remove();
        });
      });
      targetEl.querySelectorAll('.tr-also a').forEach(link => {
        link.addEventListener('click', event => {
          const other = allSegments.find(seg => seg.key === link.dataset.alsoKey && seg.stream === link.dataset.alsoStream);
          if (!other) return;
          event.preventDefault();
          selectSegment(other);
        });
      });
      const passage = linkedPassage();
      if (!passage) return;
      const entry = [...targetEl.querySelectorAll('.tr-msg[data-turn-id]')].find(el => el.dataset.turnId === passage.turn);
      if (!entry) return;
      revealBoundedEntry(targetEl, entry);
      entry.classList.add('tr-entry-active');
      (entry.querySelector('.tr-msg-passage') || entry).scrollIntoView?.({ block: 'start' });
    }

    function renderSegmentTimeline(data, showAudio, showScreen, targetEl, showBrowser = true, showImage = false) {
      const browserOnlyView = showBrowser && !showAudio && !showScreen && !showImage;
      const imageOnlyView = showImage && !showAudio && !showScreen && !showBrowser;
      const chunks = (data.chunks || []).filter(c => {
        if (browserOnlyView && c.type !== 'browser') return false;
        if (imageOnlyView && c.type !== 'image') return false;
        if (c.type === 'transcript' && (!showAudio || !showScreen)) return false;
        if (c.type === 'audio' && !showAudio) return false;
        if (c.type === 'screen' && !showScreen) return false;
        if (c.type === 'browser' && !showBrowser) return false;
        if (c.type === 'image' && !showImage) return false;
        return true;
      });
      const dataState = data.data_state || {};

      if (isBodySummarySegment() && showAudio && showScreen) {
        setPaneEntryCount(targetEl, null);
        targetEl.innerHTML = renderBodyContextCard(data);
        attachBodyWindowButtons(targetEl);
        return;
      }

      const textOnlyScreen = showScreen
        && Object.keys(currentVideoFiles).length === 0
        && Object.keys(currentImageFiles).length === 0;

      // Build flat list of all screen and image frames for modal navigation
      allScreenFrames = chunks.filter(c => c.type === 'screen' || c.type === 'image');
      currentFrameIndex = -1;

      const granolaNotes = data.granola && showAudio ? renderGranolaContext(data.granola) : '';
      if (chunks.length === 0 && granolaNotes) {
        setPaneEntryCount(targetEl, 0);
        targetEl.innerHTML = granolaNotes
          + `<p class="tr-granola-empty">${escapeHtml(GRANOLA_COPY.noTranscript)}</p>`;
        return;
      }
      if (chunks.length === 0) {
        setPaneEntryCount(targetEl, 0);
        if (showAudio && !showScreen && !showImage && dataState.audio && dataState.audio !== 'analyzed') {
          targetEl.innerHTML = renderDataStateAffordance('audio', dataState.audio, data);
          return;
        }
        if (!showAudio && showScreen && !showImage && dataState.screen && dataState.screen !== 'analyzed') {
          targetEl.innerHTML = renderDataStateAffordance('screen', dataState.screen, data);
          return;
        }
        if (!showAudio && !showScreen && showImage && dataState.image && dataState.image !== 'analyzed') {
          targetEl.innerHTML = renderDataStateAffordance('image', dataState.image, data);
          return;
        }
        if (showAudio && showScreen && showImage) {
          const states = Object.keys(dataState)
            .filter(modality => dataState[modality] && dataState[modality] !== 'analyzed')
            .map(modality => renderDataStateAffordance(modality, dataState[modality], data))
            .filter(Boolean);
          if (states.length > 0) {
            targetEl.innerHTML = `<div class="tr-state-stack">${states.join('')}</div>`;
            return;
          }
        }
        const tabType = imageOnlyView ? 'image' : browserOnlyView ? 'browser' : !showScreen ? 'audio' : !showAudio ? 'screen' : 'transcript';
        const tabEmptyMap = {
          transcript: { icon: window.ConveyIcons.svg('file-text'), heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.no.transcript.entries'), desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.this.segment.has.no.transcript.content') },
          audio: { icon: window.ConveyIcons.svg('volume-2'), heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.no.audio.entries'), desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.this.segment.has.no.audio.content') },
          screen: { icon: window.ConveyIcons.svg('monitor'), heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.no.screen.entries'), desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.this.segment.has.no.screen.observations') },
          browser: { icon: window.ConveyIcons.svg('file-text'), heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.no.page.entries'), desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.this.segment.has.no.page.content') },
          image: { icon: window.ConveyIcons.svg('eye'), heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.no.photos'), desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.this.segment.has.no.photos') }
        };
        const emptyInfo = tabEmptyMap[tabType] || tabEmptyMap.transcript;
        targetEl.innerHTML = window.SurfaceState.empty(emptyInfo);
        return;
      }

      // Group sequential basic screen frames together
      const displayItems = textOnlyScreen ? chunks : groupBasicScreenFrames(chunks);
      groupEntriesByIdx = new Map();
      setPaneEntryCount(targetEl, displayItems.length);

      const conversationHead = selectedSegment && showAudio && showScreen
        ? renderUnresolvedNotice(selectedSegment) + renderAlsoAtThisTime(selectedSegment)
        : '';
      const groupAudience = displayItems.some(item => messageAddressedToEveryone(item.message));
      const passage = linkedPassage();
      let html = renderBodyStrip(data.body_window) + granolaNotes + conversationHead;
      html += `<div class="tr-unified" role="list" aria-label="${window.JournalI18n.renderHtml('journal.window.transcripts.entries.quantity', transcriptCount('items', displayItems.length))}">`;

      // Audio player section (if we have audio)
      if (data.audio_file && showAudio) {
        const audioExt = (data.audio_file.split('.').pop() || '').toLowerCase();
        const audioMime = ({flac:'audio/flac', m4a:'audio/mp4', mp4:'audio/mp4', mp3:'audio/mpeg', wav:'audio/wav', ogg:'audio/ogg'})[audioExt] || '';
        const audioDuration = formatAudioDuration(data.duration);
        const audioLabel = audioDuration ? window.JournalI18n.render('journal.window.transcripts.media.audio.duration', [['duration', audioDuration]]) : window.JournalI18n.render('journal.window.transcripts.media.audio.label');
        html += '<div class="tr-audio-players" role="presentation">';
        html += '<div class="tr-audio-player">';
        html += `<div class="tr-audio-player-label">${escapeHtml(audioLabel)}</div>`;
        html += `<audio data-role="segment-audio" controls preload="metadata"><source src="${escapeHtml(data.audio_file)}" type="${audioMime}">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.your.browser.does.not.support.audio')}</audio>`;
        html += '</div></div>';
      }

      const audioPurged = data.media_purged?.audio && !data.audio_file && showAudio;
      const screenPurged = data.media_purged?.screen && showScreen && chunks.some(c => c.type === 'screen');
      const purgeNotice = (modality) => `<div class="tr-purge-notice" role="presentation">${escapeHtml(data.media_removal[modality])}`
        + (mediaInTrash(data, modality) ? ` · ${mediaTrashLink()}` : '') + '</div>';
      if (audioPurged) html += purgeNotice('audio');
      if (screenPurged) html += purgeNotice('screen');

      // Render items (chunks or groups)
      displayItems.forEach((item, idx) => {
        if (item.type === 'screen-group') {
          groupEntriesByIdx.set(idx, item.entries || []);
          // Render collapsed group of basic frames
          html += renderScreenGroup(item, idx);
        } else if (item.type === 'audio') {
          const timeStr = item.time || '';
          let audioAttrs = '';
          if (item.audio_url && item.audio_origin !== undefined) {
            audioAttrs = ` data-audio-url="${escapeHtml(item.audio_url)}" data-audio-origin="${item.audio_origin}"`;
          }
          html += `<div class="tr-entry tr-entry-audio" data-idx="${idx}" data-type="audio" data-timestamp="${item.timestamp}"${audioAttrs} role="listitem" tabindex="0" aria-label="${window.JournalI18n.renderHtml('journal.window.transcripts.entry.play.from.time', [['time', timeStr]])}">`;
          html += `<div class="tr-entry-time">${timeStr}</div>`;
          html += '<div class="tr-entry-content">';
          html += `<span class="sr-only">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.audio.variant2')} </span>`;
          html += `<div class="tr-speaker-slot" data-speaker-slot>${renderSpeakerSlot(item, idx, data)}</div>`;
          html += `<div class="tr-entry-text">${renderAudioText(item)}</div>`;
          html += '</div></div>';
        } else if (item.type === 'transcript' && item.message) {
          html += renderMessageEntry(item, idx, groupAudience, passage);
        } else if (item.type === 'markdown' || item.type === 'transcript') {
          // A caption cue's clock arrives with milliseconds; the line shows seconds.
          const timeStr = escapeHtml(String(item.time || '').replace(/^(\d{2}:\d{2}:\d{2})\.\d+$/, '$1'));
          const speaker = item.speaker ? `<strong>${escapeHtml(item.speaker)}:</strong> ` : '';
          const text = item.type === 'transcript'
            ? speaker + escapeHtml(item.text || '').replace(/\n/g, '<br>')
            : window.AppServices.renderMarkdown(item.markdown) || '';
          html += `<div class="tr-entry tr-entry-transcript" data-idx="${idx}" data-type="${escapeHtml(item.type)}" data-timestamp="${item.timestamp ?? ''}" role="listitem" tabindex="0">`;
          html += `<div class="tr-entry-time">${timeStr}</div>`;
          html += '<div class="tr-entry-content">';
          html += `<div class="tr-entry-text">${text}</div>`;
          html += '</div></div>';
        } else if (item.type === 'calendar') {
          // A calendar note reads as plain text, one field per line.
          const timeStr = item.time || '';
          const text = escapeHtml(item.text || '').replace(/\n/g, '<br>');
          html += `<div class="tr-entry tr-entry-transcript" data-idx="${idx}" data-type="calendar" data-timestamp="${item.timestamp}" role="listitem" tabindex="0">`;
          html += `<div class="tr-entry-time">${timeStr}</div>`;
          html += '<div class="tr-entry-content">';
          html += `<div class="tr-entry-text">${text}</div>`;
          html += '</div></div>';
        } else if (item.type === 'browser') {
          const timeStr = item.time || '';
          const markdown = window.AppServices.renderMarkdown(item.markdown) || '';
          const source = item.source_ref || {};
          const siteName = source.site_name || source.site || source.file || window.JournalI18n.render('journal.window.transcripts.kind.pages');
          const title = source.title || '';
          html += `<div class="tr-entry tr-entry-browser" data-idx="${idx}" data-type="browser" data-timestamp="${item.timestamp}" role="listitem">`;
          html += `<div class="tr-entry-time">${timeStr}</div>`;
          html += '<div class="tr-entry-content">';
          html += `<span class="sr-only">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.pages')} </span>`;
          html += `<div class="tr-browser-site">${escapeHtml(siteName)}${title ? `<span class="tr-browser-title">${escapeHtml(title)}</span>` : ''}</div>`;
          html += `<div class="tr-entry-text">${markdown}</div>`;
          html += '</div></div>';
        } else if (item.type === 'image') {
          html += renderPhotoEntry(item, idx);
        } else if (item.type === 'screen' && item.screenshot) {
          html += renderScreenshotEntry(item, idx);
        } else if (item.type === 'screen') {
          if (textOnlyScreen) {
            const timeStr = item.time || '';
            const markdown = renderScreenChunkMarkdown(item.markdown) || window.JournalI18n.render('journal.window.transcripts.media.screen.activity');
            html += '<div class="tr-entry" role="listitem">';
            html += `<div class="tr-entry-time">${timeStr}</div>`;
            html += '<div class="tr-entry-content">';
            html += `<span class="sr-only">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.screen.variant2')} </span>`;
            html += `<div class="tr-screen-text">${markdown}</div>`;
            html += '</div></div>';
          } else {
            // Enhanced screen frame - render fully
            html += renderEnhancedScreenEntry(item, idx);
          }
        }
      });

      html += '</div>';
      targetEl.innerHTML = html;
      wireMessagePane(targetEl);
      attachBodyWindowButtons(targetEl);
      attachSpeakerPickerHandlers(targetEl);
      trimHeadingMilliseconds(targetEl);
      boundEntryList(targetEl);
      collapseLongPreBlocks(targetEl);
      // A pre inside a closed disclosure has no measurable height, so re-check
      // once one opens. 'toggle' does not bubble; listen in the capture phase.
      targetEl.addEventListener('toggle', () => collapseLongPreBlocks(targetEl), true);

      // Get audio element reference
      const paneAudioEl = targetEl.querySelector('audio[data-role="segment-audio"]');
      // Where a link's moment set the player, until the owner plays something.
      let cueSeconds = null;
      if (paneAudioEl) {
        paneAudioEl.addEventListener('loadedmetadata', () => {
          if (Number.isFinite(paneAudioEl.duration) && paneAudioEl.duration > 0) {
            return;
          }

          const handleDurationChange = () => {
            if (!Number.isFinite(paneAudioEl.duration) || paneAudioEl.duration === 0) {
              return;
            }
            // Back to the start, or to the moment a link cued meanwhile.
            paneAudioEl.currentTime = cueSeconds ?? 0;
            paneAudioEl.removeEventListener('durationchange', handleDurationChange);
          };

          paneAudioEl.addEventListener('durationchange', handleDurationChange);
          paneAudioEl.currentTime = 1e7;
        }, { once: true });
      }

      // Set the player to an entry's line: played on a click, only cued for a
      // link's moment.
      const seekToEntry = (entry, play) => {
        if (!paneAudioEl) return;
        const audioUrl = entry.dataset.audioUrl;
        const audioOrigin = entry.dataset.audioOrigin;
        const timestamp = parseInt(entry.dataset.timestamp, 10);
        let offsetSec;
        if (audioUrl && audioOrigin !== undefined) {
          offsetSec = Math.max(0, (timestamp - parseInt(audioOrigin, 10)) / 1000);
          if (paneAudioEl.getAttribute('src') !== audioUrl && paneAudioEl.src !== audioUrl) {
            paneAudioEl.src = audioUrl;
          }
        } else if (segmentData?.audio_file) {
          const baseTimestamp = chunks[0]?.timestamp || timestamp;
          offsetSec = Math.max(0, (timestamp - baseTimestamp) / 1000);
        } else {
          return;
        }
        paneAudioEl.currentTime = offsetSec;
        cueSeconds = play ? null : offsetSec;
        if (play) paneAudioEl.play();
      };

      // Add click handlers for audio entries to seek
      targetEl.querySelectorAll('.tr-entry-audio').forEach(entry => {
        entry.addEventListener('click', e => {
          if (e.target.closest('[data-action="speaker-picker"]')) return;
          seekToEntry(entry, true);
        });
      });

      // Add keyboard handler for audio entries
      targetEl.querySelectorAll('.tr-entry-audio').forEach(entry => {
        entry.addEventListener('keydown', e => {
          if (e.target !== entry) return;
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            entry.click();
          }
        });
      });

      // Playback highlight: track current entry during audio playback
      let activeEntry = null;
      if (paneAudioEl) {
        const baseTimestamp = chunks[0]?.timestamp || 0;

        paneAudioEl.addEventListener('timeupdate', () => {
          const currentSrc = paneAudioEl.getAttribute('src') || paneAudioEl.src || paneAudioEl.querySelector('source')?.getAttribute('src') || '';
          const entries = targetEl.querySelectorAll('.tr-entry-audio[data-timestamp]');
          let best = null;
          for (const el of entries) {
            const elUrl = el.dataset.audioUrl;
            const elOrigin = el.dataset.audioOrigin;
            if (elUrl && elOrigin !== undefined) {
              if (currentSrc && !currentSrc.endsWith(elUrl) && currentSrc !== elUrl) {
                continue;
              }
              const ts = parseInt(el.dataset.timestamp, 10);
              const origin = parseInt(elOrigin, 10);
              const currentMs = origin + (paneAudioEl.currentTime * 1000);
              if (ts <= currentMs) best = el;
              else if (best) break;
            } else {
              const currentMs = baseTimestamp + (paneAudioEl.currentTime * 1000);
              const ts = parseInt(el.dataset.timestamp, 10);
              if (ts <= currentMs) best = el;
              else break;
            }
          }
          if (best === activeEntry) return;
          if (activeEntry) activeEntry.classList.remove('tr-entry-active');
          activeEntry = best;
          if (activeEntry) {
            revealBoundedEntry(targetEl, activeEntry);
            activeEntry.classList.add('tr-entry-active');
            activeEntry.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
          }
        });

        paneAudioEl.addEventListener('ended', () => {
          if (activeEntry) {
            activeEntry.classList.remove('tr-entry-active');
            activeEntry = null;
          }
        });
      }

      // A link's moment: bring the line being said at that second into view,
      // marked, with the player set there. Nothing plays until the owner asks.
      // The match is on the clock each line shows, the one an agent reads; a
      // re-transcribed recording cues the line that now holds that second.
      const moment = selectedSegment && momentFor(selectedSegment);
      if (moment && !moment.cued) {
        let cue = null;
        for (const entry of targetEl.querySelectorAll('.tr-entry-audio[data-timestamp]')) {
          const clock = String(displayItems[entry.dataset.idx]?.time || '').replace(/:/g, '');
          if (!/^\d{6}$/.test(clock)) continue;
          if (clock > moment.clock) break;
          cue = entry;
        }
        if (cue) {
          moment.cued = true;
          revealBoundedEntry(targetEl, cue);
          activeEntry = cue;
          cue.classList.add('tr-entry-active');
          // This pane isn't showing yet: activateTab brings the line into
          // view once it is.
          moment.reveal = { pane: targetEl, entry: cue };
          seekToEntry(cue, false);
        }
      }

      // Add click handlers for enhanced screen and image entries to open modal
      targetEl.querySelectorAll('.tr-entry-screen, .tr-entry-image').forEach(entry => {
        // A screenshot's image sits in its own button; other thumbnails are
        // the target themselves.
        const thumb = entry.querySelector('.tr-entry-shot-open') || entry.querySelector('.tr-entry-thumb');
        if (thumb) {
          thumb.addEventListener('click', (e) => {
            e.stopPropagation();
            const frameIdx = parseInt(entry.dataset.frameIdx, 10);
            if (!isNaN(frameIdx)) openImageModal(frameIdx);
          });
        }
      });

      // Add click handlers for group headers to expand/collapse
      targetEl.querySelectorAll('.tr-group-header').forEach(header => {
        header.addEventListener('click', () => {
          const groupEl = header.parentElement;
          const isExpanded = groupEl.classList.toggle('expanded');
          header.setAttribute('aria-expanded', String(isExpanded));
          if (!isExpanded) return;
          if (groupEl.dataset.prefetched === 'true') return;
          const groupIdx = parseInt(groupEl.dataset.idx, 10);
          if (isNaN(groupIdx)) return;
          const entries = groupEntriesByIdx.get(groupIdx) || [];
          prefetchGroupThumbnails(
            entries,
            groupEl,
            targetEl,
            currentSegmentAbort?.signal
          );
        });
        header.addEventListener('keydown', e => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            header.click();
          }
        });
      });

      // Add click handlers for group grid items to open modal
      targetEl.querySelectorAll('.tr-group-item').forEach(item => {
        item.addEventListener('click', () => {
          const frameIdx = parseInt(item.dataset.frameIdx, 10);
          if (!isNaN(frameIdx)) openImageModal(frameIdx);
        });
        item.addEventListener('keydown', e => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            item.click();
          }
        });
      });

      // Set up lazy loading for canvas thumbnails using IntersectionObserver
      if (!textOnlyScreen) {
        setupLazyCanvasLoading(targetEl);
      }
    }

    function prefetchGroupThumbnails(entries, groupEl, targetEl, signal) {
      const frameIdsByVideo = new Map();
      for (const entry of entries) {
        const filename = entry.source_ref?.filename;
        const frameId = entry.source_ref?.frame_id;
        if (!filename || !frameId) continue;
        if (!frameIdsByVideo.has(filename)) {
          frameIdsByVideo.set(filename, new Set());
        }
        frameIdsByVideo.get(filename).add(frameId);
      }

      const jobs = [];
      for (const [filename, frameIds] of frameIdsByVideo.entries()) {
        const url = currentVideoFiles[filename];
        if (!url) continue;
        jobs.push(
          frameCapture.prefetchThumbnails(url, Array.from(frameIds), null, signal)
        );
      }

      if (jobs.length > 0) {
        groupEl.dataset.prefetched = 'true';
        Promise.all(jobs).finally(() => {
          setupLazyCanvasLoading(targetEl);
        });
      }
    }

    // Lazy load canvas thumbnails when they become visible
    function setupLazyCanvasLoading(targetEl = panel) {
      const canvases = targetEl.querySelectorAll('canvas[data-video-url]');
      if (canvases.length === 0) return;

      const observer = new IntersectionObserver((entries) => {
        for (const entry of entries) {
          if (entry.isIntersecting) {
            const canvas = entry.target;
            observer.unobserve(canvas);
            loadCanvasThumbnail(canvas);
          }
        }
      }, { rootMargin: '100px' });

      canvases.forEach(canvas => observer.observe(canvas));
    }

    // Load a single canvas thumbnail
    function loadCanvasThumbnail(canvas) {
      const videoUrl = canvas.dataset.videoUrl;
      const frameId = parseInt(canvas.dataset.frameId, 10);

      if (!videoUrl || isNaN(frameId)) {
        canvas.classList.remove('loading');
        return;
      }

      // Draw at thumbnail size (120x68) - no overlays on thumbnails
      frameCapture.drawThumbnail(canvas, videoUrl, frameId, {
        width: 120,
        height: 68
      });
    }

    function groupBasicScreenFrames(chunks) {
      // Group sequential basic screen frames, keep audio and enhanced screens separate
      const result = [];
      let currentGroup = null;

      for (const chunk of chunks) {
        const isBasicScreen = chunk.type === 'screen' && chunk.basic === true && !chunk.screenshot;

        if (isBasicScreen) {
          // Add to current group or start new one
          if (!currentGroup) {
            currentGroup = {
              type: 'screen-group',
              entries: [],
              startTime: chunk.time,
              endTime: chunk.time
            };
          }
          currentGroup.entries.push(chunk);
          currentGroup.endTime = chunk.time;
        } else {
          // Flush any pending group
          if (currentGroup) {
            result.push(currentGroup);
            currentGroup = null;
          }
          result.push(chunk);
        }
      }

      // Flush final group
      if (currentGroup) {
        result.push(currentGroup);
      }

      return result;
    }

    function findFrameIndex(chunk) {
      // Find this chunk's index in allScreenFrames by matching source_ref
      const ref = chunk.source_ref;
      if (!ref) return -1;
      const raw = ref?.raw || '';
      return allScreenFrames.findIndex(f => {
        const source = f.source_ref;
        if (!source) return false;
        if (ref.frame_id != null) {
          return source.filename === ref.filename && source.frame_id === ref.frame_id
            && (!raw || source.raw === raw);
        }
        // A raw path alone identifies a still image, not a frame in a video.
        return !!raw && source.frame_id == null && source.raw === raw;
      });
    }

    // Get video URL for a chunk
    function getVideoUrlForChunk(chunk) {
      const filename = chunk.source_ref?.filename;
      return filename ? currentVideoFiles[filename] : null;
    }

    function getImageUrlForChunk(chunk) {
      const raw = chunk.source_ref?.raw;
      return raw ? currentImageFiles[raw] : null;
    }

    function renderScreenGroup(group, idx) {
      const count = group.entries.length;
      const timeRange = group.startTime === group.endTime
        ? group.startTime
        : `${group.startTime} - ${group.endTime}`;
      const countText = window.JournalI18n.render('journal.window.transcripts.media.frame.count', transcriptCount('frames', count));

      let html = `<div class="tr-group" data-idx="${idx}" role="listitem">`;
      html += `<div class="tr-group-header" role="button" tabindex="0" aria-expanded="false" aria-controls="tr-group-grid-${idx}">`;
      html += `<svg class="tr-group-chevron" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 18l6-6-6-6"/></svg>`;
      html += `<span class="tr-group-time">${timeRange}</span>`;
      html += `<span class="tr-group-count">${countText}</span>`;
      html += '</div>';

      // Grid of thumbnails (hidden until expanded)
      html += `<div class="tr-group-grid" id="tr-group-grid-${idx}">`;
      for (const entry of group.entries) {
        const videoUrl = getVideoUrlForChunk(entry);
        const imageUrl = getImageUrlForChunk(entry);
        const frameId = entry.source_ref?.frame_id;
        const analysis = entry.source_ref?.analysis || {};
        const category = analysis.primary || 'unknown';
        const description = analysis.visual_description || category;
        const frameIdx = findFrameIndex(entry);

        if (imageUrl) {
          html += `<div class="tr-group-item" data-frame-idx="${frameIdx}" title="${escapeHtml(description)}" role="button" tabindex="0">`;
          html += `<img src="${escapeHtml(imageUrl)}" alt="${escapeHtml(description || window.JournalI18n.render('journal.window.transcripts.status.text.screen.image'))}">`;
          html += `<span class="tr-group-item-badge">${escapeHtml(category)}</span>`;
          html += '</div>';
        } else if (videoUrl && frameId) {
          html += `<div class="tr-group-item" data-frame-idx="${frameIdx}" title="${escapeHtml(description)}" role="button" tabindex="0">`;
          html += `<canvas class="loading" data-video-url="${escapeHtml(videoUrl)}" data-frame-id="${frameId}"></canvas>`;
          html += `<span class="tr-group-item-badge">${escapeHtml(category)}</span>`;
          html += '</div>';
        }
      }
      html += '</div>';

      html += '</div>';
      return html;
    }

    // A screenshot in its moment: the image at its own shape beside its text,
    // or, until its text is searchable, where that stands.
    // A screen entry's text opens with its own time as a heading, which the
    // time column already shows. Drop that one line, so every heading the text
    // itself carries can show.
    function withoutLeadingTimeHeading(markdown) {
      return String(markdown || '').replace(/^\s*#{1,6}[ \t]+\d{1,2}:\d{2}(?::\d{2}(?:\.\d{1,3})?)?[ \t]*(?:\r?\n|$)/, '');
    }

    // Each drawn screenshot label gets its own id: the same entry can be drawn
    // in more than one tab.
    let shotLabelSeq = 0;

    function renderPhotoEntry(chunk, idx) {
      const photo = chunk.photo || null;
      const timeStr = chunk.time || '';
      const imageUrl = getImageUrlForChunk(chunk);
      const frameIdx = findFrameIndex(chunk);
      const isShot = Boolean(photo && photo.is_screenshot);
      const label = isShot ? SCREENSHOT_COPY.J1 : PHOTO_COPY.label;
      const caption = photo && typeof photo.caption === 'string' && photo.caption.trim()
        ? photo.caption.trim()
        : '';
      const description = photo
        ? (typeof photo.description === 'string' && photo.description.trim() ? photo.description.trim() : '')
        : (chunk.markdown || '');
      const altText = caption || description || label;
      const size = photo && photo.width && photo.height
        ? ` width="${Number(photo.width)}" height="${Number(photo.height)}"`
        : '';

      shotLabelSeq += 1;
      const labelId = `tr-photo-label-${shotLabelSeq}`;
      let html = `<div class="tr-entry tr-entry-image tr-entry-photo" data-idx="${idx}" data-frame-idx="${frameIdx}" data-type="image" role="listitem">`;
      html += `<div class="tr-entry-time">${timeStr}</div>`;
      html += '<div class="tr-entry-content">';
      if (imageUrl) {
        html += `<button type="button" class="tr-entry-shot-open" aria-labelledby="${labelId}">`
          + `<img class="tr-entry-thumb tr-entry-shot" src="${escapeHtml(imageUrl)}" alt="${escapeHtml(altText)}"${size} loading="lazy">`
          + '</button>';
      }
      html += '<div class="tr-entry-desc">';
      html += `<div class="tr-shot-label" id="${labelId}"${imageUrl ? ' aria-hidden="true"' : ''}>${escapeHtml(label)}</div>`;
      if (photo && photo.time_source === 'upload_fallback') {
        html += `<p class="tr-photo-time-note" data-time-source="upload_fallback">${escapeHtml(PHOTO_COPY.uploadFallback)}</p>`;
      }
      if (caption) {
        const captionHtml = escapeHtml(caption).replace(/\n/g, '<br>');
        html += '<div class="tr-photo-caption" data-photo-part="caption">'
          + `<div class="tr-photo-part-label">${escapeHtml(PHOTO_COPY.captionLabel)}</div>`
          + `<p class="tr-photo-caption-text">${captionHtml}</p>`
          + '</div>';
      }
      if (photo) {
        if (photo.description_state === 'described' && description) {
          const descHtml = window.AppServices.renderMarkdown(description) || '';
          html += '<div class="tr-photo-description" data-photo-part="description">'
            + `<div class="tr-photo-part-label">${escapeHtml(PHOTO_COPY.descriptionLabel)}</div>`
            + `<div class="tr-image-text">${descHtml}</div>`
            + '</div>';
        } else if (photo.description_state === 'no_processing') {
          html += `<div class="tr-shot-final" data-photo-state="no_processing"><p>${escapeHtml(PHOTO_COPY.noProcessing)}</p>`
            + `<p class="tr-shot-action"><a href="/app/thinking/">${escapeHtml(PHOTO_COPY.noProcessingAction)}</a></p></div>`;
        } else if (photo.description_state === 'unavailable') {
          html += `<div class="tr-shot-final" data-photo-state="unavailable"><p>${escapeHtml(PHOTO_COPY.unavailable)}</p></div>`;
        }
      } else if (description) {
        const markdown = window.AppServices.renderMarkdown(description) || '';
        if (markdown) {
          html += `<div class="tr-image-text">${markdown}</div>`;
        }
      }
      html += '</div>';
      html += '</div></div>';
      return html;
    }

    function renderScreenshotEntry(chunk, idx) {
      const shot = chunk.screenshot || {};
      const timeStr = chunk.time || '';
      const imageUrl = getImageUrlForChunk(chunk);
      const frameIdx = findFrameIndex(chunk);
      const device = screenshotDevice(selectedSegment && selectedSegment.stream);
      const label = device ? `${SCREENSHOT_COPY.J1} · ${device}` : SCREENSHOT_COPY.J1;
      const searchable = shot.text === 'searchable';
      const description = searchable ? (chunk.source_ref?.analysis?.visual_description || '') : '';
      const alt = description || SCREENSHOT_COPY.J5;
      const size = shot.width && shot.height ? ` width="${Number(shot.width)}" height="${Number(shot.height)}"` : '';

      let html = `<div class="tr-entry tr-entry-screen tr-entry-screenshot" data-idx="${idx}" data-frame-idx="${frameIdx}" data-type="screen" role="listitem">`;
      html += `<div class="tr-entry-time">${timeStr}</div>`;
      html += '<div class="tr-entry-content">';
      // A screen reader hears J1 once: the visible label names the button that
      // opens the viewer, and is not read again on its own. With no image the
      // label is read where it stands.
      shotLabelSeq += 1;
      const labelId = `tr-shot-label-${shotLabelSeq}`;
      if (imageUrl) {
        html += `<button type="button" class="tr-entry-shot-open" aria-labelledby="${labelId}">`
          + `<img class="tr-entry-thumb tr-entry-shot" src="${escapeHtml(imageUrl)}" alt="${escapeHtml(alt)}"${size} loading="lazy">`
          + '</button>';
      }
      html += '<div class="tr-entry-desc">';
      html += `<div class="tr-shot-label" id="${labelId}"${imageUrl ? ' aria-hidden="true"' : ''}>${escapeHtml(label)}</div>`;
      if (searchable) {
        let markdown = withoutLeadingTimeHeading(chunk.markdown);
        // A raw screen chunk puts the analysis category before its caption.
        if (chunk.source_ref?.analysis) {
          markdown = markdown.replace(/^\r?\n\*\*Category:\*\* [^\r\n]*(?:\r?\n|$)/, '');
        }
        html += renderScreenChunkMarkdown(markdown);
      } else if (shot.text === 'no_processing') {
        html += `<div class="tr-shot-final"><p>${escapeHtml(SCREENSHOT_COPY.J6)}</p>`
          + `<p class="tr-shot-action"><a href="/app/thinking/">${escapeHtml(SCREENSHOT_COPY.J6_ACTION)}</a></p></div>`;
      } else if (shot.text === 'unreadable') {
        html += `<div class="tr-shot-final"><p>${escapeHtml(SCREENSHOT_COPY.J7)}</p>`
          + `<p class="tr-shot-action"><button type="button" class="tr-analyze-action" data-modality="screen">${escapeHtml(BTN_LABEL_FAILED)}</button></p></div>`;
      } else {
        html += `<p class="tr-shot-waiting">${escapeHtml(SCREENSHOT_COPY.J3)}</p>`;
      }
      html += '</div>';
      html += '</div></div>';
      return html;
    }

    function renderEnhancedScreenEntry(chunk, idx) {
      const timeStr = chunk.time || '';
      const monitor = chunk.source_ref?.monitor || '';
      const videoUrl = getVideoUrlForChunk(chunk);
      const imageUrl = getImageUrlForChunk(chunk);
      const frameId = chunk.source_ref?.frame_id;
      const frameIdx = findFrameIndex(chunk);

      let html = `<div class="tr-entry tr-entry-screen" data-idx="${idx}" data-frame-idx="${frameIdx}" data-type="screen" role="listitem">`;
      html += `<div class="tr-entry-time">${timeStr}</div>`;
      html += '<div class="tr-entry-content">';
      html += `<span class="sr-only">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.screen.variant2')} </span>`;

      if (imageUrl) {
        const description = chunk.source_ref?.analysis?.visual_description || window.JournalI18n.render('journal.window.transcripts.media.screen.image');
        html += `<img class="tr-entry-thumb" src="${escapeHtml(imageUrl)}" alt="${escapeHtml(description)}">`;
      } else if (videoUrl && frameId) {
        html += `<canvas class="tr-entry-thumb loading" data-video-url="${escapeHtml(videoUrl)}" data-frame-id="${frameId}"></canvas>`;
      }

      html += '<div class="tr-entry-desc">';
      if (monitor) {
        const monitorPos = getMonitorPosition(monitor);
        if (monitorPos) html += `<span class="tr-entry-badge tr-entry-badge-monitor">${monitorPos}</span>`;
      }
      if (chunk.markdown) {
        html += renderScreenChunkMarkdown(chunk.markdown);
      }
      html += '</div>';

      html += '</div></div>';
      return html;
    }

    // A screen chunk's markdown carries its payload in a fenced ```json block
    // (see core/crates/solstone-core-format/src/content/raw_screen.rs). Rendered
    // as-is, one five-minute block is dozens of full JSON dumps stacked down the
    // pane. Fold every block that actually parses behind a disclosure and leave
    // anything that does not parse exactly as it rendered before.
    const TR_JSON_FENCE = /```json[^\S\r\n]*\r?\n([\s\S]*?)\r?\n?```/g;

    // Only counts we can read straight off the parsed value. Nothing inferred:
    // a shape we do not recognise gets no summary rather than a guessed one.
    function screenJsonSummary(value) {
      if (!value || typeof value !== 'object' || Array.isArray(value)) return '';
      const parts = [];
      [['windows', 'window', 'windows'], ['panes', 'pane', 'panes']].forEach(([key, one, many]) => {
        const list = value[key];
        if (!Array.isArray(list) || list.length === 0) return;
        parts.push(`${list.length} ${list.length === 1 ? one : many}`);
      });
      return parts.join(', ');
    }

    function renderRawJsonDisclosure(raw, parsed) {
      const summary = screenJsonSummary(parsed);
      return '<details class="tr-raw-json">'
        + `<summary>${escapeHtml(summary ? window.JournalI18n.render('journal.window.transcripts.raw.json.summary', [['summary', String(summary)]]) : window.JournalI18n.render('journal.window.transcripts.status.text.raw.json'))}</summary>`
        + `<pre>${escapeHtml(raw)}</pre>`
        + '</details>';
    }

    function renderScreenChunkMarkdown(text) {
      const source = String(text || '');
      if (!source.includes('```json')) {
        return window.AppServices.renderMarkdown(source) || '';
      }

      let html = '';
      let cursor = 0;
      let match;
      TR_JSON_FENCE.lastIndex = 0;
      while ((match = TR_JSON_FENCE.exec(source)) !== null) {
        let parsed;
        try {
          parsed = JSON.parse(match[1]);
        } catch {
          continue; // not JSON after all, so leave the fence in the markdown run
        }
        html += window.AppServices.renderMarkdown(source.slice(cursor, match.index)) || '';
        html += renderRawJsonDisclosure(match[1], parsed);
        cursor = match.index + match[0].length;
      }
      if (cursor === 0) {
        return window.AppServices.renderMarkdown(source) || '';
      }
      return html + (window.AppServices.renderMarkdown(source.slice(cursor)) || '');
    }

    // Entry headings arrive as HH:MM:SS.mmm. Milliseconds are not a time anyone
    // reads, so the heading shows seconds and keeps the exact value in a title
    // for the case where two entries land in the same second.
    const TR_HEADING_MS = /^(\d{1,2}:\d{2}:\d{2})\.\d{1,3}$/;
    const TR_HEADING_SELECTOR = ['.tr-screen-text', '.tr-entry-desc']
      .flatMap(scope => ['h1', 'h2', 'h3', 'h4', 'h5', 'h6'].map(tag => `${scope} ${tag}`))
      .join(', ');

    function trimHeadingMilliseconds(root) {
      root.querySelectorAll(TR_HEADING_SELECTOR).forEach(heading => {
        const text = heading.textContent.trim();
        const match = TR_HEADING_MS.exec(text);
        if (!match) return;
        heading.setAttribute('title', text);
        heading.textContent = match[1];
      });
    }

    // A block can hold dozens of entries; open only the first few and put the
    // rest behind one control, so the pane opens at a readable height.
    const TR_ENTRY_VISIBLE_MAX = 8;

    function boundEntryList(targetEl) {
      const list = targetEl.querySelector('.tr-unified');
      if (!list) return;
      const items = Array.from(list.querySelectorAll(':scope > [role="listitem"]'));
      const rest = items.slice(TR_ENTRY_VISIBLE_MAX);
      if (rest.length === 0) return;

      rest.forEach(entry => { entry.hidden = true; });
      // X-03: the list label is read aloud, and it counted every entry the pane
      // built while the bound below was hiding all but the first few — so a
      // screen reader was told about entries it could not reach. The label says
      // what is rendered; the button beside it carries the rest.
      const shown = items.length - rest.length;
      list.setAttribute('aria-label', window.JournalI18n.render('journal.window.transcripts.entries.shown', [...transcriptCount('shown', shown), ...transcriptCount('total', items.length)]));

      const more = document.createElement('button');
      more.type = 'button';
      more.className = 'tr-entry-more';
      more.setAttribute('aria-expanded', 'false');
      more.textContent = window.JournalI18n.render('journal.window.transcripts.entries.show.remaining', transcriptCount('remaining', rest.length));
      more.addEventListener('click', () => {
        const expanded = more.getAttribute('aria-expanded') !== 'true';
        rest.forEach(entry => { entry.hidden = !expanded; });
        more.setAttribute('aria-expanded', String(expanded));
        list.setAttribute('aria-label', expanded
          ? window.JournalI18n.render('journal.window.transcripts.entries.quantity', transcriptCount('items', items.length))
          : window.JournalI18n.render('journal.window.transcripts.entries.shown', [...transcriptCount('shown', shown), ...transcriptCount('total', items.length)]));
        more.textContent = expanded
          ? window.JournalI18n.render('journal.window.transcripts.visible.text.show.fewer.entries')
          : window.JournalI18n.render('journal.window.transcripts.entries.show.remaining', transcriptCount('remaining', rest.length));
        if (expanded) collapseLongPreBlocks(targetEl);
      });
      list.insertAdjacentElement('afterend', more);
    }

    // Playback walks every entry, including the ones the bound above is holding
    // back; open the rest rather than highlighting something nobody can see.
    function revealBoundedEntry(targetEl, entry) {
      if (!entry.hidden) return;
      targetEl.querySelector('.tr-entry-more[aria-expanded="false"]')?.click();
    }

    const TR_PRE_COLLAPSE_MAX_HEIGHT = 256; // fallback only, when the computed max-height is not a usable number

    function collapseLongPreBlocks(root) {
      root.querySelectorAll('.tr-entry-desc pre, .tr-screen-text pre').forEach(pre => {
        if (pre.dataset.trPreChecked) return;
        if (!pre.scrollHeight) return; // unmeasurable while the pane is hidden; try again once it is shown
        pre.dataset.trPreChecked = '1';
        const computedMax = parseFloat(getComputedStyle(pre).maxHeight);
        const maxHeight = Number.isFinite(computedMax) ? computedMax : TR_PRE_COLLAPSE_MAX_HEIGHT;
        if (pre.scrollHeight <= maxHeight + 2) return;

        const toggle = document.createElement('button');
        toggle.type = 'button';
        toggle.className = 'tr-pre-toggle';
        toggle.textContent = window.JournalI18n.render('journal.window.transcripts.visible.text.show.full.output');
        toggle.setAttribute('aria-expanded', 'false');
        toggle.addEventListener('click', () => {
          const expanded = pre.classList.toggle('tr-pre-expanded');
          toggle.textContent = expanded ? window.JournalI18n.render('journal.window.transcripts.visible.text.show.less.output') : window.JournalI18n.render('journal.window.transcripts.visible.text.show.full.output');
          toggle.setAttribute('aria-expanded', String(expanded));
        });
        pre.insertAdjacentElement('afterend', toggle);
      });
    }

    // The day's screen frames and screenshots in time order, one segment at a
    // time: the open segment's, then its neighbours' as the viewer reaches them.
    function screenSegmentsInDayOrder() {
      return allSegments
        .filter(seg => (seg.types || []).includes('screen') || (seg.types || []).includes('image'))
        .slice()
        .sort((a, b) => (parseTime(a.start) - parseTime(b.start)) || String(a.stream).localeCompare(String(b.stream)));
    }

    function viewerFrame(chunk, files, seg) {
      const raw = chunk.source_ref?.raw;
      const filename = chunk.source_ref?.filename;
      return {
        chunk,
        seg,
        image: raw ? (files.image_files || {})[raw] || null : null,
        video: filename ? (files.video_files || {})[filename] || null : null,
      };
    }

    function openImageModal(frameIndex) {
      if (frameIndex < 0 || frameIndex >= allScreenFrames.length) return;

      currentFrameIndex = frameIndex;
      const triggerElement = document.activeElement;
      const prevOverflow = document.body.style.overflow;
      const openSegment = selectedSegment;
      const frames = allScreenFrames.map(chunk => viewerFrame(
        chunk,
        { image_files: currentImageFiles, video_files: currentVideoFiles },
        openSegment
      ));
      let index = frameIndex;
      let loadingNeighbour = false;
      const exhausted = { '-1': false, '1': false };

      const modal = document.createElement('div');
      modal.className = 'tr-screenshot-modal';
      modal.id = 'trImageModal';
      modal.setAttribute('role', 'dialog');
      modal.setAttribute('aria-modal', 'true');
      modal.setAttribute('aria-label', window.JournalI18n.render('journal.window.transcripts.accessible.label.screenshot.viewer'));

      // Where the next segment holding frames sits, from the frames at this end.
      const neighbourPosition = (order, direction) => {
        const edge = direction > 0 ? frames[frames.length - 1].seg : frames[0].seg;
        if (!edge) return -1;
        const at = order.findIndex(seg => seg.key === edge.key && seg.stream === edge.stream);
        if (at >= 0) return at;
        const start = parseTime(edge.start);
        const after = order.findIndex(seg => parseTime(seg.start) > start);
        if (direction > 0) return after < 0 ? order.length - 1 : after - 1;
        return after < 0 ? order.length : after;
      };

      const hasNeighbour = direction => {
        if (exhausted[String(direction)]) return false;
        const order = screenSegmentsInDayOrder();
        const next = neighbourPosition(order, direction) + direction;
        return next >= 0 && next < order.length;
      };

      // Load the frames of the nearest segment that has any, in this direction.
      const extend = async direction => {
        const order = screenSegmentsInDayOrder();
        let position = neighbourPosition(order, direction);
        for (let step = 0; step < 50; step += 1) {
          position += direction;
          if (position < 0 || position >= order.length) break;
          const seg = order[position];
          let data;
          try {
            const response = await dayFetch(`/app/transcripts/api/segment/${day}/${seg.stream}/${seg.key}`);
            if (!response.ok) continue;
            data = await response.json();
          } catch (_) {
            continue;
          }
          const found = (data.chunks || [])
            .filter(chunk => chunk.type === 'screen' || chunk.type === 'image')
            .map(chunk => viewerFrame(chunk, data, seg))
            .filter(frame => frame.image || frame.video);
          if (found.length === 0) continue;
          if (direction > 0) {
            frames.push(...found);
          } else {
            frames.unshift(...found);
            index += found.length;
          }
          return true;
        }
        exhausted[String(direction)] = true;
        return false;
      };

      const drawFrame = (canvas, frame) => {
        const f = frame.chunk;
        const frameId = f.source_ref?.frame_id;
        const boxCoords = f.source_ref?.box_2d;
        const participants = f.source_ref?.participants || [];
        const objects = f.source_ref?.objects;

        if (frame.image) {
          frameCapture.drawStill(canvas, frame.image, { objects });
        } else if (frame.video && frameId) {
          frameCapture.drawFull(canvas, frame.video, frameId, {
            boxCoords,
            participants,
            objects
          });
        } else {
          canvas.classList.remove('loading');
        }
      };

      const updateModalContent = () => {
        const frame = frames[index];
        const f = frame.chunk;
        const shot = f.screenshot;
        const photo = f.type === 'image' ? (f.photo || null) : null;
        modal.setAttribute('aria-label', f.type === 'image' ? PHOTO_COPY.viewerLabel : window.JournalI18n.render('journal.window.transcripts.accessible.label.screenshot.viewer'));
        const monitor = shot ? '' : (f.source_ref?.monitor || '');
        const monitorPos = getMonitorPosition(monitor);
        const analysis = f.source_ref?.analysis || {};
        const category = shot ? '' : (analysis.primary || '');
        const searchableShot = !shot || shot.text === 'searchable';
        const photoCaption = photo && typeof photo.caption === 'string' && photo.caption.trim() ? photo.caption.trim() : '';
        const description = f.type === 'image'
          ? (photo ? (photo.description || '') : (f.markdown || ''))
          : (searchableShot ? (analysis.visual_description || '') : '');
        const imageUrl = frame.image;
        const altText = photoCaption || description || (shot ? SCREENSHOT_COPY.J5 : f.type === 'image' ? PHOTO_COPY.label : window.JournalI18n.render('journal.window.transcripts.media.screen.image'));
        const device = shot ? screenshotDevice(frame.seg && frame.seg.stream) : '';
        const shotLabel = shot
          ? (device ? `${SCREENSHOT_COPY.J1} · ${device}` : SCREENSHOT_COPY.J1)
          : (f.type === 'image' ? (photo && photo.is_screenshot ? SCREENSHOT_COPY.J1 : PHOTO_COPY.label) : '');
        const hasPrev = index > 0 || hasNeighbour(-1);
        const hasNext = index < frames.length - 1 || hasNeighbour(1);
        const modalFooter = (photoCaption || description)
          ? `<div class="tr-modal-description">${photoCaption ? `<div class="tr-modal-photo-caption"><span class="tr-modal-part-label">${escapeHtml(PHOTO_COPY.captionLabel)}</span> <span class="tr-modal-part-text">${escapeHtml(photoCaption)}</span></div>` : ''}${description ? `<div class="tr-modal-photo-desc">${photo ? `<span class="tr-modal-part-label">${escapeHtml(PHOTO_COPY.descriptionLabel)}</span> ` : ''}<span class="tr-modal-part-text">${escapeHtml(description)}</span></div>` : ''}</div>`
          : '';

        modal.innerHTML = `
          <div class="tr-modal-nav${hasPrev ? '' : ' disabled'}" data-dir="prev" title="${window.JournalI18n.renderHtml('journal.window.transcripts.tooltip.previous.frame.left.arrow')}" role="button" tabindex="${hasPrev ? '0' : '-1'}">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M15 18l-6-6 6-6"/></svg>
          </div>
          <div class="tr-modal-center">
            <div class="tr-modal-header">
              ${shotLabel ? `<span class="tr-modal-badge">${escapeHtml(shotLabel)}</span>` : ''}
              ${f.time && frame.seg !== openSegment ? `<span class="tr-modal-badge">${escapeHtml(f.time)}</span>` : ''}
              ${monitorPos ? `<span class="tr-modal-badge tr-modal-badge-monitor">${monitorPos}</span>` : ''}
              ${category ? `<span class="tr-modal-badge tr-modal-badge-category">${escapeHtml(category)}</span>` : ''}
              <button class="tr-modal-close" title="${window.JournalI18n.renderHtml('journal.window.transcripts.tooltip.close.esc')}" aria-label="${window.JournalI18n.renderHtml('journal.window.transcripts.accessible.name.close')}">&times;</button>
            </div>
            <div class="tr-modal-img-wrap">
              ${imageUrl && !f.source_ref?.objects?.items?.length
                ? `<img id="trModalImage" src="${escapeHtml(imageUrl)}" alt="${escapeHtml(altText)}">`
                : `<canvas id="trModalCanvas" class="loading" role="img" aria-label="${escapeHtml(altText)}"></canvas>`}
            </div>
            ${modalFooter}
          </div>
          <div class="tr-modal-nav${hasNext ? '' : ' disabled'}" data-dir="next" title="${window.JournalI18n.renderHtml('journal.window.transcripts.tooltip.next.frame.right.arrow')}" role="button" tabindex="${hasNext ? '0' : '-1'}">
            <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M9 18l6-6-6-6"/></svg>
          </div>
        `;

        // Draw video frames, and stills that carry object boxes, to the modal canvas.
        // Other still images are loaded directly.
        const canvas = modal.querySelector('#trModalCanvas');
        if (canvas) {
          drawFrame(canvas, frame);
        }
      };

      const navigateFrame = async (delta) => {
        if (loadingNeighbour) return;
        let newIndex = index + delta;
        if (newIndex < 0 || newIndex >= frames.length) {
          loadingNeighbour = true;
          const grew = await extend(delta);
          loadingNeighbour = false;
          if (!document.contains(modal)) return;
          if (!grew) {
            updateModalContent();
            modal.querySelector('.tr-modal-close')?.focus();
            return;
          }
          newIndex = index + delta;
        }
        if (newIndex >= 0 && newIndex < frames.length) {
          index = newIndex;
          currentFrameIndex = frames[index].seg === openSegment ? allScreenFrames.indexOf(frames[index].chunk) : -1;
          updateModalContent();
          modal.querySelector('.tr-modal-close')?.focus();
        }
      };

      let stopKeys = () => {};
      const closeModal = () => {
        modal.remove();
        stopKeys();
        document.body.style.overflow = prevOverflow;
        if (triggerElement && document.contains(triggerElement)) triggerElement.focus();
        currentFrameIndex = -1;
      };

      // The viewer is modal, so a key goes to it alone: nothing under it, such
      // as the day's arrow keys, acts while it is open.
      const handleKeys = (e) => {
        e.stopPropagation();
        const nav = (e.key === 'Enter' || e.key === ' ') && document.activeElement?.closest?.('.tr-modal-nav:not(.disabled)');
        if (nav && modal.contains(nav)) {
          e.preventDefault();
          navigateFrame(nav.dataset.dir === 'prev' ? -1 : 1);
        } else if (e.key === 'Escape') closeModal();
        else if (e.key === 'ArrowLeft') { e.preventDefault(); navigateFrame(-1); }
        else if (e.key === 'ArrowRight') { e.preventDefault(); navigateFrame(1); }
        else if (e.key === 'Tab') {
          const focusable = [...modal.querySelectorAll('button:not([disabled]), [tabindex="0"]')];
          if (focusable.length === 0) return;
          const currentIndex = focusable.indexOf(document.activeElement);
          if (e.shiftKey) {
            e.preventDefault();
            focusable[currentIndex <= 0 ? focusable.length - 1 : currentIndex - 1].focus();
          } else {
            e.preventDefault();
            focusable[currentIndex >= focusable.length - 1 ? 0 : currentIndex + 1].focus();
          }
        }
      };

      // Event delegation for modal clicks
      modal.addEventListener('click', (e) => {
        const target = e.target.closest('.tr-modal-close, .tr-modal-nav:not(.disabled)');
        if (target) {
          if (target.classList.contains('tr-modal-close')) closeModal();
          else if (target.classList.contains('tr-modal-nav')) {
            navigateFrame(target.dataset.dir === 'prev' ? -1 : 1);
          }
        } else if (e.target === modal || e.target.classList.contains('tr-modal-img-wrap')) {
          closeModal();
        }
      });

      document.body.style.overflow = 'hidden';
      document.body.appendChild(modal);
      updateModalContent();
      modal.querySelector('.tr-modal-close')?.focus();
      stopKeys = ctx.on(document, 'keydown', handleKeys, true);
    }

    function updateZoom() {
      zoom.setAttribute('aria-label', window.JournalI18n.render('journal.window.transcripts.timeline.detail.range', [['start_time', clockLabel(range.start)], ['end_time', clockLabel(range.end)]]));
      zoomHeight = zoom.clientHeight - ZOOM_PADDING;
      const rangeLen = range.end - range.start;
      if (rangeLen > 0) {
        zoomPpm = zoomHeight / rangeLen;
        buildZoomGrid();
        buildZoomSegments();
      }
    }

    // Rail content is inset below the header and keeps a compact bottom
    // gutter; the zoom rail also reserves the band naming its stream columns.
    const RAIL_BOTTOM = 12;          // the rails' `bottom: 12px` insets
    const ZOOM_STREAMS_HEIGHT = 18;  // must equal .tr-zoom-streams' height
    const DAY_PADDING = HEADER_HEIGHT + RAIL_BOTTOM;
    const ZOOM_PADDING = HEADER_HEIGHT + ZOOM_STREAMS_HEIGHT + RAIL_BOTTOM;

    // One quiet line when part of the day was recorded on another clock, so
    // the grid's times (each on its own recording's clock) read right.
    function dayZonesText(zones) {
      if (!zones || !Array.isArray(zones.zones) || zones.zones.length === 0) return '';
      const list = items => items.length <= 1
        ? items.join('')
        : `${items.slice(0, -1).join(', ')} and ${items[items.length - 1]}`;
      const places = zones.zones.filter(zone => zone.place).map(zone => zone.label);
      const offsets = zones.zones.filter(zone => !zone.place).map(zone => zone.label);
      const named = places.length
        ? [window.JournalI18n.render('journal.window.chrome.zone.time', [['label', list(places)]])]
        : [];
      const text = list(named.concat(offsets));
      if (!text) return '';
      const id = zones.all ? 'journal.window.chrome.day.zones.all' : 'journal.window.chrome.day.zones.part';
      return window.JournalI18n.render(id, [['zones', text]]);
    }

    function renderDayZones(zones) {
      const el = document.getElementById('trDayZones');
      if (!el) return;
      const text = dayZonesText(zones);
      el.textContent = text;
      el.hidden = !text;
    }

    function journalToday() {
      if (window.JournalClock) return window.JournalClock.today();
      const now = new Date();
      return String(now.getFullYear()) +
        String(now.getMonth() + 1).padStart(2, '0') +
        String(now.getDate()).padStart(2, '0');
    }

    // The day rail's blocks: audio and screen as the journal merged them, then
    // each import, written message, page visit, desktop activity and photo.
    // Each block says what it is, so the day switch can draw what it leaves
    // out faintly. A range of audio is spoken words.
    function drawDayIndicators(ranges, segments) {
      segmentsLane.innerHTML = '';
      const kindOf = (el, kind) => { el.dataset.showKind = kind; };
      (ranges.audio || []).forEach(rg => {
        kindOf(addSegmentIndicator('audio', parseTime(rg.start), parseTime(rg.end), 0, rg.streams, rg.state || 'analyzed', rg.think), 'transcripts');
      });
      (ranges.screen || []).forEach(rg => {
        kindOf(addSegmentIndicator('screen', parseTime(rg.start), parseTime(rg.end), 1, rg.streams, rg.state || 'analyzed', rg.think), 'other');
      });
      [...markdownOnlySegments(segments), ...writtenSegments(segments)].forEach(seg => {
        kindOf(addSegmentIndicator('markdown', parseTime(seg.start), parseTime(seg.end), 0, [seg.stream], 'analyzed', seg.think, seg), showKind(seg));
      });
      browserOnlySegments(segments).forEach(seg => {
        kindOf(addSegmentIndicator('browser', parseTime(seg.start), parseTime(seg.end), 1, [seg.stream], 'analyzed', seg.think, seg), 'other');
      });
      desktopActivityOnlySegments(segments).forEach(seg => {
        kindOf(addSegmentIndicator('desktop_activity', parseTime(seg.start), parseTime(seg.end), 1, [seg.stream], 'analyzed', seg.think, seg), 'other');
      });
      imageOnlySegments(segments).forEach(seg => {
        kindOf(addSegmentIndicator('image', parseTime(seg.start), parseTime(seg.end), 1, [seg.stream], 'analyzed', seg.think, seg), 'other');
      });
    }

    // The key above the rails names only the kinds this day draws, on either
    // rail.
    function renderLegend() {
      const legend = document.getElementById('trLegend');
      if (!legend) return;
      const drawn = new Set(allSegments.filter(seg => !isDateOnlyImport(seg)).map(pillKind));
      for (const kind of ['audio', 'screen', 'markdown', 'browser', 'image']) {
        if (segmentsLane.querySelector('.tr-seg-' + kind)) drawn.add(kind);
      }
      for (const kind of ['sleep', 'workout']) {
        if (bodyEventsLane.querySelector('.tr-body-event-' + kind)) drawn.add(kind);
      }
      legend.querySelectorAll('[data-legend-kind]').forEach(item => {
        item.hidden = !drawn.has(item.dataset.legendKind);
      });
    }

    // The newest thing a device captured today. Imports, written messages and
    // what an agent wrote are not a device capturing.
    const CAPTURED_TYPES = ['audio', 'screen', 'browser', 'desktop_activity', 'image'];
    function latestCapture(segments) {
      let latest = null;
      for (const seg of segments) {
        if (seg.item || isDateOnlyImport(seg) || isWrittenOrUnplaced(seg)) continue;
        if (!(seg.types || []).some(type => CAPTURED_TYPES.includes(type))) continue;
        if (window.JournalFormat.streamSays(seg.stream).imported) continue;
        const end = parseTime(seg.end);
        if (!latest || end > parseTime(latest.end) || (end === parseTime(latest.end) && seg.key > latest.key)) latest = seg;
      }
      return latest;
    }

    // On today, one quiet line by the heading: which device last added
    // something, and when. It states a fact; whether capture is healthy is the
    // status icon's to say. Nothing captured today, nothing said.
    function renderLatest() {
      const line = document.getElementById('trDayLatest');
      if (!line) return;
      const latest = day === journalToday() ? latestCapture(allSegments) : null;
      line.hidden = !latest;
      if (!latest) {
        line.textContent = '';
        line.removeAttribute('title');
        return;
      }
      line.textContent = window.JournalI18n.render('journal.window.transcripts.day.latest', [
        ['device', humanizeStreamName(latest.stream)],
        ['time', window.JournalFormat.segmentTimeOfDay(latest.end.replace(':', '') + '00')],
      ]);
      line.title = latest.stream;
    }

    // What a read of the day drew from, to tell a changed day from the same one.
    function dayShape(data) {
      return JSON.stringify([data.audio || [], data.screen || [], data.segments || []]);
    }

    // The owner is in the middle of something a redraw would disturb.
    function ownerMidAction() {
      return Boolean(drag) || segPick.on || deleteSegmentModal.style.display !== 'none'
        || Boolean(document.getElementById('trImageModal')) || document.activeElement === segmentPicker;
    }

    // A redraw replaces the rails' blocks. The one holding the focus is found
    // again by what it stands for, so the focus stays where the owner left it.
    function railFocus() {
      const el = document.activeElement;
      if (!el) return null;
      if (el.parentElement === zoomSegments) return { lane: zoomSegments, key: el.dataset.key, stream: el.dataset.stream };
      if (el.parentElement === segmentsLane) return { lane: segmentsLane, start: el.dataset.startMinute, col: el.classList.contains('tr-seg-col-1') };
      return null;
    }

    function restoreRailFocus(focused) {
      if (!focused) return;
      const match = [...focused.lane.children].find(el => (focused.lane === zoomSegments
        ? el.dataset.key === focused.key && el.dataset.stream === focused.stream
        : el.dataset.startMinute === focused.start && el.classList.contains('tr-seg-col-1') === focused.col));
      if (match) match.focus({ preventScroll: true });
    }

    function redrawDetail() {
      const focused = railFocus();
      buildZoomSegments();
      dimLeftOut();
      restoreRailFocus(focused);
    }

    // Today, while the page is open and in view, the day is read again on the
    // now line's tick, so what arrives shows without a reload. Only the rails,
    // the key and the latest line are redrawn: the range, the open segment
    // and the reading pane stay as the owner left them. A tick is skipped
    // while the owner is mid-action.
    function readTodayAgain() {
      if (document.hidden || todayRead || dayLoad !== 'ready' || day !== journalToday() || ownerMidAction()) return;
      todayRead = dayFetch(`/app/transcripts/api/day/${day}`)
        .then(r => {
          if (!r.ok) throw new Error(`day read answered ${r.status}`);
          return r.json();
        })
        .then(data => {
          if (!ownerMidAction()) applyTodayAgain(data);
        })
        .catch(err => {
          if (err?.name !== 'AbortError') console.warn('transcripts: reading today again did not complete:', err);
        })
        .finally(() => { todayRead = null; });
    }

    function listedImports(segments) {
      return segments.filter(seg => seg.item || isWrittenOrUnplaced(seg)).map(segmentChoiceId).join('\n');
    }

    function applyTodayAgain(data) {
      const shape = dayShape(data);
      if (shape === drawnDayShape) return;
      drawnDayShape = shape;
      const before = new Map(allSegments.map(seg => [segmentChoiceId(seg), seg]));
      const importsBefore = listedImports(allSegments);
      const hadNone = allSegments.length === 0;
      const switchBefore = dayShowsSwitch();
      // A segment that has not changed keeps its object.
      allSegments = (data.segments || []).map(seg => {
        const old = before.get(segmentChoiceId(seg));
        return old && JSON.stringify(old) === JSON.stringify(seg) ? old : seg;
      });
      renderDayZones(data.zones);

      // The day rail only widens: what was in view stays in view.
      const bounds = computeTimelineBounds(data);
      if (bounds.start < timelineStart || bounds.end > timelineEnd) {
        timelineStart = Math.min(timelineStart, bounds.start);
        timelineEnd = Math.max(timelineEnd, bounds.end);
        timeline.setAttribute('aria-label', window.JournalI18n.render('journal.window.transcripts.timeline.day.range', [['start_time', clockLabel(timelineStart)], ['end_time', clockLabel(timelineEnd)]]));
        ppm = height / (timelineEnd - timelineStart);
        buildGrid();
        renderTimeline();
        renderBodyEvents(dayBodyWindow);
        if (updateNowPosition) updateNowPosition();
      }

      const focused = railFocus();
      drawDayIndicators(data, allSegments);
      buildZoomSegments();
      dimLeftOut();
      restoreRailFocus(focused);
      if (!selectedSegment && hadNone && allSegments.length) panel.innerHTML = dayInvitationHtml;
      if (switchBefore !== dayShowsSwitch()) applyShow();
      else if (!selectedSegment && (hadNone || listedImports(allSegments) !== importsBefore)) refreshUnselectedLocation(null, true);
      syncStretchButton();
      renderLegend();
      renderLatest();

      // A device first met since the page loaded has its label read once.
      const unread = [...new Set(allSegments.map(seg => seg.stream))].filter(stream => window.JournalFormat.needsStreamLabel(stream));
      if (!unread.length) return;
      const asked = dayEpoch;
      Promise.all(unread.map(stream => window.JournalFormat.noteLiveStream(stream))).then(() => {
        if (asked !== dayEpoch || ownerMidAction()) return;
        redrawDetail();
        renderLatest();
      });
    }

    // Back in view, today shows what arrived while it was away at once.
    ctx.on(document, 'visibilitychange', () => {
      if (document.hidden || !nowTimer) return;
      if (updateNowPosition) updateNowPosition();
      renderLatest();
      readTodayAgain();
    });

    // Load combined transcript data
    function loadDayData() {
      dayLoad = 'pending';
      syncStretchButton();
      if (!selectedSegment) {
        panel.innerHTML = window.SurfaceState.loading({ text: window.JournalI18n.render('journal.window.transcripts.text.loading.this.day'), busy: false });
      }
      dayFetch(`/app/transcripts/api/day/${day}`)
      .then(async r => {
        if (!r.ok) {
          let payload = {};
          try {
            payload = await r.json();
          } catch (_) {
            payload = {};
          }
          throw new window.ApiError({
            status: r.status,
            statusText: r.statusText,
            serverMessage: payload.error || window.JournalI18n.render('journal.window.transcripts.day.load.error.status', [['status', String(r.status)]]),
            url: r.url,
            reasonCode: payload.reason_code,
            rawDetail: payload.detail,
          });
        }
        return r.json();
      })
      .then(data => {
        // Apply dynamic timeline bounds from ranges
        renderDayZones(data.zones);
        const bounds = computeTimelineBounds(data);
        timelineStart = bounds.start;
        timelineEnd = bounds.end;
        timeline.setAttribute('aria-label', window.JournalI18n.render('journal.window.transcripts.timeline.day.range', [['start_time', clockLabel(timelineStart)], ['end_time', clockLabel(timelineEnd)]]));

        // Recalculate pixels-per-minute with new bounds
        height = timeline.clientHeight - DAY_PADDING;
        ppm = height / (timelineEnd - timelineStart);

        // Set initial selection range within bounds
        // Center on current time if viewing today, otherwise midpoint
        const todayStr = journalToday();
        let nowMin;
        if (window.JournalClock) {
          const p = window.JournalClock.parts(window.JournalClock.now());
          nowMin = parseInt(p.hour, 10) * 60 + parseInt(p.minute, 10);
        } else {
          const now = new Date();
          nowMin = now.getHours() * 60 + now.getMinutes();
        }
        let center;
        if (day === todayStr) {
          center = Math.max(timelineStart, Math.min(timelineEnd, nowMin));
        } else {
          center = (timelineStart + timelineEnd) / 2;
        }
        range = { start: snap(center - DEFAULT_LEN / 2), end: snap(center + DEFAULT_LEN / 2) };
        if (range.start < timelineStart) {
          range = { start: timelineStart, end: timelineStart + DEFAULT_LEN };
        }
        if (range.end > timelineEnd) {
          range = { start: timelineEnd - DEFAULT_LEN, end: timelineEnd };
        }
        const daySegs = data.segments || [];
        if (day !== todayStr && daySegs.length > 0) {
          const hasInRange = daySegs.some(seg => {
            const s = parseTime(seg.start);
            const e = parseTime(seg.end);
            return s === e ? (s >= range.start && s < range.end) : (e > range.start && s < range.end);
          });
          if (!hasInRange) {
            const firstMin = parseTime(daySegs[0].start);
            let start = snap(firstMin - DEFAULT_LEN / 2);
            if (start < timelineStart) start = timelineStart;
            if (start + DEFAULT_LEN > timelineEnd) start = Math.max(timelineStart, timelineEnd - DEFAULT_LEN);
            range = { start, end: start + DEFAULT_LEN };
          }
        }

        // Build the grid and render timeline
        buildGrid();
        renderTimeline();

        drawDayIndicators(data, data.segments || []);
        drawnDayShape = dayShape(data);
        fetchDayBodyWindow()
          .then(payload => {
            dayBodyWindow = payload;
            renderBodyEvents(dayBodyWindow);
            renderLegend();
          })
          .catch(() => {
            dayBodyWindow = null;
            renderBodyEvents(null);
            renderLegend();
          });

        // Now-marker for today. A reload of the day replaces the one it had.
        if (nowTimer) clearInterval(nowTimer);
        nowTimer = null;
        updateNowPosition = null;
        timeline.querySelectorAll('.tr-now-marker').forEach(old => old.remove());
        if (day === todayStr) {
          const marker = document.createElement('div');
          marker.className = 'tr-now-marker';
          marker.setAttribute('aria-label', window.JournalI18n.render('journal.window.transcripts.accessible.label.current.time'));
          const lbl = document.createElement('span');
          lbl.className = 'tr-now-label';
          lbl.textContent = window.JournalI18n.render('journal.window.transcripts.visible.text.now');
          marker.appendChild(lbl);
          timeline.appendChild(marker);

          updateNowPosition = function() {
            let currentNowMin;
            if (window.JournalClock) {
              const p = window.JournalClock.parts(window.JournalClock.now());
              currentNowMin = parseInt(p.hour, 10) * 60 + parseInt(p.minute, 10);
            } else {
              const n = new Date();
              currentNowMin = n.getHours() * 60 + n.getMinutes();
            }
            if (currentNowMin < timelineStart || currentNowMin > timelineEnd) {
              marker.style.display = 'none';
            } else {
              marker.style.display = '';
              marker.style.top = (HEADER_HEIGHT + y(currentNowMin)) + 'px';
            }
          };
          updateNowPosition();
          // The same tick reads today again while the page is in view.
          nowTimer = setInterval(() => {
            updateNowPosition();
            renderLatest();
            readTodayAgain();
          }, 60000);
        }

        // Store segments and update zoom
        allSegments = data.segments || [];
        dayLoad = 'ready';
        syncStretchButton();
        if (allSegments.length && !selectedSegment) {
          panel.innerHTML = dayInvitationHtml;
        }
        updateZoom();
        applyShow();
        renderLegend();
        renderLatest();
        reportTranscriptsReady(allSegments.length ? 'content' : 'empty');
        refreshUnselectedLocation(null);

        // Check for hash fragment to auto-select segment
        const { segKey, tab: tabIdFromHash, moment } = hashLocation();
        if (segKey) {
          const seg = linkedSegment(segKey);
          if (seg) {
            linkMoment(seg, moment);
            // selectSegment recenters the detail zoom range when needed.
            selectSegment(seg, false, tabIdFromHash);
          } else {
            // A link to a segment the day doesn't list may lead to the trash.
            const stream = new URLSearchParams(window.location.search).get('stream');
            if (stream) showSegmentInTrash(stream, segKey);
          }
        }
      })
      .catch(err => {
        console.error('transcripts: day data load did not complete:', err);
        dayLoad = 'error';
        syncStretchButton();
        zoomSegments.innerHTML = '';
        renderStreamHeads([]);
        segmentPicker.innerHTML = `<option value="">${window.JournalI18n.renderHtml('journal.window.transcripts.option.label.choose.a.segment')}</option>`;
        segmentPicker.disabled = true;
        panel.innerHTML = window.SurfaceState.error({
          heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.transcripts.couldn.t.be.loaded'),
          desc: window.CONVEY_COPY?.RELOAD_HINT || window.JournalI18n.render('journal.window.transcripts.owner.desc.reload.to.try.again'),
          retry: true,
          retryLabel: window.CONVEY_COPY?.ACTION_TRY_AGAIN || window.JournalI18n.render('journal.window.transcripts.day.load.retry'),
          serverMessage: err.serverMessage,
          detail: err,
        });
        const retry = panel.querySelector('.surface-state-retry');
        if (retry) retry.addEventListener('click', loadDayData);
        reportTranscriptsReady('error');
        refreshUnselectedLocation(null);
      });
    }

    function locationAddonHtml() {
      if (locationLoad === 'error') {
        return `<p data-location-notice="true">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.location.for.this.day.couldn.t')}</p>`;
      }
      if (locationLoad === 'ready' && locationSegments.length > 0) {
        const count = locationSegments.length;
        const collapsedText = window.JournalI18n.render('journal.window.transcripts.location.show.count', transcriptCount('segments', count));
        // Name each row's device only when more than one sent location this day.
        const manyLocationStreams = new Set(locationSegments.map(s => humanizeStreamName(s.stream))).size > 1;
        const rowsHtml = locationSectionExpanded ? `
          <div class="tr-location-rows">
            ${locationSegments.map(s => {
              const deviceSuffix = manyLocationStreams
                ? ` · <span title="${escapeHtml(s.stream)}">${escapeHtml(humanizeStreamName(s.stream))}</span>`
                : '';
              const hasReadings = typeof s.location_readings === 'number';
              const readings = hasReadings ? s.location_readings : null;
              const text = readings > 0
                ? window.JournalI18n.render('journal.window.transcripts.location.row.readings', [
                  ...transcriptCount('readings', readings), ['start_time', s.start],
                ])
                : s.start;
              const dataReadingsAttr = hasReadings ? ` data-readings="${readings}"` : '';
              return `<button type="button" class="tr-location-row" data-key="${escapeHtml(s.key)}" data-stream="${escapeHtml(s.stream)}"${dataReadingsAttr}>${escapeHtml(text)}${deviceSuffix}</button>`;
            }).join('')}
          </div>
        ` : '';
        return `
          <div class="tr-location-section" data-location-section="true">
            <h3 class="tr-location-heading">${window.JournalI18n.renderHtml('journal.window.transcripts.section.heading.location')}</h3>
            <button type="button" class="tr-location-toggle" aria-expanded="${locationSectionExpanded}">
              ${escapeHtml(locationSectionExpanded ? window.JournalI18n.render('journal.window.transcripts.status.text.hide.location.segments') : collapsedText)}
            </button>
            ${rowsHtml}
          </div>
        `;
      }
      return '';
    }

    function wireLocationAddonListeners() {
      const toggle = panel.querySelector('.tr-location-toggle');
      if (toggle) {
        toggle.addEventListener('click', () => {
          locationSectionExpanded = !locationSectionExpanded;
          refreshUnselectedLocation(null);
        });
      }
      panel.querySelectorAll('.tr-location-row').forEach(row => {
        row.addEventListener('click', () => {
          const seg = locationSegments.find(s => s.key === row.dataset.key && s.stream === row.dataset.stream);
          if (seg) selectLocationSegment(seg);
        });
      });
    }

    function renderDayImports(glanceHtml, importsChanged) {
      const listed = allSegments.filter(seg => seg.item || isWrittenOrUnplaced(seg));
      const filtered = dayShowsSwitch() && showFilter !== 'everything';
      const imports = filtered ? listed.filter(shownUnder) : listed;
      const leftOut = filtered ? listed.filter(seg => conversationKind(seg) === 'conversation').length : 0;
      const existing = panel.querySelector('.tr-day-imports, .tr-day-left-out');
      if (!imports.length && !leftOut) {
        if (glanceHtml || existing) panel.innerHTML = glanceHtml || dayInvitationHtml;
        return;
      }
      if (existing && !glanceHtml && !importsChanged) return;

      const list = document.createElement('ul');
      list.className = 'tr-day-imports';
      list.setAttribute('role', 'list');
      const ordered = [
        ...imports.filter(isDateOnlyImport),
        ...imports.filter(seg => !isDateOnlyImport(seg)),
      ];
      for (const seg of ordered) {
        const row = document.createElement('li');
        const button = document.createElement('button');
        button.type = 'button';
        button.append(
          deleteDialogElement('strong', null, importItemLabel(seg)),
          deleteDialogElement('span', 'tr-day-import-when', importItemWhen(seg)),
        );
        button.addEventListener('click', () => selectSegment(seg));
        row.append(button);
        list.append(row);
      }
      const shown = imports.length ? [list] : [];
      // A conversation the journal can't place belongs to neither filter, so
      // a filtered day names it rather than hide it, and links back.
      if (leftOut) {
        const note = deleteDialogElement('p', 'tr-day-left-out');
        const everything = document.createElement('a');
        everything.href = showHref('everything');
        everything.addEventListener('click', event => {
          event.preventDefault();
          chooseShow('everything');
        });
        note.append(...window.JournalI18n.renderMarkup('journal.window.transcripts.show.left.out', [
          ['conversations_quantity', window.JournalI18n.cardinalQuantity(leftOut)],
          ['conversations', leftOut.toLocaleString(window.JournalI18n.formatLocale)],
        ], { elements: { everything } }));
        shown.push(note);
      }
      panel.replaceChildren(...shown);
    }

    // ----- The day switch: everything, transcripts or messages -----
    // Shown only on a day that holds messages or a conversation. Its state is
    // the address (?show=), so a day step keeps it and back undoes it.
    const SHOW_CHOICES = ['everything', 'transcripts', 'messages'];
    let showFilter = readShow();

    function readShow() {
      const value = new URLSearchParams(window.location.search).get('show');
      return SHOW_CHOICES.includes(value) ? value : 'everything';
    }

    function showHref(choice) {
      const url = new URL(window.location.href);
      if (choice === 'everything') url.searchParams.delete('show');
      else url.searchParams.set('show', choice);
      return url.pathname + url.search + url.hash;
    }

    function dayShowsSwitch() {
      return allSegments.some(isWrittenOrUnplaced);
    }

    // What an item is, for the switch: spoken, written, a conversation the
    // journal can't place, or something else (screen, notes, photos…).
    function showKind(seg) {
      const kind = conversationKind(seg);
      if (kind) return kind;
      if (seg.item) return seg.item.kind === 'transcripts' ? 'transcripts' : 'other';
      return (seg.types || []).includes('audio') ? 'transcripts' : 'other';
    }

    function shownUnder(seg) {
      return showFilter === 'everything' || !dayShowsSwitch() || showKind(seg) === showFilter;
    }

    const showSwitch = document.createElement('nav');
    showSwitch.className = 'tr-show';
    showSwitch.hidden = true;
    showSwitch.setAttribute('aria-label', window.JournalI18n.render('journal.window.transcripts.show.label'));
    for (const choice of SHOW_CHOICES) {
      const link = document.createElement('a');
      link.className = 'tr-show-item';
      link.dataset.show = choice;
      link.textContent = window.JournalI18n.render(`journal.window.transcripts.show.${choice}`);
      link.addEventListener('click', event => {
        event.preventDefault();
        chooseShow(choice);
      });
      showSwitch.append(link);
    }
    document.querySelector('.tr-day-head')?.insertBefore(showSwitch, document.getElementById('trStretchBtn'));

    function chooseShow(choice) {
      if (choice === showFilter) return;
      ctx.history.push(showHref(choice));
      showFilter = choice;
      applyShow();
    }

    // Draw the switch and what it leaves out. Left-out blocks stay on the
    // day, faint, so the day still reads as the day.
    function applyShow() {
      const visible = dayShowsSwitch();
      showSwitch.hidden = !visible;
      for (const link of showSwitch.querySelectorAll('.tr-show-item')) {
        link.href = showHref(link.dataset.show);
        if (link.dataset.show === showFilter) link.setAttribute('aria-current', 'page');
        else link.removeAttribute('aria-current');
      }
      dimLeftOut();
      if (!selectedSegment && dayLoad === 'ready') renderDayImports(null, true);
    }

    function dimLeftOut() {
      const active = dayShowsSwitch() && showFilter !== 'everything';
      for (const block of [...segmentsLane.querySelectorAll('.tr-seg'), ...zoomSegments.querySelectorAll('.tr-zoom-pill')]) {
        block.classList.toggle('tr-show-dim', active && block.dataset.showKind !== showFilter);
      }
    }

    function refreshUnselectedLocation(glanceHtml = null, importsChanged = false) {
      if (selectedSegment) return;
      if (importsChanged && !allSegments.length) panel.querySelector('.tr-day-imports')?.remove();

      // Remove existing [data-location-section] and [data-location-notice] before painting a new one
      panel.querySelectorAll('[data-location-section]').forEach(el => el.remove());
      panel.querySelectorAll('[data-location-notice]').forEach(el => el.remove());

      if (dayLoad === 'error') {
        const addon = locationAddonHtml();
        if (addon) {
          panel.insertAdjacentHTML('beforeend', addon);
          wireLocationAddonListeners();
        }
        return;
      }

      if (allSegments.length === 0) {
        if (dayLoad === 'pending') {
          const addon = locationAddonHtml();
          if (addon) {
            panel.insertAdjacentHTML('beforeend', addon);
            wireLocationAddonListeners();
          }
          return;
        }
        if (dayLoad === 'ready') {
          if (locationLoad === 'pending') {
            return;
          }
          if (locationLoad === 'error') {
            panel.innerHTML = `<p data-location-notice="true">${window.JournalI18n.renderHtml('journal.window.transcripts.visible.text.location.for.this.day.couldn.t')}</p>`;
            return;
          }
          if (locationLoad === 'ready') {
            if (locationSegments.length === 0) {
              panel.innerHTML = window.SurfaceState.empty({
                icon: window.ConveyIcons.svg('calendar-days'),
                heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.nothing.here'),
                desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.no.observations.were.found.for.this')
              });
            } else {
              panel.innerHTML = locationAddonHtml();
              wireLocationAddonListeners();
            }
            return;
          }
        }
      }

      renderDayImports(glanceHtml, importsChanged);
      const addon = locationAddonHtml();
      if (addon) {
        panel.insertAdjacentHTML('beforeend', addon);
        wireLocationAddonListeners();
      }
    }

    function loadLocationData() {
      locationLoad = 'pending';
      return dayRead(`/app/transcripts/api/location/${day}`)
        .then(data => {
          if (!data || !Array.isArray(data.segments)) {
            locationSegments = [];
            locationLoad = 'error';
            refreshUnselectedLocation(null);
            return;
          }
          locationSegments = data.segments;
          locationLoad = 'ready';
          if (selectedSegment) {
            return;
          }
          refreshUnselectedLocation(null);
          const rawHash = window.location.hash.slice(1);
          const [segKey] = rawHash.split('/', 2);
          if (segKey) {
            if (linkedSegment(segKey)) {
              return;
            }
            const stream = new URLSearchParams(window.location.search).get('stream');
            const matching = locationSegments.filter(s => s.key === segKey && (!stream || s.stream === stream));
            if (matching.length === 1) {
              selectLocationSegment(matching[0], false);
            }
          }
        })
        .catch(err => {
          console.error('transcripts: location data load failed:', err);
          locationSegments = [];
          locationLoad = 'error';
          if (!selectedSegment) {
            refreshUnselectedLocation(null);
          }
        });
    }

    function resizeOverviewIndicators() {
      document.querySelectorAll('.tr-seg, .tr-body-event').forEach(el => {
        const start = Number(el.dataset.startMinute);
        const end = Number(el.dataset.endMinute);
        el.style.top = y(start) + 'px';
        el.style.height = Math.max(el.classList.contains('tr-body-event') ? 4 : 2, y(end) - y(start)) + 'px';
      });
    }

    function initTranscripts() {
      // Resize observers
      new ResizeObserver(() => {
        height = timeline.clientHeight - DAY_PADDING;
        ppm = height / (timelineEnd - timelineStart);
        buildGrid();
        resizeOverviewIndicators();
        renderTimeline();
        if (updateNowPosition) updateNowPosition();
      }).observe(timeline);

      new ResizeObserver(() => {
        updateZoom();
      }).observe(zoom);

      loadDayData();
      loadLocationData();

      // Popstate and a later hashchange share this restore, and an in-flight load
      // of the same segment and tab is already that page. The activeTab == null guard
      // is what stops a stale in-flight record from swallowing a tab change after the tab is shown.
      function restoreHistoryLocation() {
        const show = readShow();
        if (show !== showFilter) {
          showFilter = show;
          applyShow();
        }
        const { segKey, tab: tabIdFromHash, moment } = hashLocation();
        if (!segKey) {
          resetSegmentSelection();
          return;
        }
        const seg = linkedSegment(segKey);
        if (!seg) {
          const stream = new URLSearchParams(window.location.search).get('stream');
          const locSeg = locationSegments.find(s => s.key === segKey && (!stream || s.stream === stream));
          if (locSeg) {
            selectLocationSegment(locSeg, false);
            return;
          }
          resetSegmentSelection();
          // A segment the day doesn't list may wait in the trash: say so, and
          // where, rather than show the day as if the link led nowhere.
          if (stream) showSegmentInTrash(stream, segKey);
          return;
        }
        const wantedTab = tabIdFromHash || 'transcript';
        const samePair = selectedSegment
          && selectedSegment.key === seg.key
          && selectedSegment.stream === seg.stream;
        // Another moment of the recording already open is a place of its own:
        // load it again so its line is cued.
        // Coming back to a moment from another recording cues it again.
        if ((momentFor(seg)?.clock || null) !== moment || (moment && !samePair)) {
          linkMoment(seg, moment);
          if (moment) {
            selectSegment(seg, false, wantedTab);
            return;
          }
        }
        const inFlightMatches = activeTab == null
          && inFlightSegmentRequest
          && inFlightSegmentRequest.key === seg.key
          && inFlightSegmentRequest.stream === seg.stream
          && inFlightSegmentRequest.tab === wantedTab;
        if (samePair && (activeTab === wantedTab || inFlightMatches)) {
          return;
        }
        if (
          samePair
          && segmentData
          && activeTab
          && activeTab !== wantedTab
          && [...tabsContainer.querySelectorAll('.tr-tab')].some((tab) => tab.dataset.tab === wantedTab)
        ) {
          activateTab(wantedTab);
          return;
        }
        selectSegment(seg, false, wantedTab);
      }

      ctx.on(window, 'popstate', restoreHistoryLocation);
      ctx.on(window, 'hashchange', restoreHistoryLocation);
    }

    initTranscripts();

    // A day step: the page stays and shows another day. Everything that belongs
    // to the old day goes (its reads, its selection, its timeline, the now line,
    // the analyze poller); what belongs to the journal stays (known voices and
    // people, the calendar the date picker read).
    function changeDay(next) {
      if (!next || next === day) return;
      dayScope.abort();
      dayScope = window.SolStore.createAbort();
      dayEpoch += 1;
      if (currentSegmentAbort) currentSegmentAbort.abort();
      if (deleteSegmentModal.style.display !== 'none') closeDeleteSegmentModal(false);
      if (bodyPanel.classList.contains('visible')) closeBodyPanel();
      if (segPick.on) setSegPickMode(false);
      resetSegmentSelection();
      if (nowTimer) clearInterval(nowTimer);
      nowTimer = null;
      updateNowPosition = null;
      timeline.querySelectorAll('.tr-now-marker').forEach(marker => marker.remove());
      todayRead = null;
      drawnDayShape = '';
      day = next;
      allSegments = [];
      locationSegments = [];
      locationLoad = 'pending';
      locationSectionExpanded = false;
      dayBodyWindow = null;
      segmentsLane.innerHTML = '';
      bodyEventsLane.innerHTML = '';
      zoomSegments.innerHTML = '';
      renderStreamHeads([]);
      segmentPicker.innerHTML = `<option value="">${window.JournalI18n.renderHtml('journal.window.transcripts.option.label.choose.a.segment')}</option>`;
      segmentPicker.disabled = true;
      renderDayZones(null);
      renderLegend();
      renderLatest();
      window.DateNav?.setDay?.(next);
      loadDayData();
      loadLocationData();
    }

    // Keyboard navigation for segment stepping
    ctx.on(document, 'keydown', (e) => {
      if (e.key === 'Escape' && deleteSegmentModal.style.display !== 'none') {
        e.preventDefault();
        closeDeleteSegmentModal();
        return;
      }
      if (e.key === 'Escape' && bodyPanel.classList.contains('visible')
          && !document.getElementById('trImageModal')) {
        e.preventDefault();
        closeBodyPanel();
        return;
      }
      const tag = e.target.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA' || e.target.isContentEditable) return;
      if (document.getElementById('trImageModal')) return;
      if (deleteSegmentModal.style.display !== 'none') return;
      if (e.key === ']') { e.preventDefault(); navigateSegment(1); }
      else if (e.key === '[') { e.preventDefault(); navigateSegment(-1); }
    });

    ctx.on(window, 'click', (event) => {
      if (event.target === deleteSegmentModal) {
        closeDeleteSegmentModal();
      }
    });

    // Leaving the page, or the view, stops the poller. (Not `beforeunload`,
    // which keeps the page out of the browser's back-forward cache.)
    ctx.on(window, 'pagehide', stopAnalyzePoller);
    ctx.onDispose(stopAnalyzePoller);

    panel.addEventListener('click', (event) => {
      const button = event.target.closest('.tr-analyze-action');
      if (!button || !panel.contains(button)) return;
      event.preventDefault();
      performAnalyzeNow(button, button.dataset.modality);
    });

    // Click on timeline to set range
    timeline.addEventListener('click', (e) => {
      if (e.target.closest('.tr-sel')) return;
      const box = timeline.getBoundingClientRect();
      const py = e.clientY - box.top - HEADER_HEIGHT;
      let mid = snap(mFromY(py));
      let start = Math.max(timelineStart, Math.min(timelineEnd - DEFAULT_LEN, mid - DEFAULT_LEN / 2));
      start = snap(start);
      range = { start, end: snap(start + DEFAULT_LEN) };
      renderTimeline();
      updateZoom();
    });

    // Drag handlers for main selection
    function onPointerMove(ev) {
      if (!drag) return;
      ev.preventDefault();
      const dy = ev.clientY - drag.y0;
      const dMin = Math.round((dy / ppm) / STEP) * STEP;

      if (drag.mode === 'move') {
        const len = drag.r0.end - drag.r0.start;
        let s = drag.r0.start + dMin;
        s = Math.max(timelineStart, Math.min(timelineEnd - len, s));
        s = snap(s);
        range = { start: s, end: snap(s + len) };
      } else if (drag.mode === 'start') {
        let s = drag.r0.start + dMin;
        s = Math.max(timelineStart, Math.min(drag.r0.end - MIN_LEN, s));
        range = { start: snap(s), end: snap(drag.r0.end) };
      } else if (drag.mode === 'end') {
        let e = drag.r0.end + dMin;
        e = Math.min(timelineEnd, Math.max(drag.r0.start + MIN_LEN, e));
        range = { start: snap(drag.r0.start), end: snap(e) };
      }
      renderTimeline();
      updateZoom();
    }

    function onPointerUp() {
      drag = null;
      ctx.bodyClass('tr-dragging', false);
      window.removeEventListener('pointermove', onPointerMove);
      window.removeEventListener('pointerup', onPointerUp);
    }

    function beginDrag(mode) {
      return (ev) => {
        ev.stopPropagation();
        ev.preventDefault();
        ctx.bodyClass('tr-dragging', true);
        drag = { mode, y0: ev.clientY, r0: { ...range } };
        ctx.on(window, 'pointermove', onPointerMove);
        ctx.on(window, 'pointerup', onPointerUp);
      };
    }

    sel.addEventListener('pointerdown', beginDrag('move'));
    sel.querySelector('[data-handle="start"]').addEventListener('pointerdown', beginDrag('start'));
    sel.querySelector('[data-handle="end"]').addEventListener('pointerdown', beginDrag('end'));

    function getMonitorPosition(monitor) {
      if (!monitor) return null;
      // Extract position from monitor string (e.g., "center_DP-3" -> "Center")
      const pos = monitor.split('_')[0];
      if (!pos) return null;
      // Capitalize first letter
      return pos.charAt(0).toUpperCase() + pos.slice(1);
    }

    const TRASH_COPY = transcriptTrashCopy();

    deleteSegmentModalErase.textContent = TRASH_COPY.buttons.eraseNow;
    deleteSegmentModalCancel.textContent = TRASH_COPY.buttons.cancel;
    deleteSegmentModalConfirm.textContent = TRASH_COPY.buttons.moveToTrash;
    deleteSegmentModalBack.textContent = TRASH_COPY.buttons.back;
    deleteSegmentModalEraseConfirm.textContent = TRASH_COPY.buttons.eraseNow;

    function deleteDialogElement(tag, className, text) {
      const node = document.createElement(tag);
      if (className) node.className = className;
      if (text != null) node.textContent = text;
      return node;
    }

    // What a delete takes from each segment, so the confirm states its reach:
    // a screenshot's segment holds its screenshot and what was made from it.
    // Where the page knows a segment's kinds it lists only what it holds, and a
    // photo's is its photo; otherwise audio, screen frames and a transcript.
    function segmentKinds(seg) {
      if (isScreenshotsStream(seg.stream)) return { screenshot: true };
      const types = Array.isArray(seg.types) ? seg.types : null;
      if (!types) return null;
      const audio = types.includes('audio');
      const screen = types.includes('screen');
      if (isPhotoSegment(seg)) return { photo: true };
      // Saved written messages, or a conversation the journal can't place,
      // are the segment's own: no audio or frames to take.
      const kind = conversationKind(seg);
      if (!audio && !screen && (kind === 'messages' || kind === 'conversation')) return { [kind]: true };
      if (!audio && !screen) return null;
      return { audio, screen, transcript: true };
    }

    function isPhotoSegment(seg) {
      const types = Array.isArray(seg.types) ? seg.types : [];
      return types.includes('image') && !['audio', 'screen', 'markdown', 'browser'].some((kind) => types.includes(kind));
    }

    function segmentParts(segs, holdsLoc) {
      const kinds = segs.map((seg) => segmentKinds(seg) || { audio: true, screen: true, transcript: true });
      const any = (kind) => kinds.some((held) => held[kind]);
      return [
        ...(any('audio') ? ['audio'] : []),
        ...(any('screen') ? ['screen'] : []),
        ...(any('photo') ? ['photo'] : []),
        ...(any('screenshot') ? ['screenshot'] : []),
        ...(any('transcript') ? ['transcript'] : []),
        ...(any('messages') ? ['messages'] : []),
        ...(any('conversation') ? ['conversation'] : []),
        'analysis',
        ...(holdsLoc ? ['location'] : []),
      ];
    }

    // What the dialog's segments would take of the imports that made them, read
    // when it opens: null until the journal answers, or if it doesn't.
    let dialogImports = null;
    const BODY_IMPORT_STREAMS = new Set(['import.strava', 'import.apple_health', 'import.oura']);
    const isImportStream = (stream) => typeof stream === 'string' && stream.startsWith('import.') && !BODY_IMPORT_STREAMS.has(stream);

    function readDialogImports(segs) {
      dialogImports = null;
      const asked = segs;
      fetch('/app/transcripts/api/segments/imports', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', accept: 'application/json' },
        body: JSON.stringify({ segments: segs.map((seg) => ({ day, stream: seg.stream, key: seg.key })) }),
      })
        .then(response => (response.ok ? response.json() : null))
        .then((answer) => {
          if (!Array.isArray(answer?.imports) || deleteDialogSegs !== asked) return;
          dialogImports = answer.imports;
          if (deleteSegmentModal.style.display !== 'none') renderDeleteStep(deleteSegmentModalBody.dataset.step || 'move');
        })
        .catch(() => {});
    }

    // The paragraph about the import, with its link when the owner should go there.
    function importLine(text, href) {
      if (!href) return deleteDialogElement('p', 'tr-delete-import', text);
      const line = deleteDialogElement('p', 'tr-delete-import');
      const link = deleteDialogElement('a', null, TRASH_COPY.imports.openImport);
      link.href = href;
      line.append(deleteDialogElement('span', null, `${text} `), link);
      return line;
    }

    // Which of a string's forms the kept upload decides: its name, one, several,
    // none, or unknown when the journal didn't say.
    function keptForm(imp, withNone = true) {
      const kept = imp?.kept;
      if (typeof kept !== 'number') return 'unknown';
      if (kept >= 1 && imp.label) return 'label';
      if (kept === 1) return 'one';
      if (kept > 1) return 'many';
      return withNone ? 'none' : null;
    }

    // The lines a delete dialog adds after its reach about the imports it takes
    // or leaves. One segment says only when one import made it.
    function importLines(segs, step) {
      const C = TRASH_COPY.imports;
      const erasing = step === 'erase';
      if (dialogImports === null) {
        if (!segs.some((seg) => isImportStream(seg.stream))) return [];
        const maybe = segs.length === 1 ? C.importMaybe : C.setMaybe;
        return [importLine(erasing ? maybe.erase : maybe.move)];
      }
      if (segs.length === 1) {
        const imp = dialogImports[0];
        if (!imp) return [];
        if (imp.takes) {
          const form = keptForm(imp);
          const table = erasing ? C.importErased : C.importGoes;
          const lines = [importLine(renderTranscriptCopy(table[form], { n: imp.kept, label: imp.label || '' }))];
          if (erasing && imp.waiting > 0) {
            lines.push(importLine(renderTranscriptCopy(imp.waiting === 1 ? C.waitingErased.one : C.waitingErased.many, { w: imp.waiting })));
          }
          return lines;
        }
        const form = keptForm(imp, false);
        if (!form) return [];
        return [importLine(renderTranscriptCopy(C.importStays[form], { label: imp.label || '' }), `/app/import/${encodeURIComponent(imp.id)}`)];
      }
      const lines = [];
      const taken = dialogImports.filter((imp) => imp.takes);
      const left = dialogImports.filter((imp) => !imp.takes && imp.kept > 0);
      if (taken.length) {
        const table = erasing ? C.setErases : C.setTakes;
        const anyKept = taken.some((imp) => imp.kept > 0);
        let key;
        if (taken.length === 1) key = taken[0].kept >= 1 && taken[0].label ? 'label' : (anyKept ? 'one' : 'oneNone');
        else key = anyKept ? 'many' : 'manyNone';
        lines.push(importLine(renderTranscriptCopy(table[key], { k: taken.length, label: taken[0].label || '' })));
        const waiting = taken.reduce((sum, imp) => sum + (imp.waiting || 0), 0);
        if (erasing && waiting > 0) {
          const key = `${taken.length === 1 ? 'one' : 'many'}${waiting === 1 ? 'One' : 'Many'}`;
          lines.push(importLine(renderTranscriptCopy(C.setWaiting[key], { w: waiting })));
        }
      }
      if (left.length === 1) {
        lines.push(importLine(renderTranscriptCopy(left[0].label ? C.setStays.label : C.setStays.one, { label: left[0].label || '' })));
      } else if (left.length > 1) {
        lines.push(importLine(renderTranscriptCopy(C.setStays.many, { j: left.length })));
      }
      return lines;
    }

    // One step of the delete dialog: 'move' (the trash, the default) or 'erase'
    // (its own confirm). The segment's names are journal data, so they go in as text.
    function renderDeleteSegmentStep(seg, step) {
      const copy = TRASH_COPY[step];
      const isLocRow = Boolean(seg.locationRow);
      // What the page knows of a segment's files is the open segment's alone.
      const ownData = isOpenSegment(seg) ? segmentData : null;
      const holdsLoc = Boolean(ownData?.holds_location);
      const erasing = step === 'erase';

      deleteSegmentModalTitle.textContent = copy.title;
      deleteSegmentModalHeader.classList.toggle('danger', erasing);
      deleteSegmentModalBody.className = erasing ? 'delete-warning' : 'trash-dlg-note';
      deleteSegmentModalBody.dataset.step = step;
      deleteSegmentModalMoveFoot.hidden = erasing;
      deleteSegmentModalEraseFoot.hidden = !erasing;

      const meta = deleteDialogElement('p', 'tr-delete-segment-meta trash-dlg-meta');
      const streamName = deleteDialogElement('strong', null, importItemLabel(seg) || humanizeStreamName(seg.stream));
      streamName.setAttribute('title', seg.stream);
      const dayLabel = isDateOnlyImport(seg) ? window.JournalFormat.filedDay(day) : window.JournalFormat.day(day).toLowerCase();
      const dayName = deleteDialogElement('strong', null, dayLabel);
      const timeName = deleteDialogElement('strong', null, isLocRow || isScreenshotsStream(seg.stream) || isPhotoSegment(seg) ? seg.start : `${seg.start} - ${seg.end}`);
      meta.append(streamName, deleteDialogElement('span', null, ' · '), dayName);
      if (!isDateOnlyImport(seg)) meta.append(deleteDialogElement('span', null, ' · '), timeName);

      const parts = isLocRow ? ['location'] : segmentParts([seg], holdsLoc);
      const list = deleteDialogElement('ul', 'tr-delete-segment-list');
      parts.forEach(part => list.append(deleteDialogElement('li', null, TRASH_COPY.parts[part])));

      deleteSegmentModalBody.replaceChildren();
      deleteSegmentModalBody.append(
        meta,
        deleteDialogElement('p', 'tr-delete-reach', copy.reach),
        ...importLines([seg], step),
        ...(erasing && originalsInTrash.has(`${day}/${seg.stream}/${seg.key}`)
          ? [deleteDialogElement('p', 'tr-delete-originals', TRASH_COPY.originalsGo.one)]
          : []),
        deleteDialogElement('p', 'tr-delete-lead', copy.lead),
        list,
        deleteDialogElement('p', 'tr-delete-stays', copy.stays),
        deleteDialogElement('p', 'tr-delete-found', copy.found),
      );
      const totalBytes = (ownData?.media_sizes?.audio || 0) + (ownData?.media_sizes?.screen || 0);
      if (!erasing && !isLocRow && totalBytes > 0) {
        deleteSegmentModalBody.append(deleteDialogElement('p', 'tr-delete-segment-size trash-dlg-size', copy.size({ size: formatSize(totalBytes) })));
      }
    }

    // The segments the open dialog would delete: the open one, or what select
    // mode chose. One segment gets the single dialog; several, the set's.
    let deleteDialogSegs = null;

    function renderDeleteStep(step) {
      if (stretchDialog) renderStretchStep(step);
      else if (deleteDialogSegs.length === 1) renderDeleteSegmentStep(deleteDialogSegs[0], step);
      else renderDeleteSetStep(deleteDialogSegs, step);
      attachBackupChoice(step, stretchDialog ? 1 : deleteDialogSegs.length);
    }

    // The erase step offers to also remove it from the backups, while backup
    // is set up. Each render starts again unticked-or-as-set.
    let backupChoice = null;
    let backupChoiceTurn = 0;
    async function attachBackupChoice(step, count) {
      const turn = ++backupChoiceTurn;
      backupChoice = null;
      if (step !== 'erase' || !window.BackupErase) return;
      const row = await window.BackupErase.row(count);
      if (!row || turn !== backupChoiceTurn) return;
      backupChoice = row;
      deleteSegmentModalBody.append(row);
    }
    function backupsChosen() {
      return Boolean(window.BackupErase && window.BackupErase.chosen(backupChoice));
    }
    function backupsClause(count) {
      return window.BackupErase ? window.BackupErase.receipt(backupChoice, count) : '';
    }

    // Segments whose original media waits in the trash, read when the dialog
    // opens, so an erase can say it goes too.
    let originalsInTrash = new Set();

    function readOriginalsInTrash() {
      originalsInTrash = new Set();
      fetch('/app/settings/api/trash', { headers: { accept: 'application/json' } })
        .then(response => (response.ok ? response.json() : null))
        .then((answer) => {
          originalsInTrash = new Set((answer?.items || [])
            .filter(item => item.kind === 'originals')
            .map(item => `${item.day}/${item.stream}/${item.segment}`));
        })
        .catch(() => {});
    }

    function showDeleteSegmentModal(segs = selectedSegment ? [selectedSegment] : null) {
      if (!segs || !segs.length) return;
      readOriginalsInTrash();

      deleteDialogSegs = segs;
      readDialogImports(segs);
      renderDeleteStep('move');
      deleteModalReturnFocus = document.activeElement;
      deleteModalPrevOverflow = document.body.style.overflow;
      deleteSegmentModal.style.display = 'flex';
      document.body.style.overflow = 'hidden';
      // The reversible act takes the focus.
      deleteSegmentModalConfirm.focus();
    }

    function showEraseSegmentStep() {
      if ((!deleteDialogSegs && !stretchDialog) || deleteSegmentModal.style.display === 'none') return;
      renderDeleteStep('erase');
      deleteSegmentModalBack.focus();
    }

    function backFromEraseSegmentStep() {
      if ((!deleteDialogSegs && !stretchDialog) || deleteSegmentModal.style.display === 'none') return;
      renderDeleteStep('move');
      deleteSegmentModalErase.focus();
    }

    function closeDeleteSegmentModal(restoreFocus = true) {
      if (deleteSegmentModal.style.display === 'none') return;

      deleteSegmentModal.style.display = 'none';
      deleteSegmentModalBody.replaceChildren();
      deleteDialogSegs = null;
      if (stretchDialog) clearTimeout(stretchDialog.timer);
      stretchDialog = null;
      deleteSegmentModalConfirm.disabled = false;
      deleteSegmentModalErase.disabled = false;
      document.body.style.overflow = deleteModalPrevOverflow;
      deleteModalPrevOverflow = '';
      if (restoreFocus && deleteModalReturnFocus && document.contains(deleteModalReturnFocus)) {
        deleteModalReturnFocus.focus();
      }
      deleteModalReturnFocus = null;
    }

    async function confirmDeleteSegment(erase = false) {
      if (stretchDialog) {
        // Only what the dialog shows now is ever sent: the bounds its answer
        // was read for, and only while the fields still say them.
        const dialog = stretchDialog;
        const shownFor = dialog.planFor;
        if (dialog.state !== 'ready' || !dialog.plan || dialog.plan.empty || !shownFor
          || shownFor.from !== dialog.from || shownFor.to !== dialog.to) return;
        const asked = { from: shownFor.from, to: shownFor.to, shown: dialog.plan };
        closeDeleteSegmentModal(false);
        if (stretchButton) stretchButton.dataset.focusOnShow = '1';
        await performStretchDelete(asked, erase);
        syncStretchButton();
        return;
      }
      const segs = deleteDialogSegs;
      if (!segs) return;

      closeDeleteSegmentModal(false);
      if (segPick.on) await performPickDelete(segs, erase);
      else await performDeleteSegment(segs[0], erase);
    }

    async function confirmEraseSegment() {
      await confirmDeleteSegment(true);
    }

    deleteSegmentModalClose.addEventListener('click', () => closeDeleteSegmentModal());
    deleteSegmentModalCancel.addEventListener('click', () => closeDeleteSegmentModal());
    deleteSegmentModalConfirm.addEventListener('click', () => confirmDeleteSegment(false));
    deleteSegmentModalErase.addEventListener('click', () => showEraseSegmentStep());
    deleteSegmentModalBack.addEventListener('click', () => backFromEraseSegmentStep());
    deleteSegmentModalEraseConfirm.addEventListener('click', () => confirmEraseSegment());

    // ── deleting a stretch of time ──────────────────────────────────────────
    // A from and a to, in the journal's wall time; everything in the journal
    // from then moves to the trash as one thing. The journal answers what it
    // would take, and the dialog shows only that answer's bounds and kinds,
    // never a count.
    const STRETCH_COPY = {
      button: window.JournalI18n.render('journal.window.transcripts.stretch.copy.button.delete.a.stretch.of.time'),
      from: window.JournalI18n.render('journal.window.transcripts.stretch.copy.from.from'),
      to: window.JournalI18n.render('journal.window.transcripts.stretch.copy.to.to'),
      zone: ({ zone }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.journal.zone', [['journal_zone', String(zone ?? '')]]),
      checking: window.JournalI18n.render('journal.window.transcripts.stretch.copy.checking.checking.what.s.in.your.journal'),
      move: {
        title: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.title.delete.a.stretch.of.time'),
        bounds: ({ from, to }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.move.range', [['start_time', String(from ?? '')], ['end_time', String(to ?? '')]]),
        reach: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.reach.it.all.leaves.your.journal.now.variant2'),
        madeChanges: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.madechanges.activities.made.from.it.drop.it'),
        made: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.made.the.entities.you.re.tending.and'),
        madeBody: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.madebody.activities.summaries.and.entities.made.from'),
      },
      erase: {
        title: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.title.erase.this.stretch.of.time.now'),
        bounds: ({ from, to }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.erase.range', [['start_time', String(from ?? '')], ['end_time', String(to ?? '')]]),
        reach: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.reach.it.all.leaves.your.journal.now'),
        madeChanges: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.madechanges.activities.made.from.it.drop.it'),
        made: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.made.the.entities.you.re.tending.and.variant2'),
        madeBody: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.madebody.activities.summaries.and.entities.made.from.variant2'),
      },
      kinds: ({ kinds }) => window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.content.kinds', [['content_kinds', String(kinds ?? '')]]),
      kindWords: {
        audio: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.audio.audio'),
        screen: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.screen.screen'),
        location: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.location.location'),
        body: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.body.body.data'),
        imported: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.imported.things.you.imported'),
        other: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.other.other.things.in.your.journal'),
      },
      staysLead: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.stayslead.these.stay.in.your.journal'),
      stays: {
        arriving: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.arriving.anything.still.arriving.from.a.device'),
        processing: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.processing.anything.your.journal.is.still.working'),
        import_running: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.import.running.anything.from.an.import.that.s'),
        whole_day: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.whole.day.whole.day.body.data.like.a'),
        in_trash: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.in.trash.anything.from.then.that.s.already'),
        memories: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.memories.memories.your.agents.saved.about.that'),
      },
      inFlight: window.JournalI18n.renderEnglish('journal.window.transcripts.consent.stretch.copy.inflight.to.move.those.delete.this.stretch'),
      emptyNone: window.JournalI18n.render('journal.window.transcripts.stretch.copy.emptynone.your.journal.has.nothing.from.that'),
      emptyLeft: window.JournalI18n.render('journal.window.transcripts.stretch.copy.emptyleft.nothing.from.that.time.can.go'),
      backwards: window.JournalI18n.render('journal.window.transcripts.stretch.copy.backwards.the.end.has.to.be.after'),
      tooLong: window.JournalI18n.render('journal.window.transcripts.stretch.copy.toolong.choose.days.or.less'),
      failed: window.JournalI18n.render('journal.window.transcripts.stretch.copy.failed.couldn.t.check.what.s.in'),
      retry: window.JournalI18n.render('journal.window.transcripts.stretch.copy.retry.try.again'),
      receipt: {
        moved: { title: ({ bounds }) => window.JournalI18n.render('journal.window.transcripts.stretch.receipt.move.range', [['date_range', String(bounds ?? '')]]), message: ({ date }) => window.JournalI18n.render('journal.window.transcripts.stretch.receipt.erase.date', [['erase_date', String(date ?? '')]]) },
        partMoved: { title: ({ bounds }) => window.JournalI18n.render('journal.window.transcripts.stretch.receipt.move.range', [['date_range', String(bounds ?? '')]]), message: window.JournalI18n.render('journal.window.transcripts.stretch.copy.message.some.of.it.couldn.t.be') },
        erased: { title: ({ bounds }) => window.JournalI18n.render('journal.window.transcripts.stretch.receipt.erase.range', [['date_range', String(bounds ?? '')]]) },
        recovered: { title: ({ bounds }) => window.JournalI18n.render('journal.window.transcripts.stretch.receipt.recover.range', [['date_range', String(bounds ?? '')]]), message: window.JournalI18n.render('journal.window.transcripts.stretch.copy.message.it.s.back.in.your.journal') },
        recoverBlocked: { title: window.JournalI18n.render('journal.window.transcripts.stretch.copy.title.couldn.t.recover.that'), message: window.JournalI18n.render('journal.window.transcripts.stretch.copy.message.part.of.it.comes.from.an') },
        recoverFinishing: { title: window.JournalI18n.render('journal.window.transcripts.stretch.copy.title.couldn.t.recover.that.yet'), message: window.JournalI18n.render('journal.window.transcripts.stretch.copy.message.it.s.still.moving.into.the') },
        empty: { title: window.JournalI18n.render('journal.window.transcripts.stretch.copy.title.nothing.was.moved'), message: window.JournalI18n.render('journal.window.transcripts.stretch.copy.message.nothing.from.that.time.could.go') },
      },
      inTrash: {
        heading: window.JournalI18n.render('journal.window.transcripts.stretch.copy.heading.this.segment.is.in.the.trash'),
        stretch: ({ bounds, date }) => window.JournalI18n.render('journal.window.transcripts.stretch.removed.range.and.date', [['date_range', String(bounds ?? '')], ['erase_date', String(date ?? '')]]),
        alone: ({ date }) => window.JournalI18n.render('journal.window.transcripts.stretch.removed.erase.date', [['erase_date', String(date ?? '')]]),
      },
    };
    const STRETCH_KIND_ORDER = ['audio', 'screen', 'location', 'body', 'imported', 'other'];
    const STRETCH_WHY_ORDER = ['arriving', 'processing', 'import_running', 'whole_day'];
    const STRETCH_MAX_DAYS = 31;

    let stretchDialog = null;
    const stretchButton = document.getElementById('trStretchBtn');

    // A wall time `YYYY-MM-DDTHH:MM[:SS]` as a Date on the browser's clock that
    // reads the same digits, for formatting only.
    function stretchWall(value) {
      const match = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})/.exec(value || '');
      if (!match) return null;
      const [, y, mo, d, h, mi] = match.map(Number);
      return { date: new Date(y, mo - 1, d, h, mi), day: `${match[1]}${match[2]}${match[3]}` };
    }

    const stretchTimeFormat = new Intl.DateTimeFormat(undefined, { hour: 'numeric', minute: '2-digit' });

    function stretchTime(at) {
      return stretchTimeFormat.format(at.date).toLowerCase();
    }

    function stretchDayWord(at, sourceEnglish = false) {
      return window.JournalFormat.day(at.day, undefined, { sourceEnglish }).toLowerCase();
    }

    // `sat nov 7, 2:05 – 3:35 pm`, or across days `sat nov 7, 11:00 pm – sun nov 8, 1:10 am`.
    function stretchBounds(from, to) {
      const a = stretchWall(from);
      const b = stretchWall(to);
      if (!a || !b) return '';
      if (a.day === b.day) {
        const range = typeof stretchTimeFormat.formatRange === 'function'
          ? stretchTimeFormat.formatRange(a.date, b.date).toLowerCase()
          : `${stretchTime(a)} – ${stretchTime(b)}`;
        return `${stretchDayWord(a)}, ${range}`;
      }
      return `${stretchDayWord(a)}, ${stretchTime(a)} – ${stretchDayWord(b)}, ${stretchTime(b)}`;
    }

    // The bounds sentence's halves: `{day}, {t1}` and `{t2}` on the same day.
    function stretchFromTo(from, to) {
      const a = stretchWall(from);
      const b = stretchWall(to);
      if (!a || !b) return { from: '', to: '' };
      return {
        from: `${stretchDayWord(a, true)}, ${stretchTime(a)}`,
        to: a.day === b.day ? stretchTime(b) : `${stretchDayWord(b, true)}, ${stretchTime(b)}`,
      };
    }

    function stretchKindsText(kinds) {
      const words = STRETCH_KIND_ORDER.filter((kind) => kinds.includes(kind)).map((kind) => STRETCH_COPY.kindWords[kind]);
      if (!words.length) return '';
      const list = words.length === 1 ? words[0] : `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;
      return renderTranscriptCopy(STRETCH_COPY.kinds, { kinds: list });
    }

    function minutesWall(dayKey, minutes) {
      const clamped = Math.max(0, Math.min(minutes, 24 * 60 - 1));
      const hh = String(Math.floor(clamped / 60)).padStart(2, '0');
      const mm = String(clamped % 60).padStart(2, '0');
      return `${dayKey.slice(0, 4)}-${dayKey.slice(4, 6)}-${dayKey.slice(6, 8)}T${hh}:${mm}`;
    }

    // What the fields say is wrong, before asking the journal.
    function stretchFieldError(from, to) {
      const a = stretchWall(from);
      const b = stretchWall(to);
      if (!a || !b) return null;
      if (b.date <= a.date) return STRETCH_COPY.backwards;
      if (b.date - a.date > STRETCH_MAX_DAYS * 86400000) return STRETCH_COPY.tooLong;
      return null;
    }

    function showStretchDialog() {
      readOriginalsInTrash();
      stretchDialog = {
        from: minutesWall(day, range.start),
        to: minutesWall(day, range.end),
        plan: null,
        planFor: null,
        state: 'checking',
        error: null,
        timer: null,
        asked: 0,
      };
      deleteDialogSegs = null;
      renderStretchStep('move');
      deleteModalReturnFocus = stretchButton || document.activeElement;
      deleteModalPrevOverflow = document.body.style.overflow;
      deleteSegmentModal.style.display = 'flex';
      document.body.style.overflow = 'hidden';
      deleteSegmentModalBody.querySelector('#trStretchFrom')?.focus();
      previewStretch();
    }

    // Ask the journal what the fields would take; an answer for fields that
    // have changed since is dropped.
    function previewStretch() {
      const dialog = stretchDialog;
      if (!dialog) return;
      const error = stretchFieldError(dialog.from, dialog.to);
      dialog.error = error;
      dialog.plan = null;
      if (error || !stretchWall(dialog.from) || !stretchWall(dialog.to)) {
        dialog.state = error ? 'invalid' : 'checking';
        renderStretchStep('move');
        return;
      }
      dialog.state = 'checking';
      const asked = ++dialog.asked;
      const bounds = { from: dialog.from, to: dialog.to };
      renderStretchStep('move');
      fetch('/app/transcripts/api/stretch/preview', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json', accept: 'application/json' },
        body: JSON.stringify({ from: dialog.from, to: dialog.to }),
      })
        .then(async (response) => {
          const answer = await response.json().catch(() => null);
          if (stretchDialog !== dialog || dialog.asked !== asked) return;
          if (!response.ok) {
            const code = answer?.reason_code;
            if (code === 'stretch_backwards') { dialog.error = STRETCH_COPY.backwards; dialog.state = 'invalid'; }
            else if (code === 'stretch_too_long') { dialog.error = STRETCH_COPY.tooLong; dialog.state = 'invalid'; }
            else dialog.state = 'failed';
          } else {
            dialog.plan = answer;
            dialog.planFor = bounds;
            dialog.state = 'ready';
          }
          renderStretchStep(deleteSegmentModalBody.dataset.step || 'move');
        })
        .catch(() => {
          if (stretchDialog !== dialog || dialog.asked !== asked) return;
          dialog.state = 'failed';
          renderStretchStep('move');
        });
    }

    function stretchField(id, label, value) {
      const wrap = deleteDialogElement('label', 'tr-stretch-field');
      wrap.htmlFor = id;
      wrap.append(deleteDialogElement('span', null, label));
      const input = document.createElement('input');
      input.type = 'datetime-local';
      input.step = '60';
      input.id = id;
      input.value = value;
      wrap.append(input);
      return { wrap, input };
    }

    // What was made from the stretch is made again only when it holds more
    // than body data: a briefing never reads body data, so nothing is asked.
    function stretchRemakes(plan) {
      return (plan?.kinds || []).some((kind) => kind !== 'body');
    }

    function stretchStaysList(plan, step) {
      const copy = STRETCH_COPY;
      const whys = new Set((plan?.left_out || []).map((left) => left.why));
      const items = STRETCH_WHY_ORDER.filter((why) => whys.has(why)).map((why) => copy.stays[why]);
      if (plan?.in_trash) items.push(copy.stays.in_trash);
      // A stretch of body data alone remakes nothing: it keeps today's line.
      items.push(copy.stays.memories, stretchRemakes(plan) ? copy[step].made : copy[step].madeBody);
      const list = deleteDialogElement('ul', 'tr-delete-segment-list');
      items.forEach((text) => list.append(deleteDialogElement('li', null, text)));
      const nodes = [deleteDialogElement('p', 'tr-delete-lead', copy.staysLead), list];
      if (['arriving', 'processing', 'import_running'].some((why) => whys.has(why))) {
        nodes.push(deleteDialogElement('p', 'tr-delete-stays', copy.inFlight));
      }
      return nodes;
    }

    function renderStretchStep(step) {
      const dialog = stretchDialog;
      if (!dialog) return;
      const copy = STRETCH_COPY[step];
      const erasing = step === 'erase';
      const plan = dialog.state === 'ready' ? dialog.plan : null;
      deleteSegmentModalTitle.textContent = copy.title;
      deleteSegmentModalHeader.classList.toggle('danger', erasing);
      deleteSegmentModalBody.className = erasing ? 'delete-warning' : 'trash-dlg-note';
      deleteSegmentModalBody.dataset.step = step;
      deleteSegmentModalMoveFoot.hidden = erasing;
      deleteSegmentModalEraseFoot.hidden = !erasing;
      const actionable = Boolean(plan && !plan.empty);
      deleteSegmentModalConfirm.disabled = !actionable;
      deleteSegmentModalErase.disabled = !actionable;
      deleteSegmentModalEraseConfirm.disabled = !actionable;

      const nodes = [];
      if (!erasing) {
        // The fields keep what the owner typed, and focus, across a re-render.
        const focused = document.activeElement?.id;
        const fields = deleteDialogElement('div', 'tr-stretch-fields');
        const from = stretchField('trStretchFrom', STRETCH_COPY.from, dialog.from);
        const to = stretchField('trStretchTo', STRETCH_COPY.to, dialog.to);
        fields.append(from.wrap, to.wrap);
        nodes.push(fields);
        if (dialog.error) {
          const err = deleteDialogElement('p', 'tr-stretch-err', dialog.error);
          err.id = 'trStretchErr';
          to.input.setAttribute('aria-invalid', 'true');
          to.input.setAttribute('aria-describedby', 'trStretchErr');
          nodes.push(err);
        }
        const journalZone = window.JournalClock?.zone?.();
        const browserZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
        if (journalZone && browserZone && journalZone !== browserZone) {
          nodes.push(deleteDialogElement('p', 'tr-delete-stays', renderTranscriptCopy(STRETCH_COPY.zone, { zone: journalZone })));
        }
        // A change makes what the dialog shows no longer what it would take:
        // the answer it showed, and any answer still on its way, are dropped,
        // and neither act is offered until the new bounds have been read.
        const onChange = () => {
          dialog.from = from.input.value;
          dialog.to = to.input.value;
          dialog.asked += 1;
          dialog.plan = null;
          dialog.planFor = null;
          dialog.state = 'checking';
          deleteSegmentModalConfirm.disabled = true;
          deleteSegmentModalErase.disabled = true;
          deleteSegmentModalEraseConfirm.disabled = true;
          while (fields.nextSibling) fields.nextSibling.remove();
          deleteSegmentModalBody.append(deleteDialogElement('p', 'tr-stretch-checking', STRETCH_COPY.checking));
          clearTimeout(dialog.timer);
          dialog.timer = setTimeout(previewStretch, 400);
        };
        from.input.addEventListener('input', onChange);
        to.input.addEventListener('input', onChange);
        deleteSegmentModalBody.replaceChildren(...nodes);
        if (focused === 'trStretchFrom') from.input.focus();
        if (focused === 'trStretchTo') to.input.focus();
        nodes.length = 0;
      }
      if (dialog.state === 'checking') {
        nodes.push(deleteDialogElement('p', 'tr-stretch-checking', STRETCH_COPY.checking));
      } else if (dialog.state === 'failed') {
        const line = deleteDialogElement('p', 'tr-stretch-checking');
        const retry = deleteDialogElement('button', 'tr-stretch-retry', STRETCH_COPY.retry);
        retry.type = 'button';
        retry.addEventListener('click', previewStretch);
        line.append(deleteDialogElement('span', null, `${STRETCH_COPY.failed} `), retry);
        nodes.push(line);
      } else if (plan && plan.empty) {
        const left = (plan.left_out || []).length > 0;
        nodes.push(deleteDialogElement('p', 'tr-delete-reach', left ? STRETCH_COPY.emptyLeft : STRETCH_COPY.emptyNone));
        if (left) nodes.push(...stretchStaysList(plan, step));
      } else if (plan) {
        const halves = stretchFromTo(plan.from, plan.to);
        nodes.push(
          deleteDialogElement('p', 'tr-stretch-bounds', renderTranscriptCopy(copy.bounds, halves)),
          deleteDialogElement('p', 'tr-delete-reach', copy.reach),
        );
        const kinds = stretchKindsText(plan.kinds || []);
        if (kinds) nodes.push(deleteDialogElement('p', 'tr-delete-lead', kinds));
        dialogImports = Array.isArray(plan.imports) ? plan.imports : [];
        nodes.push(...importLines([{}, {}], step));
        if (stretchRemakes(plan)) {
          nodes.push(deleteDialogElement('p', 'tr-delete-stays tr-stretch-made', copy.madeChanges));
        }
        nodes.push(...stretchStaysList(plan, step));
      }
      if (erasing) deleteSegmentModalBody.replaceChildren(...nodes);
      else deleteSegmentModalBody.append(...nodes);
    }

    function stretchReceipt(kind, fill = {}, buttons = []) {
      const copy = STRETCH_COPY.receipt[kind];
      const settled = kind === 'moved' || kind === 'recovered' || kind === 'erased';
      const notice = {
        app: 'transcripts',
        icon: settled ? (kind === 'recovered' ? 'undo-2' : 'trash-2') : 'triangle-alert',
        title: renderTranscriptCopy(copy.title, fill),
        dismissible: true,
        buttons,
      };
      if (copy.message && (kind !== 'moved' || fill.date)) notice.message = renderTranscriptCopy(copy.message, fill);
      if (fill.message) notice.message = fill.message;
      if (settled) notice.autoDismiss = 10000;
      window.AppServices.notifications.show(notice);
    }

    // The receipt's recover is the trash page's recover of the stretch, whole.
    function stretchRecoverClick(stretch, bounds) {
      return async function () {
        let response;
        try {
          response = await fetch('/app/settings/api/trash/recover', {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ ids: [stretch.id], page: 'transcripts' }),
          });
        } catch (_) {
          trashReceipt('noAnswerRecover');
          return;
        }
        const answer = await response.json().catch(() => null);
        const failed = Array.isArray(answer?.failed) ? answer.failed : [];
        if (!response.ok || !answer) {
          trashReceipt('recoverFailed', {}, [openTrashButton()]);
          return;
        }
        if (failed.length) {
          if (failed.some((row) => row.reason === 'import_in_trash')) stretchReceipt('recoverBlocked', {}, [openTrashButton()]);
          else if (failed.some((row) => row.reason === 'stretch_finishing')) stretchReceipt('recoverFinishing', {}, [openTrashButton()]);
          else trashReceipt('recoverFailed', {}, [openTrashButton()]);
          return;
        }
        stretchReceipt('recovered', { bounds });
        reloadAfterStretch();
      };
    }

    // The day's lists read again: what the stretch took has left them.
    function reloadAfterStretch() {
      if (selectedSegment) clearSegmentSelection();
      loadDayData();
      loadLocationData();
      refreshSegmentIndicators();
    }

    // Send what the owner typed: the journal plans again at confirm, and the
    // receipt shows what it took.
    async function performStretchDelete(asked, erase) {
      let response;
      try {
        response = await fetch('/app/transcripts/api/stretch/delete', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ from: asked.from, to: asked.to, erase, ...(erase ? { backups: backupsChosen() } : {}) }),
        });
      } catch (_) {
        trashReceipt(erase ? 'noAnswerErase' : 'noAnswerMove');
        return;
      }
      const data = await response.json().catch(() => null);
      if (!response.ok) {
        const code = data?.reason_code;
        if (code === 'stretch_empty') stretchReceipt('empty');
        else trashReceipt(erase ? 'eraseRefused' : 'moveRefused');
        return;
      }
      if (!data || data.success !== true || !data.stretch) {
        trashReceipt(erase ? 'noAnswerErase' : 'noAnswerMove');
        return;
      }
      const bounds = stretchBounds(data.stretch.from, data.stretch.to);
      reloadAfterStretch();
      if (erase && data.erased === true) {
        stretchReceipt('erased', { bounds, message: backupsClause(1) });
        return;
      }
      if (erase) {
        trashReceipt('eraseFailed', {}, [openTrashButton()]);
        return;
      }
      const refused = Array.isArray(data.refused) && data.refused.length > 0;
      stretchReceipt(refused ? 'partMoved' : 'moved', { bounds, date: trashEraseDate(data.erase_at) }, [
        { label: TRASH_COPY.buttons.recover, onClick: stretchRecoverClick(data.stretch, bounds) },
        openTrashButton(),
      ]);
    }

    function showSegmentInTrash(stream, key) {
      const asked = day;
      dayRead(`/app/transcripts/api/segment/${day}/${encodeURIComponent(stream)}/${encodeURIComponent(key)}`)
        .then((data) => {
          if (!data || !data.trash || asked !== day || selectedSegment) return;
          panel.innerHTML = segmentInTrashHtml(data.trash);
          panel.querySelector('[data-open-trash]')?.addEventListener('click', goToSettingsTrash);
        })
        .catch(() => {});
    }

    // A segment the page was opened at that waits in the trash.
    function segmentInTrashHtml(trash) {
      const date = trashEraseDate(trash.erase_at);
      const desc = trash.stretch
        ? renderTranscriptCopy(STRETCH_COPY.inTrash.stretch, { bounds: stretchBounds(trash.stretch.from, trash.stretch.to), date })
        : renderTranscriptCopy(STRETCH_COPY.inTrash.alone, { date });
      const label = document.createElement('span');
      label.textContent = TRASH_COPY.buttons.openTrash;
      return window.SurfaceState.empty({
        icon: window.ConveyIcons.svg('trash-2'),
        heading: STRETCH_COPY.inTrash.heading,
        desc,
        action: `<button type="button" class="btn-secondary" data-open-trash>${label.innerHTML}</button>`,
      });
    }

    if (stretchButton) {
      const label = stretchButton.querySelector('[data-stretch-label]');
      if (label) label.textContent = STRETCH_COPY.button;
      const icon = stretchButton.querySelector('[data-stretch-icon]');
      if (icon && window.ConveyIcons) icon.innerHTML = window.ConveyIcons.svg('trash-2');
      stretchButton.addEventListener('click', showStretchDialog);
      syncStretchButton();
    }

    // Shown once the day has loaded, on a day with anything listed.
    // Called from the day's load, which can run before this block does, so it
    // reads nothing declared here.
    function syncStretchButton() {
      const button = document.getElementById('trStretchBtn');
      if (!button) return;
      const shown = dayLoad === 'ready' && (allSegments.length > 0 || locationSegments.length > 0);
      button.hidden = !shown;
      // After a stretch delete, focus returns here once the reloaded day shows it.
      if (shown && button.dataset.focusOnShow) {
        delete button.dataset.focusOnShow;
        button.focus();
      }
      if (day) {
        const label = window.JournalI18n.render('journal.window.transcripts.stretch.delete.start.label', [
          ['day', window.JournalFormat.day(day).toLowerCase()],
        ]);
        button.setAttribute('aria-label', label);
      }
    }

    function sortSegmentsByKey() {
      allSegments.sort((a, b) => a.key.localeCompare(b.key));
    }

    function refreshSegmentIndicators() {
      return dayFetch(`/app/transcripts/api/ranges/${day}`)
        .then(r => r.ok ? r.json() : Promise.reject(window.JournalI18n.render('journal.window.transcripts.window.explanation.failed.to.fetch.ranges')))
        .then(data => {
          drawDayIndicators(data, allSegments);
          dimLeftOut();
          renderBodyEvents(dayBodyWindow);
          renderLegend();
          renderLatest();
        })
        .catch(() => {
          // Range indicators may be stale, but the main segment state is already updated.
        });
    }

    function trashReceipt(kind, fill = {}, buttons = []) {
      const copy = TRASH_COPY.receipt[kind];
      const settled = kind === 'moved' || kind === 'recovered' || kind === 'erased';
      const notice = {
        app: 'transcripts',
        icon: settled ? (kind === 'recovered' ? 'undo-2' : 'trash-2') : 'triangle-alert',
        title: renderTranscriptCopy(copy.title, fill),
        dismissible: true,
        buttons,
      };
      if (copy.message && (kind !== 'moved' || fill.date)) notice.message = renderTranscriptCopy(copy.message, fill);
      if (fill.message) notice.message = fill.message;
      // A refused or failed card stays until the owner dismisses it.
      if (settled) notice.autoDismiss = 10000;
      window.AppServices.notifications.show(notice);
    }

    function openTrashButton() {
      return { label: TRASH_COPY.buttons.openTrash, onClick: goToSettingsTrash };
    }

    // The erase date on the journal's own clock, worded as the trash page words it.
    function trashEraseDate(eraseAt) {
      const instant = new Date(eraseAt);
      if (!eraseAt || Number.isNaN(instant.getTime())) return '';
      const key = window.JournalClock
        ? window.JournalClock.dayKey(instant)
        : `${instant.getFullYear()}${String(instant.getMonth() + 1).padStart(2, '0')}${String(instant.getDate()).padStart(2, '0')}`;
      return window.JournalFormat.day(key).toLowerCase();
    }

    function dropDeletedSegment(seg) {
      if (seg.locationRow) {
        locationSegments = locationSegments.filter(s => s.key !== seg.key || s.stream !== seg.stream);
        clearSegmentSelection();
        return;
      }
      const wasOpen = isOpenSegment(seg);
      allSegments = allSegments.filter(s => s.key !== seg.key || s.stream !== seg.stream);
      if (wasOpen) clearSegmentSelection();
      else if (seg.item) refreshUnselectedLocation(null, true);
      buildZoomSegments();
      refreshSegmentIndicators();
    }

    function isOpenSegment(seg) {
      return Boolean(selectedSegment && seg && selectedSegment.key === seg.key && selectedSegment.stream === seg.stream
        && Boolean(selectedSegment.locationRow) === Boolean(seg.locationRow));
    }

    // Moves the segment to the trash, or erases it now. A non-200 answer means
    // nothing changed; no answer at all means it may have.
    async function performDeleteSegment(seg, erase = false) {
      const segDay = day;
      let response;
      try {
        response = await fetch(`/app/transcripts/api/segment/${day}/${seg.stream}/${seg.key}`, erase
          ? { method: 'DELETE', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ erase: true, backups: backupsChosen() }) }
          : { method: 'DELETE' });
      } catch (_) {
        trashReceipt(erase ? 'noAnswerErase' : 'noAnswerMove');
        return { kind: 'noAnswer' };
      }
      const data = await response.json().catch(() => null);
      if (!response.ok) {
        trashReceipt(erase ? 'eraseRefused' : 'moveRefused');
        return { kind: 'refused', key: data?.key };
      }
      if (!data || data.success !== true) {
        trashReceipt(erase ? 'noAnswerErase' : 'noAnswerMove');
        return { kind: 'noAnswer' };
      }

      dropDeletedSegment(seg);
      if (data.erased === true) {
        trashReceipt('erased', { message: backupsClause(1) });
        return { kind: 'erased' };
      }
      if (data.erase_failed === true) {
        trashReceipt('eraseFailed', {}, [openTrashButton()]);
        return { kind: 'eraseFailed' };
      }
      const trashId = data.trash?.id;
      const recoverCopy = { receipt: TRASH_COPY.receipt, buttons: TRASH_COPY.buttons };
      trashReceipt('moved', { date: trashEraseDate(data.trash?.erase_at) }, [
        { label: TRASH_COPY.buttons.recover, onClick: segmentRecoverClick(trashId, seg, segDay, recoverCopy) },
        openTrashButton(),
      ]);
      return { kind: 'moved' };
    }

    // The receipt's recover is the trash page's recover: the same files go back.
    async function recoverFromReceipt(trashId, seg, segDay) {
      let response;
      try {
        response = await fetch('/app/settings/api/trash/recover', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ ids: [trashId], page: 'transcripts' }),
        });
      } catch (_) {
        trashReceipt('noAnswerRecover');
        return;
      }
      const data = response.ok ? await response.json().catch(() => null) : null;
      if (response.ok && !data) {
        trashReceipt('noAnswerRecover');
        return;
      }
      if (Array.isArray(data?.done) && data.done.includes(trashId)) {
        restoreDeletedSegment(seg, segDay);
        trashReceipt('recovered');
        return;
      }
      trashReceipt('recoverFailed', {}, [openTrashButton()]);
    }

    // Put a segment back only on the day it was deleted from; the owner may have
    // moved on to another day, where the same key names a different segment.
    function restoreDeletedSegment(seg, segDay) {
      if (!seg || segDay !== day) return;
      if (seg.locationRow) {
        if (!locationSegments.some(existing => existing.key === seg.key && existing.stream === seg.stream)) {
          locationSegments.push(seg);
          locationSegments.sort((a, b) => a.start.localeCompare(b.start) || a.stream.localeCompare(b.stream) || a.key.localeCompare(b.key));
        }
        if (!selectedSegment) {
          refreshUnselectedLocation(null);
        }
        return;
      }
      if (!allSegments.some(existing => existing.key === seg.key && existing.stream === seg.stream)) {
        allSegments.push(seg);
        sortSegmentsByKey();
        if (seg.item) refreshUnselectedLocation(null, true);
      }
      buildZoomSegments();
      refreshSegmentIndicators();
    }

    ctx.global('closeDeleteSegmentModal', closeDeleteSegmentModal);
    ctx.global('confirmDeleteSegment', confirmDeleteSegment);
    ctx.global('selectLocationSegment', selectLocationSegment);

    // Clear selection and reset UI state without modifying browser history
    function resetSegmentSelection() {
      stopAnalyzePoller();
      selectedSegment = null;
      linkedMoment = null;
      segmentPicker.value = '';
      segmentData = null;
      currentVideoFiles = {};
      currentImageFiles = {};
      activeTab = null;
      inFlightSegmentRequest = null;
      tabPanes = {};
      tabEntryCounts = {};
      screenDecoded = false;
      frameCapture.clear();
      allScreenFrames = [];
      currentFrameIndex = -1;
      groupEntriesByIdx.clear();

      // Stop and clear audio player reference
      panel.querySelectorAll('audio').forEach(audio => audio.pause());

      // Hide delete button
      deleteBtn.classList.remove('visible');

      // Reset UI
      titleEl.textContent = window.JournalI18n.render('journal.window.transcripts.visible.text.transcripts');
      titleEl.removeAttribute('title');
      if (copyStreamBtn) copyStreamBtn.style.display = 'none';
      rangeText.textContent = '';
      tabsContainer.innerHTML = '';
      tabsContainer.classList.remove('visible');
      warningNotice.classList.remove('visible');
      refreshUnselectedLocation(window.SurfaceState.empty({
        icon: window.ConveyIcons.svg('clock'),
        heading: window.JournalI18n.render('journal.window.transcripts.owner.heading.your.day.at.a.glance'),
        desc: window.JournalI18n.render('journal.window.transcripts.owner.desc.pick.part.of.the.day.on')
      }));

      // Clear active state in zoom view
      zoomSegments.querySelectorAll('.tr-zoom-pill').forEach(pill => {
        pill.classList.remove('tr-active');
      });
    }

    function clearSegmentSelection() {
      resetSegmentSelection();
      ctx.history.replace(window.location.pathname + (showFilter === 'everything' ? '' : `?show=${showFilter}`));
    }

    // ----- Select mode: several segments of this range, deleted together -----
    function segPickRange(seg) {
      if (seg.item && !isDateOnlyImport(seg)) return importItemWhen(seg);
      // A screenshot is a moment, so its time alone.
      if (isScreenshotsStream(seg.stream)) return seg.start;
      return renderTranscriptCopy(TRASH_COPY.pickRange, { start: seg.start, end: seg.end });
    }

    function segPickNoteKey(key, erase) {
      if (key === 'busy') return 'busy';
      if (key === 'importing') return erase ? 'eraseImporting' : 'importing';
      return erase ? 'eraseOther' : 'other';
    }

    // One row per segment in this range, in the picker's order: its time and its
    // stream, and the note a refused one carries.
    function segPickRow(seg) {
      const id = segmentChoiceId(seg);
      const row = deleteDialogElement('label', 'trash-row');
      const box = document.createElement('input');
      box.type = 'checkbox';
      box.checked = segPick.chosen.has(id);
      box.dataset.pick = id;
      box.addEventListener('change', () => setSegPicked(seg, box.checked));
      const body = deleteDialogElement('span', 'trash-row-body');
      const line = deleteDialogElement('span', 'removal-line');
      line.append(
        deleteDialogElement('span', 'removal-what removal-time', segPickRange(seg)),
        deleteDialogElement('span', 'removal-qual', importItemLabel(seg) || humanizeStreamName(seg.stream)),
      );
      body.append(line);
      const note = segPick.notes.get(id);
      if (note) body.append(deleteDialogElement('span', 'tr-pick-note', TRASH_COPY.notes[note] || TRASH_COPY.notes.other));
      row.append(box, body);
      return row;
    }

    function paintSegPickBar() {
      const count = segPick.chosen.size;
      pickIdle.hidden = count > 0;
      pickSome.hidden = count === 0;
      pickAll.checked = false;
      pickAllLabel.textContent = TRASH_COPY.selectAll;
      pickCount.textContent = renderTranscriptCopy(TRASH_COPY.selected, { n: count });
      pickDelete.textContent = TRASH_COPY.delete;
      pickDoneIdle.textContent = TRASH_COPY.done;
      pickDoneSome.textContent = TRASH_COPY.done;
    }

    // Runs with every rebuild of the detail column. Moving the window keeps the
    // chosen segments still in the range and drops the rest.
    function renderSegPick(filtered) {
      selectBtn.textContent = TRASH_COPY.select;
      selectBtn.hidden = filtered.length === 0;
      if (!segPick.on) return;
      const inRange = new Set(filtered.map(segmentChoiceId));
      [...segPick.chosen.keys()].forEach((id) => {
        if (!inRange.has(id)) {
          segPick.chosen.delete(id);
          segPick.notes.delete(id);
        }
      });
      pickList.replaceChildren(...filtered.map(segPickRow));
      paintSegPickBar();
    }

    function setSegPicked(seg, picked) {
      const id = segmentChoiceId(seg);
      if (picked) segPick.chosen.set(id, seg);
      else segPick.chosen.delete(id);
      segPick.notes.delete(id);
      buildZoomSegments();
    }

    function toggleSegPick(seg) {
      setSegPicked(seg, !segPick.chosen.has(segmentChoiceId(seg)));
    }

    function setSegPickMode(on) {
      segPick.on = on;
      segPick.chosen.clear();
      segPick.notes.clear();
      contentEl.classList.toggle('tr-picking', on);
      pickPanel.hidden = !on;
      if (!on) pickList.replaceChildren();
      buildZoomSegments();
    }

    function enterSegPick() {
      setSegPickMode(true);
      pickBar.focus();
    }

    function exitSegPick() {
      setSegPickMode(false);
      selectBtn.focus();
    }

    function chosenSegments() {
      return filterSegmentsInRange().filter((seg) => segPick.chosen.has(segmentChoiceId(seg)));
    }

    // The size is the sum only when every chosen segment's size is known, and
    // the page knows only the open segment's.
    function knownSegmentBytes(seg) {
      if (!isOpenSegment(seg) || !segmentData?.media_sizes) return null;
      return (segmentData.media_sizes.audio || 0) + (segmentData.media_sizes.screen || 0);
    }

    // The set's own dialog: the day, up to five segments by time and stream, and
    // what moves, as the union of what each segment holds. Never a time span.
    function renderDeleteSetStep(segs, step) {
      const erasing = step === 'erase';
      const copy = TRASH_COPY.set;
      deleteSegmentModalTitle.textContent = renderTranscriptCopy(erasing ? copy.eraseTitle : copy.moveTitle, { n: segs.length });
      deleteSegmentModalHeader.classList.toggle('danger', erasing);
      deleteSegmentModalBody.className = erasing ? 'delete-warning' : 'trash-dlg-note';
      deleteSegmentModalBody.dataset.step = step;
      deleteSegmentModalMoveFoot.hidden = erasing;
      deleteSegmentModalEraseFoot.hidden = !erasing;

      const meta = deleteDialogElement('p', 'tr-delete-segment-meta trash-dlg-meta');
      meta.append(deleteDialogElement('strong', null, window.JournalFormat.day(day).toLowerCase()));
      const shown = deleteDialogElement('ul', 'trash-set-list');
      segs.slice(0, 5).forEach((seg) => {
        const item = deleteDialogElement('li');
        item.append(deleteDialogElement('strong', null, segPickRange(seg)), deleteDialogElement('span', null, ` · ${importItemLabel(seg) || humanizeStreamName(seg.stream)}`));
        shown.append(item);
      });
      if (segs.length > 5) shown.append(deleteDialogElement('li', 'trash-set-more', renderTranscriptCopy(copy.more, { n: segs.length - 5 })));

      const holdsLoc = segs.some((seg) => isOpenSegment(seg) && segmentData?.holds_location);
      const parts = segmentParts(segs, holdsLoc);
      const list = deleteDialogElement('ul', 'tr-delete-segment-list');
      parts.forEach((part) => list.append(deleteDialogElement('li', null, TRASH_COPY.parts[part])));

      deleteSegmentModalBody.replaceChildren(
        meta,
        shown,
        deleteDialogElement('p', 'tr-delete-reach', erasing ? copy.eraseReach : copy.moveReach),
        ...importLines(segs, step),
        ...(erasing && segs.some(seg => originalsInTrash.has(`${day}/${seg.stream}/${seg.key}`))
          ? [deleteDialogElement('p', 'tr-delete-originals', TRASH_COPY.originalsGo.many)]
          : []),
        deleteDialogElement('p', 'tr-delete-lead', erasing ? copy.eraseLead : copy.moveLead),
        list,
        deleteDialogElement('p', 'tr-delete-stays', erasing ? copy.eraseStays : copy.moveStays),
        deleteDialogElement('p', 'tr-delete-found', erasing ? copy.eraseFound : copy.moveFound),
      );
      const sizes = segs.map(knownSegmentBytes);
      const total = sizes.every((bytes) => bytes !== null) ? sizes.reduce((sum, bytes) => sum + bytes, 0) : 0;
      if (!erasing && total > 0) {
        deleteSegmentModalBody.append(deleteDialogElement('p', 'tr-delete-segment-size trash-dlg-size', renderTranscriptCopy(copy.size, { size: formatSize(total) })));
      }
    }

    function setTrashReceipt(copy, fill = {}, buttons = [], options = {}) {
      const notice = {
        app: 'transcripts',
        icon: options.icon || (options.settled ? 'trash-2' : 'triangle-alert'),
        title: renderTranscriptCopy(copy.title, fill),
        dismissible: true,
        buttons,
      };
      const message = options.message !== undefined ? options.message : copy.message && renderTranscriptCopy(copy.message, fill);
      if (message) notice.message = message;
      if (options.settled) notice.autoDismiss = 10000;
      window.AppServices.notifications.show(notice);
    }

    // One chosen uses the single delete; several, the set's. All done leaves
    // select mode; what was refused stays chosen, with its note.
    async function performPickDelete(segs, erase) {
      if (segs.length === 1) {
        const outcome = await performDeleteSegment(segs[0], erase);
        if (outcome?.kind === 'refused') {
          segPick.notes.set(segmentChoiceId(segs[0]), segPickNoteKey(outcome.key, erase));
          buildZoomSegments();
        } else if (outcome && outcome.kind !== 'noAnswer') {
          exitSegPick();
        }
        return;
      }
      await performDeleteSegmentSet(segs, erase);
    }

    async function performDeleteSegmentSet(segs, erase) {
      const segDay = day;
      const R = TRASH_COPY.setReceipt;
      const n = segs.length;
      let answer = null;
      try {
        const response = await fetch('/app/transcripts/api/segments/delete', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ segments: segs.map((seg) => ({ day: segDay, stream: seg.stream, key: seg.key })), ...(erase ? { erase: true, backups: backupsChosen() } : {}) }),
        });
        answer = response.ok ? await response.json().catch(() => null) : null;
      } catch (_) {
        answer = null;
      }
      if (!answer || !Array.isArray(answer.done) || !Array.isArray(answer.failed)) {
        setTrashReceipt(erase ? R.noAnswerErase : R.noAnswerMove);
        return;
      }
      const find = (ref) => segs.find((seg) => ref && seg.stream === ref.stream && seg.key === ref.key);
      const done = answer.done.map((entry) => ({ seg: find(entry.segment), entry })).filter((item) => item.seg);
      const failed = answer.failed.map((entry) => ({ seg: find(entry.segment), key: entry.key })).filter((item) => item.seg);

      // What moved leaves the timeline, the detail column and the list at once.
      const gone = new Set(done.map(({ seg }) => segmentChoiceId(seg)));
      allSegments = allSegments.filter((seg) => !gone.has(segmentChoiceId(seg)));
      if (done.some(({ seg }) => isOpenSegment(seg))) clearSegmentSelection();
      else if (done.some(({ seg }) => seg.item)) refreshUnselectedLocation(null, true);
      if (failed.length) {
        segPick.chosen = new Map(failed.map(({ seg }) => [segmentChoiceId(seg), seg]));
        segPick.notes = new Map(failed.map(({ seg, key }) => [segmentChoiceId(seg), segPickNoteKey(key, erase)]));
        buildZoomSegments();
      } else {
        exitSegPick();
      }
      refreshSegmentIndicators();

      const k = failed.length;
      const openTrash = openTrashButton();
      if (erase) {
        const erased = done.filter(({ entry }) => entry.erased === true).length;
        if (erased === n) {
          const clause = backupsClause(n);
          setTrashReceipt(R.erased, { n }, [], clause ? { settled: true, message: clause } : { settled: true });
        } else if (erased) {
          setTrashReceipt(R.partErased, { done: erased, n }, [openTrash], { message: R.partErasedMessage(n - erased) });
        } else {
          setTrashReceipt(R.noneErased, {}, [openTrash], { message: R.noneErasedMessage(n) });
        }
        return;
      }
      const moved = done.filter(({ entry }) => entry.trash?.id);
      const recoverCopy = {
        receipt: TRASH_COPY.receipt,
        buttons: TRASH_COPY.buttons,
        setReceipt: TRASH_COPY.setReceipt,
      };
      const recover = { label: TRASH_COPY.buttons.recover, onClick: setRecoverClick(moved, segDay, recoverCopy) };
      if (!k) {
        const earliest = moved.map(({ entry }) => entry.trash.erase_at).filter(Boolean).sort((a, b) => new Date(a) - new Date(b))[0];
        const date = trashEraseDate(earliest);
        setTrashReceipt(R.moved, { n, date }, [recover, openTrash], { settled: true, message: date ? undefined : '' });
      } else if (done.length) {
        setTrashReceipt(R.partMoved, { done: done.length, n }, [recover, openTrash], { message: R.still(k) });
      } else {
        setTrashReceipt(R.noneMoved, {}, [], { message: R.noneStill(k) });
      }
    }

    // Recover on a set's card brings back only what moved, each to its place.
    async function recoverSetFromReceipt(moved, segDay) {
      const R = TRASH_COPY.setReceipt;
      const ids = moved.map(({ entry }) => entry.trash.id);
      const one = ids.length === 1;
      let response;
      try {
        response = await fetch('/app/settings/api/trash/recover', {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ ids, page: 'transcripts' }),
        });
      } catch (_) {
        if (one) trashReceipt('noAnswerRecover');
        else setTrashReceipt(R.noAnswerRecover);
        return;
      }
      const data = response.ok ? await response.json().catch(() => null) : null;
      if (response.ok && !data) {
        if (one) trashReceipt('noAnswerRecover');
        else setTrashReceipt(R.noAnswerRecover);
        return;
      }
      const back = moved.filter(({ entry }) => Array.isArray(data?.done) && data.done.includes(entry.trash.id));
      back.forEach(({ seg }) => restoreDeletedSegment(seg, segDay));
      if (back.length && back.length === ids.length) {
        if (one) trashReceipt('recovered');
        else setTrashReceipt(R.recovered, { n: ids.length }, [], { settled: true, icon: 'undo-2' });
      } else if (back.length) {
        const others = ids.length - back.length;
        setTrashReceipt(R.partRecovered, { done: back.length, n: ids.length }, [openTrashButton()], { message: R.partRecoveredMessage(others) });
      } else if (one) {
        trashReceipt('recoverFailed', {}, [openTrashButton()]);
      } else {
        setTrashReceipt(R.recoverFailed, {}, [openTrashButton()]);
      }
    }

    selectBtn.addEventListener('click', enterSegPick);
    pickDoneIdle.addEventListener('click', exitSegPick);
    pickDoneSome.addEventListener('click', exitSegPick);
    pickAll.addEventListener('change', () => {
      filterSegmentsInRange().forEach((seg) => segPick.chosen.set(segmentChoiceId(seg), seg));
      buildZoomSegments();
      // The strip just swapped away the control that had focus.
      pickDelete.focus();
    });
    pickDelete.addEventListener('click', () => {
      const segs = chosenSegments();
      if (segs.length) showDeleteSegmentModal(segs);
    });

    // Delete segment handler
    deleteBtn.addEventListener('click', () => {
      showDeleteSegmentModal();
    });

    noticeHooks = {
      recoverSegment: recoverFromReceipt,
      recoverSet: recoverSetFromReceipt,
      shown: () => day,
    };
    transcriptHooks = noticeHooks;

    return {
      update(params) {
        reportTranscriptsReady = ctx.ready;
        changeDay(params && params.day);
      }
    };
  }
})();
