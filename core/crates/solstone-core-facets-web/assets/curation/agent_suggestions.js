// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

// Corrections a connected agent suggested, in the suggestions page. Every
// part of a card comes from the journal except the agent's reason, which is
// shown as its own words under a line naming the agent. Nothing an agent
// suggests changes the journal until the owner accepts it here.
(function (global) {
  'use strict';

  // --- owner-facing strings ---
  const S = {
    group: 'from your agents',
    earlier: 'earlier from your agents',
    onlyFrom: 'only from {agent}.',
    noneFrom: 'nothing from {agent} is waiting.',
    showAll: 'show every agent',
    loadFailed: "couldn't load what your agents suggested.",
    retry: 'try again',
    kind: {
      who_said_it: 'who said it',
      what_happened: 'what actually happened',
      recording: 'this was a recording or video playing',
      name: 'a name',
      same_person: 'the same person'
    },
    now: 'your journal says',
    nowPeople: 'your journal has',
    personIn: 'in {facets}',
    notesOne: '1 note',
    notesMany: '{n} notes',
    samePersonChange: 'merge them into one person',
    keepName: 'name to use',
    suggested: 'suggested',
    why: 'why',
    then: 'if you accept',
    basis: {
      owner_said: '{agent} says you told it',
      my_own_work: '{agent} says it knows this firsthand',
      journal_shows: '{agent} says it noticed this in your journal',
      outside_source: '{agent} says it found this outside your journal'
    },
    accountAsWritten: 'your account, as {agent} wrote it',
    linesOne: 'this line was said by {speaker}',
    linesMany: 'these {n} lines were said by {speaker}',
    speakerOwner: 'you',
    speakerSomeoneElse: "someone you don't know",
    recordingChange: 'this was a recording or video playing',
    rename: '{from} → {to}',
    alsoKnownAs: '{from}, also known as {to}',
    thenActivity: 'your journal thinks about this activity again, and about {days}.',
    thenActivities: 'your journal thinks about these {n} activities again, and about {days}.',
    thenAccount: 'your journal thinks about this activity again with your account, and about {days}.',
    thenRecording: 'your journal will think this through again as something you watched or listened to, with no one else taking part.',
    thenRename: 'your journal calls this person {to} from now on. {from} stays in the version history, where you can restore it.',
    thenAlsoKnownAs: 'your journal also calls this person {to}. the earlier version stays in the version history, where you can restore it.',
    thenSamePerson: "your journal keeps one person called {kept}, also called {other}, and moves everything attached to {other} onto them. a merge can't be undone.",
    accept: 'accept',
    edit: 'edit',
    dismiss: 'dismiss',
    cancel: 'cancel',
    undo: 'undo',
    ok: 'ok',
    openActivity: 'open the activity',
    openLines: 'open the lines',
    openNamed: 'open {name}',
    editAccountLabel: 'your account of it',
    editAccountHint: 'what you accept is kept as your own account.',
    editNameLabel: 'name',
    updating: 'updating…',
    accepted: "accepted {agent}'s suggestion. your journal will think about this again.",
    acceptedNotAsked: "accepted {agent}'s suggestion and saved it, but your journal couldn't confirm that it will think about it again.",
    acceptedPartial: "accepted {agent}'s suggestion for this activity. some other activities couldn't be updated.",
    acceptedRename: "accepted {agent}'s suggestion. {from} is now {to}.",
    acceptedAlsoKnownAs: "accepted {agent}'s suggestion. {from} is also called {to} now.",
    acceptedSamePerson: "accepted {agent}'s suggestion. {other} and {kept} are one person now, called {kept}.",
    undone: 'undone. your journal will think about it again without that change.',
    acceptedPlain: "accepted {agent}'s suggestion.",
    undonePlain: 'undone.',
    earlierAgent: 'an earlier agent',
    undoneNotAsked: "undone, but your journal couldn't confirm that it will think about it again.",
    undoneRename: 'undone. {to} is {from} again.',
    undoneAlsoKnownAs: 'undone. {to} is no longer another name for {from}.',
    undoRefused: "couldn't undo that, because you've changed it since. your change stays.",
    dismissed: 'dismissed. if {agent} suggests this again, {agent} is told you already said no.',
    stale: "this changed after {agent} suggested it, so there's nothing to accept. here's how it stands now:",
    staleChip: 'already changed',
    staleSamePerson: {
      journal_candidate: 'your journal is already asking whether {a} and {b} are the same person. you can answer it in possible duplicates.',
      owner_answered: "you already kept {a} and {b} separate, so there's nothing to accept.",
      journal_question: "your journal has its own question about {a} and {b}, so there's nothing to accept here.",
      changed: "{a} or {b} changed after {agent} suggested this, so there's nothing to accept.",
      busy: "this can't be accepted right now."
    },
    openDuplicates: 'open possible duplicates',
    earlierInDuplicates: 'in possible duplicates',
    earlierKeptSeparate: 'you kept them separate',
    earlierNotAccepted: 'not accepted',
    acceptFailed: "couldn't accept that, so nothing changed. try again in a moment.",
    actionFailed: "that didn't go through. try again.",
    acceptUnconfirmed: "your journal didn't answer, so this may already be accepted. trying again won't accept it twice.",
    actionUnconfirmed: "your journal didn't answer, so this may already be done. trying again won't do it twice.",
    showMore: 'show more',
    earlierAccepted: 'you accepted it',
    earlierPartial: 'saved for some activities',
    earlierDismissed: 'you dismissed it',
    earlierUndone: 'you accepted it, then undid it',
    earlierStale: 'already changed',
    open: 'open'
  };
  const SEP = ' · ';
  // --- end owner-facing strings ---

  const API = '/app/agents/api/suggestions';
  const PENDING_LIMIT = 100;
  const EARLIER_LIMIT = 20;
  const LOAD_TIMEOUT_MS = 60000;

  const escapeHtml = global.AppServices.escapeHtml;

  // The replacement is a function so a value carrying $& or $' is literal.
  function fmt(template, values) {
    let out = String(template || '');
    Object.entries(values || {}).forEach(([key, value]) => {
      out = out.replace(new RegExp(`\\{${key}\\}`, 'g'), () => String(value ?? ''));
    });
    return out;
  }

  // Each value is escaped before it enters the template; the template itself
  // is ours. Used where a value is wrapped in markup (the agent's name, bold).
  function fmtHtml(template, values) {
    let out = escapeHtml(String(template || ''));
    Object.entries(values || {}).forEach(([key, value]) => {
      out = out.replace(new RegExp(`\\{${key}\\}`, 'g'), () => String(value ?? ''));
    });
    return out;
  }

  function agentFilter() {
    try {
      return new URLSearchParams(global.location.search).get('agent') || '';
    } catch (err) {
      return '';
    }
  }

  function keyOf(agent) {
    return String(agent?.source || '').slice(0, 4);
  }

  function agentName(agent) {
    return String(agent?.label || '').trim() || S.earlierAgent;
  }

  function agentHtml(agent) {
    const key = keyOf(agent);
    return `<span class="sg-agent">${escapeHtml(agentName(agent))}${key ? `<span class="sg-key">${escapeHtml(SEP + key)}</span>` : ''}</span>`;
  }

  function agentText(agent) {
    const key = keyOf(agent);
    return key ? `${agentName(agent)}${SEP}${key}` : agentName(agent);
  }

  function day(dayKey) {
    return global.JournalFormat?.day ? global.JournalFormat.day(String(dayKey)) : String(dayKey);
  }

  // Inside a sentence the relative days read in lowercase ("about yesterday");
  // weekday and month names keep their capitals.
  function dayInSentence(dayKey) {
    const name = day(dayKey);
    return /^(Today|Yesterday|Tomorrow)$/.test(name) ? name.toLowerCase() : name;
  }

  function dayList(days) {
    const names = (Array.isArray(days) ? days : []).filter(Boolean).map(dayInSentence);
    if (names.length <= 1) return names.join('');
    if (names.length === 2) return `${names[0]} and ${names[1]}`;
    return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
  }

  function clock(value) {
    const m = /^(\d{2}):(\d{2})/.exec(String(value || ''));
    return m ? `${m[1]}:${m[2]}` : '';
  }

  function when(instant) {
    return global.JournalClock?.formatInstant
      ? global.JournalClock.formatInstant(instant)
      : new Date(instant).toLocaleString();
  }

  function placeText(card) {
    const place = card.place || {};
    if (place.kind === 'entity') {
      return [place.name, place.type].filter(Boolean).join(SEP);
    }
    const start = clock(place.start);
    const end = clock(place.end);
    const span = start && end ? `${start}–${end}` : start;
    const when = [place.day ? day(place.day) : '', span].filter(Boolean).join(', ');
    return [place.title, when].filter(Boolean).join(SEP);
  }

  function speakerText(speaker) {
    if (speaker?.kind === 'owner') return S.speakerOwner;
    if (speaker?.kind === 'someone_else') return S.speakerSomeoneElse;
    return String(speaker?.label || '');
  }

  // --- the same person ---------------------------------------------------------
  // Both people come from the journal (`change_parts.people`, in the order the
  // agent named them); the agent supplies only which two, and why. The owner
  // picks which one the merged person stays, and the other name is kept as
  // another name for them. A merge has no undo, so none is offered.
  let facetTitles = {};
  const keepChoice = new Map();

  function setFacetTitles(titles) {
    facetTitles = titles && typeof titles === 'object' ? titles : {};
  }

  function facetName(slug) {
    return Object.prototype.hasOwnProperty.call(facetTitles, slug) ? facetTitles[slug] : String(slug || '');
  }

  function namesList(names) {
    if (names.length <= 1) return names.join('');
    if (names.length === 2) return `${names[0]} and ${names[1]}`;
    return `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
  }

  function people(card) {
    const list = Array.isArray(card?.change_parts?.people) ? card.change_parts.people : [];
    return list.filter((person) => person && person.id);
  }

  // While it waits: the owner's pick on this page, then the journal's default.
  // Once acted on, only what the journal recorded: an accepted card names the
  // person it merged into, whatever another tab had selected.
  function keptId(card) {
    const list = people(card);
    const chosen = card.status === 'pending' ? keepChoice.get(String(card.id)) : '';
    const ids = [chosen, card.value?.keep_id, card.change_parts?.keep_id];
    return ids.find((id) => id && list.some((person) => person.id === id)) || list[0]?.id || '';
  }

  function pairOf(card) {
    const id = keptId(card);
    const list = people(card);
    return {
      kept: list.find((person) => person.id === id) || {},
      other: list.find((person) => person.id !== id) || {}
    };
  }

  // What each person is attached to, as the journal counts it: the facets
  // they appear in and the notes held under them. Nothing reported, nothing said.
  function personMeta(person) {
    const attachments = Array.isArray(person.attachments) ? person.attachments : [];
    const slugs = attachments.length
      ? attachments.map((item) => item?.facet).filter(Boolean)
      : (Array.isArray(person.facets) ? person.facets : []);
    const facets = [...new Set(slugs)].map(facetName).filter(Boolean);
    const notes = attachments.reduce((sum, item) => sum + (Number(item?.notes) || 0), 0);
    const parts = [];
    if (facets.length) parts.push(fmt(S.personIn, {facets: namesList(facets)}));
    if (notes) parts.push(notes === 1 ? S.notesOne : fmt(S.notesMany, {n: notes}));
    return parts.join(SEP);
  }

  function peopleHtml(card) {
    return `<ul class="sg-people">${people(card).map((person) => {
      const meta = personMeta(person);
      return `<li><strong>${escapeHtml(person.name || '')}</strong>${meta ? `<span class="sg-person-meta">${escapeHtml(meta)}</span>` : ''}</li>`;
    }).join('')}</ul>`;
  }

  function keepRow(card) {
    const list = people(card);
    if (list.length < 2) return '';
    const id = `sg-keep-${String(card.id).replace(/[^A-Za-z0-9_-]/g, '')}`;
    const kept = keptId(card);
    const options = list.map((person) => `<label class="sg-keep-option"><input type="radio" name="${id}" value="${escapeHtml(person.id)}" data-agent-keep${person.id === kept ? ' checked' : ''}><span>${escapeHtml(person.name || '')}</span></label>`).join('');
    return `<dt id="${id}">${escapeHtml(S.keepName)}</dt><dd><div class="sg-keep" role="radiogroup" aria-labelledby="${id}">${options}</div></dd>`;
  }

  function nowHtml(card) {
    const lines = Array.isArray(card.now_lines) ? card.now_lines : [];
    if (card.kind === 'who_said_it' && lines.length) {
      return `<ul class="sg-lines">${lines.map((line) => `<li><time>${escapeHtml(clock(line.time))}</time><span class="who">${escapeHtml(line.speaker || '')}</span><span class="said">${escapeHtml(line.text || '')}</span></li>`).join('')}</ul>`;
    }
    return escapeHtml(card.now || '');
  }

  // A recording's current state is a flag, not words: on a waiting card it is
  // always off, which the place line and the suggestion already say. When it is
  // on (the owner marked it first), the journal's words for that are shown.
  function nowRow(card) {
    if (card.kind === 'same_person') {
      return people(card).length ? `<dt>${escapeHtml(S.nowPeople)}</dt><dd>${peopleHtml(card)}</dd>` : '';
    }
    if (card.kind === 'recording') {
      const on = /^(on|true|yes)$/i.test(String(card.now || '').trim());
      return on ? `<dt>${escapeHtml(S.now)}</dt><dd>${escapeHtml(S.recordingChange)}</dd>` : '';
    }
    return `<dt>${escapeHtml(S.now)}</dt><dd>${nowHtml(card)}</dd>`;
  }

  function changeHtml(card) {
    const parts = card.change_parts || {};
    if (card.kind === 'same_person') return escapeHtml(S.samePersonChange);
    if (card.kind === 'who_said_it') {
      const n = Number(parts.line_count) || 0;
      const speaker = `<strong>${escapeHtml(speakerText(parts.speaker))}</strong>`;
      return n === 1 ? fmtHtml(S.linesOne, {speaker}) : fmtHtml(S.linesMany, {n: escapeHtml(n), speaker});
    }
    if (card.kind === 'what_happened') {
      return `<span class="sg-whoby">${escapeHtml(fmt(S.accountAsWritten, {agent: agentName(card.agent)}))}</span><span class="sg-account">${escapeHtml(parts.text || '')}</span>`;
    }
    if (card.kind === 'name') {
      const template = parts.mode === 'also_known_as' ? S.alsoKnownAs : S.rename;
      return fmtHtml(template, {from: escapeHtml(parts.from || ''), to: `<strong>${escapeHtml(parts.to || '')}</strong>`});
    }
    return escapeHtml(S.recordingChange);
  }

  // Only what the journal reported it will do; a kind with nothing reported
  // gets no line rather than a guess.
  function thenText(card) {
    const parts = card.change_parts || {};
    if (card.kind === 'same_person') {
      const {kept, other} = pairOf(card);
      return kept.name && other.name ? fmt(S.thenSamePerson, {kept: kept.name, other: other.name}) : '';
    }
    if (card.kind === 'name') {
      return fmt(parts.mode === 'also_known_as' ? S.thenAlsoKnownAs : S.thenRename, {from: parts.from || '', to: parts.to || ''});
    }
    const impact = card.impact || {};
    const activities = Array.isArray(impact.activities) ? impact.activities : [];
    const days = dayList(impact.days);
    if (!activities.length || !days) return '';
    if (card.kind === 'recording') return S.thenRecording;
    if (card.kind === 'what_happened') return fmt(S.thenAccount, {days});
    return activities.length === 1
      ? fmt(S.thenActivity, {days})
      : fmt(S.thenActivities, {n: activities.length, days});
  }

  function openLabel(card) {
    if (card.kind === 'same_person') return fmt(S.openNamed, {name: pairOf(card).kept.name || ''});
    if (card.kind === 'name') return fmt(S.openNamed, {name: card.place?.name || card.change_parts?.from || ''});
    if (card.kind === 'who_said_it') return S.openLines;
    return S.openActivity;
  }

  function openUrl(card) {
    if (card.kind === 'same_person') return pairOf(card).kept.open_url || card.open_url || '';
    return card.open_url || '';
  }

  function openHtml(card) {
    const url = openUrl(card);
    return url
      ? `<a class="sg-open" href="${escapeHtml(url)}">${escapeHtml(openLabel(card))}</a>`
      : '';
  }

  function headHtml(card, extra = '') {
    return `<p class="sg-head">${agentHtml(card.agent)}<span class="sg-when">${escapeHtml(when(card.created_at))}</span></p>
      <p class="sg-kind">${escapeHtml(S.kind[card.kind] || '')}${extra}</p>
      ${placeText(card) ? `<p class="sg-place">${escapeHtml(placeText(card))}</p>` : ''}`;
  }

  function attrs(card) {
    return `data-agent-suggestion data-suggestion-id="${escapeHtml(card.id)}" data-kind="${escapeHtml(card.kind)}"`;
  }

  function pendingCardHtml(card) {
    const then = thenText(card);
    const basis = S.basis[card.basis];
    return `<article class="sg-card" ${attrs(card)}>
      ${headHtml(card)}
      <dl class="sg-grid">
        ${nowRow(card)}
        <dt>${escapeHtml(S.suggested)}</dt><dd class="sg-change">${changeHtml(card)}</dd>
        ${card.kind === 'same_person' ? keepRow(card) : ''}
        ${card.reason ? `<dt>${escapeHtml(S.why)}</dt><dd>${basis ? `<span class="sg-whoby">${escapeHtml(fmt(basis, {agent: agentName(card.agent)}))}</span>` : ''}<p class="sg-quote">"${escapeHtml(card.reason)}"</p></dd>` : ''}
        ${then ? `<dt>${escapeHtml(S.then)}</dt><dd class="sg-then">${escapeHtml(then)}</dd>` : ''}
      </dl>
      <div class="curation-actions">
        <button class="curation-button curation-button-primary" type="button" data-action="agent-accept">${escapeHtml(S.accept)}</button>
        ${card.editable && card.kind !== 'same_person' ? `<button class="curation-button" type="button" data-action="agent-edit">${escapeHtml(S.edit)}</button>` : ''}
        <button class="curation-button" type="button" data-action="agent-dismiss">${escapeHtml(S.dismiss)}</button>
        ${openHtml(card)}
      </div>
      <p class="curation-item-error" role="alert" hidden></p>
    </article>`;
  }

  // A same-person card that can't be accepted says where the pair is answered
  // instead; its people are as they were when the agent suggested it, so they
  // are not shown as how things stand now.
  const DUPLICATES_URL = '/app/entities/?folder=duplicates';

  function staleSamePersonHtml(card) {
    const names = people(card).map((person) => person.name || '');
    const reason = S.staleSamePerson[card.stale_reason] ? card.stale_reason : 'changed';
    const open = reason === 'journal_candidate'
      ? `<a class="sg-open" href="${escapeHtml(DUPLICATES_URL)}">${escapeHtml(S.openDuplicates)}</a>`
      : openHtml(card);
    const chip = reason === 'changed' ? `<span class="sg-chip">${escapeHtml(S.staleChip)}</span>` : '';
    return `<article class="sg-card sg-stale" ${attrs(card)}>
      ${headHtml(card, chip)}
      <p class="sg-stale-lead">${escapeHtml(fmt(S.staleSamePerson[reason], {a: names[0] || '', b: names[1] || '', agent: agentName(card.agent)}))}</p>
      <div class="curation-actions">
        <button class="curation-button" type="button" data-action="agent-stale-ok">${escapeHtml(S.ok)}</button>
        ${open}
      </div>
    </article>`;
  }

  function staleCardHtml(card) {
    if (card.kind === 'same_person') return staleSamePersonHtml(card);
    return `<article class="sg-card sg-stale" ${attrs(card)}>
      ${headHtml(card, `<span class="sg-chip">${escapeHtml(S.staleChip)}</span>`)}
      <p class="sg-stale-lead">${escapeHtml(fmt(S.stale, {agent: agentName(card.agent)}))}</p>
      ${nowRow(card) ? `<dl class="sg-grid">${nowRow(card)}</dl>` : ''}
      <div class="curation-actions">
        <button class="curation-button" type="button" data-action="agent-stale-ok">${escapeHtml(S.ok)}</button>
        ${openHtml(card)}
      </div>
    </article>`;
  }

  function editHtml(card) {
    const parts = card.change_parts || {};
    const id = `sg-edit-${String(card.id).replace(/[^A-Za-z0-9_-]/g, '')}`;
    const field = card.kind === 'name'
      ? `<label for="${id}">${escapeHtml(S.editNameLabel)}</label><input id="${id}" data-agent-edit-value value="${escapeHtml(parts.to || '')}" maxlength="200">`
      : `<label for="${id}">${escapeHtml(S.editAccountLabel)}</label><textarea id="${id}" data-agent-edit-value maxlength="600">${escapeHtml(parts.text || '')}</textarea><p class="sg-hint">${escapeHtml(S.editAccountHint)}</p>`;
    return `<article class="sg-card sg-edit" ${attrs(card)}>
      ${headHtml(card)}
      <form data-agent-edit-form>
        ${field}
        <div class="curation-actions">
          <button class="curation-button curation-button-primary" type="submit">${escapeHtml(S.accept)}</button>
          <button class="curation-button" type="button" data-action="agent-edit-cancel">${escapeHtml(S.cancel)}</button>
        </div>
      </form>
      <p class="curation-item-error" role="alert" hidden></p>
    </article>`;
  }

  function receiptText(card, phase) {
    const agent = agentName(card.agent);
    const parts = card.change_parts || {};
    const aka = card.kind === 'name' && parts.mode === 'also_known_as';
    const failedAsk = card.asked === false && Boolean(card.ask_reason);
    if (phase === 'dismissed') return fmt(S.dismissed, {agent});
    if (phase === 'undone') {
      if (card.kind === 'name') return fmt(aka ? S.undoneAlsoKnownAs : S.undoneRename, {from: parts.from || '', to: parts.to || ''});
      if (failedAsk) return S.undoneNotAsked;
      return card.asked === true ? S.undone : S.undonePlain;
    }
    if (card.incomplete) return fmt(S.acceptedPartial, {agent});
    if (card.kind === 'name') return fmt(aka ? S.acceptedAlsoKnownAs : S.acceptedRename, {agent, from: parts.from || '', to: parts.to || ''});
    if (card.kind === 'same_person') {
      const {kept, other} = pairOf(card);
      return kept.name && other.name ? fmt(S.acceptedSamePerson, {agent, kept: kept.name, other: other.name}) : fmt(S.acceptedPlain, {agent});
    }
    // Thinking again is claimed only when the journal reports it asked.
    if (failedAsk) return fmt(S.acceptedNotAsked, {agent});
    return fmt(card.asked === true ? S.accepted : S.acceptedPlain, {agent});
  }

  function receiptHtml(card, phase, note = '') {
    const warn = (card.asked === false && card.ask_reason && card.kind !== 'name' && card.kind !== 'same_person') || note;
    const undo = phase === 'accepted' && card.can_undo
      ? `<button class="curation-button" type="button" data-action="agent-undo">${escapeHtml(S.undo)}</button>`
      : '';
    return `<div class="sg-receipt${warn ? ' sg-stale' : ''}" ${attrs(card)} role="status">
      <p>${escapeHtml(note || receiptText(card, phase))}</p>
      ${undo}
      ${phase === 'dismissed' ? '' : openHtml(card).replace('class="sg-open"', 'class="sg-open sg-open-inline"')}
    </div>`;
  }

  function earlierRowHtml(card) {
    const pairStale = card.kind === 'same_person' && card.status === 'stale'
      ? {journal_candidate: S.earlierInDuplicates, owner_answered: S.earlierKeptSeparate,
        journal_question: S.earlierNotAccepted, busy: S.earlierNotAccepted}[card.stale_reason]
      : '';
    const status = pairStale || (card.incomplete && card.status === 'accepted' ? S.earlierPartial : {
      accepted: S.earlierAccepted,
      dismissed: S.earlierDismissed,
      undone: S.earlierUndone,
      stale: S.earlierStale
    }[card.status] || '');
    const subject = card.kind === 'same_person'
      ? namesList(people(card).map((person) => person.name).filter(Boolean))
      : (card.place?.kind === 'entity' ? (card.place?.name || '') : (card.place?.title || ''));
    const meta = [agentText(card.agent), status, when(card.updated_at || card.created_at)].filter(Boolean).join(SEP);
    const undo = card.status === 'accepted' && card.can_undo
      ? `<button class="curation-button" type="button" data-action="agent-undo">${escapeHtml(S.undo)}</button>` : '';
    const actions = card.status === 'accepted'
      ? `<div class="sg-earlier-actions" ${attrs(card)}>${undo}${openHtml(card)}</div>`
      : (pairStale === S.earlierInDuplicates
        ? `<a href="${escapeHtml(DUPLICATES_URL)}">${escapeHtml(S.open)}</a>`
        : (openUrl(card) ? `<a href="${escapeHtml(openUrl(card))}">${escapeHtml(S.open)}</a>` : ''));
    return `<li><span class="what"><b>${escapeHtml(S.kind[card.kind] || '')}</b>${subject ? escapeHtml(SEP + subject) : ''}</span><span class="meta">${escapeHtml(meta)}</span>${actions}</li>`;

  }

  // --- state -----------------------------------------------------------------
  const state = {pending: [], earlier: [], agents: [], failed: false, loaded: false, pendingNext: null, earlierNext: null, pendingTotal: null};
  const byId = new Map();

  function requestOptions() {
    return typeof AbortSignal !== 'undefined' && AbortSignal.timeout
      ? {signal: AbortSignal.timeout(LOAD_TIMEOUT_MS)}
      : undefined;
  }

  const PENDING_STATUS = 'pending';
  const EARLIER_STATUS = 'accepted,dismissed,undone,stale';

  function query(status, limit, before) {
    const params = new URLSearchParams({status, limit: String(limit)});
    if (before) params.set('before', before);
    const agent = agentFilter();
    if (agent) params.set('agent', agent);
    return `${API}?${params}`;
  }

  async function load() {
    try {
      const [pending, earlier] = await Promise.all([
        global.apiJson(query(PENDING_STATUS, PENDING_LIMIT), requestOptions()),
        global.apiJson(query(EARLIER_STATUS, EARLIER_LIMIT), requestOptions())
      ]);
      state.pending = Array.isArray(pending?.items) ? pending.items : [];
      state.earlier = Array.isArray(earlier?.items) ? earlier.items : [];
      state.agents = Array.isArray(pending?.agents) ? pending.agents : [];
      state.pendingNext = pending?.next_cursor || null;
      state.earlierNext = earlier?.next_cursor || null;
      state.pendingTotal = totalWaiting();
      state.failed = false;
    } catch (err) {
      global.logError?.(err, {context: 'curation-agent-suggestions'});
      state.pending = [];
      state.earlier = [];
      state.failed = true;
    }
    state.loaded = true;
    byId.clear();
    state.pending.concat(state.earlier).forEach((card) => byId.set(String(card.id), card));
  }

  // The group's count is every waiting suggestion, not just the page shown:
  // the journal reports each agent's waiting count beside the list.
  function totalWaiting() {
    const source = agentFilter();
    const counts = state.agents
      .filter((agent) => !source || agent.source === source)
      .map((agent) => Number(agent.pending_count))
      .filter((n) => Number.isFinite(n) && n >= 0);
    return counts.length ? counts.reduce((sum, n) => sum + n, 0) : null;
  }

  function pendingCount() {
    if (state.failed) return null;
    const shown = state.pending.filter((card) => card.status === 'pending').length;
    return state.pendingTotal == null ? shown : Math.max(shown, state.pendingTotal);
  }

  function moreHtml(list) {
    return `<button class="curation-button curation-button-link" type="button" data-action="agent-more" data-list="${list}">${escapeHtml(S.showMore)}</button>`;
  }

  async function loadMore(button, onChange) {
    const list = button.dataset.list;
    const before = list === 'earlier' ? state.earlierNext : state.pendingNext;
    if (!before) return;
    button.disabled = true;
    let page;
    try {
      page = await global.apiJson(
        query(list === 'earlier' ? EARLIER_STATUS : PENDING_STATUS, list === 'earlier' ? EARLIER_LIMIT : PENDING_LIMIT, before),
        requestOptions()
      );
    } catch (err) {
      global.logError?.(err, {context: 'curation-agent-more'});
      button.disabled = false;
      return;
    }
    const items = (Array.isArray(page?.items) ? page.items : []).filter((card) => !byId.has(String(card.id)));
    items.forEach((card) => byId.set(String(card.id), card));
    if (list === 'earlier') {
      state.earlier = state.earlier.concat(items);
      state.earlierNext = page?.next_cursor || null;
      button.closest('.sg-earlier')?.querySelector('ul')?.insertAdjacentHTML('beforeend', items.map(earlierRowHtml).join(''));
      const summary = button.closest('.sg-earlier')?.querySelector('summary');
      if (summary) summary.textContent = `${S.earlier} (${state.earlier.length})`;
      if (!state.earlierNext) button.remove(); else button.disabled = false;
    } else {
      state.pending = state.pending.concat(items);
      state.pendingNext = page?.next_cursor || null;
      button.insertAdjacentHTML('beforebegin', items.map((card) => (card.status === 'stale' ? staleCardHtml(card) : pendingCardHtml(card))).join(''));
      if (!state.pendingNext) button.remove(); else button.disabled = false;
    }
    onChange();
  }

  function filteredAgent() {
    const source = agentFilter();
    if (!source) return null;
    return state.agents.find((agent) => agent.source === source)
      || state.pending.concat(state.earlier).map((card) => card.agent).find((agent) => agent?.source === source)
      || {source, label: ''};
  }

  function showAllHref() {
    return `${global.location.pathname || '/app/curation/'}${global.location.hash || ''}`;
  }

  // The group renders whenever there is anything to say: something waiting,
  // something earlier, a failed load, or an owner who arrived for one agent.
  function groupHtml(isOpen) {
    if (!state.loaded) return '';
    const filtered = filteredAgent();
    if (!state.failed && !state.pending.length && !state.earlier.length && !filtered) return '';
    const cards = state.pending.map((card) => (card.status === 'stale' ? staleCardHtml(card) : pendingCardHtml(card))).join('');
    const filterLine = filtered
      ? `<p class="sg-filter">${escapeHtml(fmt(state.failed || state.pending.length ? S.onlyFrom : S.noneFrom, {agent: agentText(filtered)}))} <a href="${escapeHtml(showAllHref())}" data-agent-show-all>${escapeHtml(S.showAll)}</a></p>`
      : '';
    const failed = state.failed
      ? `<p class="curation-item-error">${escapeHtml(S.loadFailed)}</p><div class="curation-actions"><button class="curation-button" type="button" data-action="agent-reload">${escapeHtml(S.retry)}</button></div>`
      : '';
    const earlier = state.earlier.length
      ? `<details class="sg-earlier"><summary>${escapeHtml(S.earlier)} (${state.earlier.length})</summary><ul>${state.earlier.map(earlierRowHtml).join('')}</ul>${state.earlierNext ? moreHtml('earlier') : ''}</details>`
      : '';
    return `<details class="curation-group" data-review-group="agents" ${isOpen ? 'open' : ''}><summary>${escapeHtml(S.group)} <span data-group-count data-agent-count>${state.failed ? '' : '(' + pendingCount() + ')'}</span></summary><div class="curation-group-items">${filterLine}${failed}${cards}${state.pendingNext ? moreHtml('pending') : ''}${earlier}</div></details>`;
  }

  // --- actions ---------------------------------------------------------------
  function postJson(path, body) {
    return global.apiJson(path, {
      method: 'POST',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify(body || {})
    });
  }

  function cardFor(node) {
    return node?.closest('[data-agent-suggestion]') || null;
  }

  function current(node) {
    return byId.get(String(node?.dataset?.suggestionId || '')) || null;
  }

  function replace(node, html) {
    const holder = document.createElement('div');
    holder.innerHTML = html.trim();
    const next = holder.firstElementChild;
    node.replaceWith(next);
    return next;
  }

  function remember(card) {
    const was = byId.get(String(card?.id));
    if (was?.status === 'pending' && card?.status !== 'pending' && state.pendingTotal != null) {
      state.pendingTotal = Math.max(0, state.pendingTotal - 1);
    }
    if (card?.id) byId.set(String(card.id), card);
    state.pending = state.pending.filter((item) => String(item.id) !== String(card?.id) || card.status === 'pending' || card.status === 'stale');
    return card;
  }

  function setBusy(node, busy) {
    node.querySelectorAll('button, input, textarea').forEach((control) => { control.disabled = busy; });
  }

  function showError(node, message) {
    const error = node.querySelector('.curation-item-error');
    if (!error) return;
    error.textContent = message || '';
    error.hidden = !message;
  }

  function updating(node) {
    return replace(node, `<div class="sg-receipt" ${node.getAttributeNames().filter((n) => n.startsWith('data-')).map((n) => `${n}="${escapeHtml(node.getAttribute(n))}"`).join(' ')} role="status"><p class="sg-updating">${escapeHtml(S.updating)}</p></div>`);
  }

  // No answer (a dropped connection, a timeout, an unreadable reply or a
  // server fault) is not a refusal: the change may already have been made.
  // A retry is safe, because the journal never applies one twice.
  function unanswered(err) {
    return !err?.status || err.cause === 'parse' || err.status >= 500;
  }

  async function accept(node, value, onChange) {
    const card = current(node);
    if (!card) return;
    const restore = node.outerHTML;
    const pending = updating(node);
    try {
      const result = await postJson(`${API}/${encodeURIComponent(card.id)}/accept`, value === undefined ? {} : {value});
      const accepted = remember(result?.suggestion || card);
      replace(pending, receiptHtml(accepted, 'accepted'));
    } catch (err) {
      const stale = err?.payload?.code === 'already_changed' && err.payload.suggestion;
      if (stale) {
        remember(err.payload.suggestion);
        replace(pending, staleCardHtml(err.payload.suggestion));
      } else {
        const back = replace(pending, restore);
        showError(back, unanswered(err) ? S.acceptUnconfirmed : S.acceptFailed);
      }
    }
    onChange();
  }

  async function act(node, verb, phase, onChange) {
    const card = current(node);
    if (!card) return;
    setBusy(node, true);
    try {
      const result = await postJson(`${API}/${encodeURIComponent(card.id)}/${verb}`);
      const next = remember(result?.suggestion || card);
      replace(node, receiptHtml(next, phase));
    } catch (err) {
      setBusy(node, false);
      const code = err?.payload?.code || err?.payload?.reason_code;
      if (verb === 'undo' && code === 'undo_refused') {
        // The change stays accepted; the owner's later work is never overwritten.
        const kept = remember(err.payload.suggestion || {...card, can_undo: false});
        replace(node, receiptHtml(kept, 'undone', S.undoRefused));
      } else {
        const message = unanswered(err)
          ? S.actionUnconfirmed
          : (global.SurfaceState?.serverMessageFrom?.(err) || S.actionFailed);
        let error = node.querySelector('.curation-item-error');
        if (!error) {
          error = document.createElement('p');
          error.className = 'curation-item-error';
          error.setAttribute('role', 'alert');
          node.appendChild(error);
        }
        error.textContent = message;
        error.hidden = false;
      }
    }
    onChange();
  }

  // Handles clicks and submits inside the agents group; returns true when the
  // event was one of ours.
  function handle(event, onChange, reload) {
    const form = event.target.closest?.('[data-agent-edit-form]');
    if (event.type === 'submit' && form) {
      event.preventDefault();
      const node = cardFor(form);
      const card = current(node);
      const input = form.querySelector('[data-agent-edit-value]');
      const text = String(input?.value || '').trim();
      if (!card || !text) return true;
      const value = card.kind === 'name'
        ? {text, mode: card.change_parts?.mode || 'replace'}
        : {text};
      accept(node, value, onChange);
      return true;
    }
    const keep = event.target.closest?.('[data-agent-keep]');
    if (event.type === 'change' && keep) {
      const node = cardFor(keep);
      const card = current(node);
      if (!card) return true;
      keepChoice.set(String(card.id), keep.value);
      const then = node.querySelector('.sg-then');
      if (then) then.textContent = thenText(card);
      const open = node.querySelector('.curation-actions .sg-open');
      if (open) open.outerHTML = openHtml(card);
      return true;
    }
    if (event.type !== 'click') return false;
    const button = event.target.closest?.('[data-action^="agent-"]');
    if (!button) return false;
    const node = cardFor(button);
    const card = current(node);
    switch (button.dataset.action) {
      case 'agent-reload':
        reload();
        return true;
      case 'agent-more':
        loadMore(button, onChange);
        return true;
      case 'agent-accept':
        accept(node, card?.kind === 'same_person' && keptId(card) ? {keep_id: keptId(card)} : undefined, onChange);
        return true;
      case 'agent-edit':
        if (card) replace(node, editHtml(card)).querySelector('[data-agent-edit-value]')?.focus();
        return true;
      case 'agent-edit-cancel':
        if (card) replace(node, pendingCardHtml(card));
        return true;
      case 'agent-dismiss':
        act(node, 'dismiss', 'dismissed', onChange);
        return true;
      case 'agent-undo':
        act(node, 'undo', 'undone', onChange);
        return true;
      case 'agent-stale-ok':
        if (node) node.remove();
        state.pending = state.pending.filter((item) => String(item.id) !== String(card?.id));
        onChange();
        return true;
      default:
        return false;
    }
  }

  global.CurationAgentSuggestions = {load, groupHtml, handle, pendingCount, setFacetTitles};
})(window);
