// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const ORIGIN_COPY = Object.freeze({
    withTalent: "sol noticed (from {trigger_talent}) at {time}",
    withoutTalent: "sol noticed at {time}",
    supersededSuffix: " (superseded by {time})",
    show: "details",
    hide: "hide details",
    triggerTalentLabel: "trigger talent",
    dedupeLabel: "dedupe",
    sinceTsLabel: "since"
  });
  const ORIGIN_SINCE_FORMATTER = new Intl.DateTimeFormat('en-US', {
    timeZone: 'UTC',
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    hour12: false,
    timeZoneName: 'short'
  });

  function renderEventItem(event, ctx) {
    if (![
      'owner_message',
      'sol_message',
      'talent_queued',
      'talent_spawned',
      'talent_finished',
      'talent_errored',
      'reflection_ready',
      'chat_error',
      'sol_chat_request',
      'sol_chat_request_superseded',
      'owner_chat_open',
      'owner_chat_dismissed'
    ].includes(event.kind)) {
      return null;
    }

    if ([
      'sol_chat_request',
      'sol_chat_request_superseded',
      'owner_chat_open',
      'owner_chat_dismissed'
    ].includes(event.kind)) {
      return null;
    }

    const item = document.createElement('li');
    item.id = ctx.id;
    item.className = 'chat-event';
    item.dataset.kind = event.kind;
    item.dataset.ts = String(event.ts);
    if (event.kind === 'sol_message' && event.notes) {
      item.dataset.notes = event.notes;
    }
    if (event.kind === 'sol_message') {
      item.dataset.useId = event.use_id;
    }
    if (event.kind === 'sol_message' && event.origin?.request_id) {
      item.dataset.requestId = event.origin.request_id;
    }
    event._thinkingId = ctx.id + '-thinking';
    event._errorDetailId = ctx.id + '-error-detail';
    item.appendChild(renderEventBody(event, ctx));
    return item;
  }

  function renderEventBody(event, ctx) {
    if (event.kind === 'owner_message') {
      return buildBubble(ctx.ownerName, event.text, 'owner', '');
    }
    if (event.kind === 'sol_message') {
      const bubble = buildBubble(ctx.agentName, event.text, 'sol', event.notes || '');
      const fragment = document.createDocumentFragment();
      if (event.origin) {
        fragment.appendChild(renderOriginTag(event.origin, ctx));
      }
      if (event.dispatchOrigin) {
        fragment.appendChild(renderDispatchOriginTag(event.dispatchOrigin, ctx));
      }
      fragment.appendChild(bubble);
      return withThinkingSurface(fragment, event, ctx);
    }
    if (event.kind === 'talent_queued') {
      return buildQueuedTalentCard(event);
    }
    if (event.kind === 'talent_spawned') {
      return buildTalentCard(window.solChatCopy.talentLabel(event.name, 'running'), event.task || '', event.use_id, 'active', 'chat-talent-card--spawned');
    }
    if (event.kind === 'talent_finished') {
      return withThinkingSurface(
        buildTalentCard(window.solChatCopy.talentLabel(event.name, 'finished'), event.summary || '', event.use_id, 'finished', 'chat-talent-card--finished'),
        event,
        ctx
      );
    }
    if (event.kind === 'talent_errored') {
      return buildTalentCard(window.solChatCopy.talentLabel(event.name, 'errored'), event.reason || '', event.use_id, 'errored', 'chat-talent-card--errored');
    }
    if (event.kind === 'reflection_ready') {
      return buildReflectionCard(event.day || '', event.url || '');
    }

    const renderedReason = window.renderChatReason
      ? window.renderChatReason(event.reason || 'unknown', event.provider || '')
      : {message: event.reason || 'unknown'};
    const block = document.createElement('div');
    block.className = 'chat-error-block';
    const message = document.createElement('div');
    message.textContent = renderedReason.message;
    block.appendChild(message);
    const detailText = String(event.detail || '');
    if (detailText) {
      const button = document.createElement('button');
      button.type = 'button';
      button.className = 'chat-error-detail-expander';
      button.dataset.errorDetailId = event._errorDetailId;
      button.setAttribute('aria-expanded', 'false');
      button.textContent = window.solChatCopy.CHAT_ERROR_DETAIL_EXPANDER_LABEL;
      const detail = document.createElement('div');
      detail.className = 'chat-error-detail-content';
      detail.id = event._errorDetailId;
      detail.hidden = true;
      const code = document.createElement('code');
      code.textContent = detailText;
      detail.appendChild(code);
      block.appendChild(button);
      block.appendChild(detail);
    }
    return block;
  }

  function withThinkingSurface(node, event, ctx) {
    const thinkingNode = buildThinkingSurface(event.thinking, event._thinkingId, ctx);
    if (!thinkingNode) return node;
    const fragment = document.createDocumentFragment();
    fragment.appendChild(node);
    fragment.appendChild(thinkingNode);
    return fragment;
  }

  function buildThinkingSurface(thinking, thinkingId, ctx) {
    const contentText = String(thinking?.content || '');
    if (!contentText.trim() || ctx.thinkingSurfaces === 'never') return null;

    const content = document.createElement('div');
    content.className = 'chat-thinking-content';
    content.id = thinkingId;
    content.textContent = contentText;

    if (ctx.thinkingSurfaces === 'always') {
      return content;
    }

    const fragment = document.createDocumentFragment();
    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'chat-thinking-expander';
    button.dataset.thinkingId = thinkingId;
    button.setAttribute('aria-expanded', 'false');
    button.textContent = window.solChatCopy.CHAT_THINKING_EXPANDER_LABEL;
    content.hidden = true;
    fragment.appendChild(button);
    fragment.appendChild(content);
    return fragment;
  }

  function buildBubble(author, text, side, notes) {
    const bubble = document.createElement('article');
    bubble.className = 'chat-bubble chat-bubble--' + side;
    bubble.setAttribute('aria-label', author + ': ' + text);
    if (notes) bubble.title = notes;

    const authorNode = document.createElement('span');
    authorNode.className = 'chat-bubble-author';
    authorNode.textContent = author;

    const textNode = document.createElement(side === 'sol' ? 'div' : 'span');
    if (side === 'sol') {
      textNode.className = 'chat-bubble-text chat-bubble-text--markdown';
      renderMarkdownInto(textNode, text);
    } else {
      textNode.className = 'chat-bubble-text';
      textNode.textContent = text;
    }

    bubble.appendChild(authorNode);
    bubble.appendChild(textNode);
    return bubble;
  }

  function buildQueuedTalentCard(event) {
    const card = document.createElement('div');
    card.className = 'chat-talent-card chat-talent-card--queued';
    card.dataset.talentUseId = event.use_id;
    card.dataset.talentStatus = 'queued';

    const labelNode = document.createElement('span');
    labelNode.className = 'chat-talent-card-label';
    labelNode.textContent = window.solChatCopy.CHAT_TALENT_QUEUED_LABEL;
    card.appendChild(labelNode);

    if (event.task) {
      const taskNode = document.createElement('span');
      taskNode.className = 'chat-talent-card-task';
      taskNode.textContent = event.task;
      card.appendChild(taskNode);
    }

    return card;
  }

  function buildTalentCard(label, detail, useId, status, variantClass) {
    const card = document.createElement('button');
    card.type = 'button';
    card.className = 'chat-talent-card ' + variantClass;
    card.dataset.talentUseId = useId;
    card.dataset.talentStatus = status;

    const labelNode = document.createElement('span');
    labelNode.className = 'chat-talent-card-label';
    labelNode.textContent = label;
    card.appendChild(labelNode);

    if (detail) {
      if (status === 'finished' || status === 'errored') {
        const detailNode = document.createElement('div');
        detailNode.className = 'chat-talent-card-detail chat-talent-card-detail--markdown';
        renderMarkdownInto(detailNode, detail || '');
        card.appendChild(detailNode);
      } else {
        const detailNode = document.createElement('span');
        detailNode.className = 'chat-talent-card-detail';
        detailNode.textContent = detail;
        card.appendChild(detailNode);
      }
    }

    return card;
  }

  function buildReflectionCard(dayValue, url) {
    const card = document.createElement('article');
    card.className = 'chat-reflection-card';

    const labelNode = document.createElement('span');
    labelNode.className = 'chat-reflection-card-label';
    labelNode.textContent = 'weekly reflection ready';
    card.appendChild(labelNode);

    const weekNode = document.createElement('span');
    weekNode.className = 'chat-reflection-card-week';
    weekNode.textContent = 'week of ' + dayValue;
    card.appendChild(weekNode);

    const linkNode = document.createElement('a');
    linkNode.className = 'chat-reflection-card-link';
    linkNode.href = url;
    linkNode.textContent = 'open reflection';
    card.appendChild(linkNode);

    return card;
  }

  function renderOriginTag(meta, ctx) {
    const tag = document.createElement('div');
    tag.className = 'chat-origin-tag';
    tag.dataset.originToggleHost = '';

    const label = document.createElement('span');
    label.className = 'chat-origin-tag__label';
    label.textContent = formatOriginLabel(meta, ctx);
    tag.appendChild(label);

    const button = document.createElement('button');
    button.type = 'button';
    button.className = 'chat-origin-tag__toggle';
    button.dataset.originToggle = '';
    button.textContent = ORIGIN_COPY.show;
    tag.appendChild(button);

    const provenance = document.createElement('div');
    provenance.className = 'chat-origin-provenance';
    provenance.hidden = true;
    appendOriginProvenanceValue(provenance, ORIGIN_COPY.triggerTalentLabel, meta.trigger_talent || '');
    appendOriginProvenanceValue(provenance, ORIGIN_COPY.dedupeLabel, meta.dedupe || '');
    appendOriginProvenanceValue(provenance, ORIGIN_COPY.sinceTsLabel, formatOriginSinceTs(meta.since_ts));
    tag.appendChild(provenance);

    return tag;
  }

  function formatOriginLabel(meta, ctx) {
    const time = meta.time || ctx.timeFormatter.format(new Date(meta.ts));
    const label = meta.trigger_talent
      ? ORIGIN_COPY.withTalent
        .replace('{trigger_talent}', meta.trigger_talent)
        .replace('{time}', time)
      : ORIGIN_COPY.withoutTalent.replace('{time}', time);
    if (meta.superseded_by_id) {
      return label + ORIGIN_COPY.supersededSuffix.replace('{time}', meta.superseded_time);
    }
    return label;
  }

  function formatOriginSinceTs(rawTs) {
    const numericTs = Number(rawTs);
    if (!Number.isFinite(numericTs) || numericTs <= 0) return '';
    return ORIGIN_SINCE_FORMATTER.format(new Date(numericTs));
  }

  function appendOriginProvenanceValue(provenance, labelText, value) {
    if (!value) return;
    const span = document.createElement('span');
    const label = document.createElement('strong');
    label.textContent = labelText;
    span.appendChild(label);
    span.appendChild(document.createTextNode(' ' + value));
    provenance.appendChild(span);
  }

  function renderDispatchOriginTag(dispatchOrigin, ctx) {
    const logicalUseId = String(dispatchOrigin?.logical_use_id || '');
    const tag = document.createElement('div');
    tag.className = 'chat-dispatch-origin';
    tag.dataset.dispatchLogicalId = logicalUseId;

    const target = findDispatchOriginTarget(ctx.transcript, logicalUseId);
    if (target) {
      tag.classList.add('chat-dispatch-origin--locatable');
      tag.setAttribute('role', 'button');
      tag.tabIndex = 0;
    }

    const prefix = document.createElement('span');
    prefix.className = 'chat-dispatch-origin__prefix';
    prefix.textContent = window.solChatCopy.CHAT_DISPATCH_ORIGIN_PREFIX;
    tag.appendChild(prefix);
    tag.appendChild(document.createTextNode(' '));

    const ask = document.createElement('span');
    ask.className = 'chat-dispatch-origin__ask';
    ask.textContent = String(dispatchOrigin?.ask || '');
    tag.appendChild(ask);

    return tag;
  }

  function renderMarkdownInto(node, source) {
    // Single path from markdown source to rendered HTML.
    if (!window.AppServices || typeof window.AppServices.renderMarkdown !== 'function') {
      throw new Error('markdown renderer is unavailable');
    }
    node.innerHTML = window.AppServices.renderMarkdown(source || '');
  }

  function findDispatchOriginTarget(transcript, logicalUseId) {
    const useId = String(logicalUseId || '');
    if (!useId) return null;
    return transcript.querySelector(`[data-use-id="${CSS.escape(useId)}"]`);
  }

  window.solChatRender = {
    renderEventItem,
    findDispatchOriginTarget,
    ORIGIN_COPY
  };
})();
