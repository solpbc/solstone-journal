// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  const bundle = globalThis.SolstoneI18nBundles['journal-window'];
  const sourceLocale = 'en-US';
  const locale = globalThis.document?.documentElement?.lang;
  const language = locale && locale !== 'en' ? locale : sourceLocale;
  const prefix = 'journal.window.chrome.';
  const dynamicMessages = new Set(['view.document.title', 'zone.time', 'day.zones.part', 'day.zones.all']);

  function renderMessage(selected, id, args = []) {
    return bundle.render(selected, id, args, { useIsolating: selected !== sourceLocale });
  }

  function render(id, args) {
    return renderMessage(language, id, args);
  }

  function renderEnglish(id, args) {
    return renderMessage(sourceLocale, id, args);
  }

  function cardinalQuantity(value) {
    if (!Number.isInteger(value) || value < 0) return 0;
    // Preserve the plural family of large counts within the cardinal range.
    return value <= 2147483647 ? value : 1000000 + value % 1000000;
  }

  function escapeMarkup(value) {
    return value.replaceAll('&', '&amp;').replaceAll('<', '&lt;')
      .replaceAll('>', '&gt;').replaceAll('"', '&quot;').replaceAll("'", '&#39;');
  }

  // The message places named elements or wraps inline text with start/end
  // arguments. Catalog text and argument values stay text nodes.
  function renderMarkup(id, args, { elements = {}, fixed = {} }) {
    const fence = '\uFDD0';
    const markers = /\uFDD0(?:\uFDD0|([+=-])(\d+)\uFDD1)/gu;
    const pairs = Object.entries(elements);
    const bindings = pairs.concat(Object.entries(fixed));
    const input = args.map(([name, value]) => [name,
      typeof value === 'string' ? value.replaceAll(fence, fence + fence) : value]);
    for (const [index, [name]] of bindings.entries()) {
      if (index < pairs.length) {
        input.push([`${name}_start`, `${fence}+${index}\uFDD1`],
          [`${name}_end`, `${fence}-${index}\uFDD1`]);
      } else {
        input.push([name, `${fence}=${index}\uFDD1`]);
      }
    }
    const message = render(id, input);
    const parts = [];
    const open = [];
    const used = new Set();
    const invalid = () => new Error(`Invalid inline elements in ${id}`);
    const append = part => {
      const parent = open.at(-1)?.node;
      if (parent) parent.appendChild(typeof part === 'string' ? document.createTextNode(part) : part);
      else parts.push(part);
    };
    let position = 0;
    for (const match of message.matchAll(markers)) {
      const [marker, edge, index] = match;
      const end = match.index + marker.length;
      const isolated = edge && language !== sourceLocale
        && message[match.index - 1] === '\u2068' && message[end] === '\u2069';
      append(message.slice(position, isolated ? match.index - 1 : match.index));
      position = isolated ? end + 1 : end;
      if (!edge) {
        append(fence);
      } else if (edge !== '-') {
        const node = bindings[Number(index)]?.[1];
        if (!node || used.has(index) || (edge === '+') !== (Number(index) < pairs.length)) throw invalid();
        used.add(index);
        if (isolated) append('\u2068');
        append(node);
        if (edge === '+') open.push({ index, node });
        else if (isolated) append('\u2069');
      } else {
        if (open.pop()?.index !== index) throw invalid();
        if (isolated) append('\u2069');
      }
    }
    if (open.length || used.size !== bindings.length) throw invalid();
    append(message.slice(position));
    return parts;
  }

  // Only shipped workspace templates contain message markers. Replace them
  // before inserting the workspace so the locale never changes asset bytes.
  function renderWorkspace(markup) {
    return markup.replace(/\{\{(!?)(journal\.window\.[a-z0-9.]+)\}\}/g, (_, restricted, id) =>
      escapeMarkup(restricted ? renderEnglish(id) : render(id)));
  }

  window.JournalI18n = Object.freeze({
    render,
    renderEnglish,
    renderMarkup,
    cardinalQuantity,
    renderHtml: (id, args) => escapeMarkup(render(id, args)),
    renderEnglishHtml: (id, args) => escapeMarkup(renderEnglish(id, args)),
    renderWorkspace,
    locale: language,
    formatLocale: language === sourceLocale ? undefined : language,
  });
  window.CONVEY_COPY = Object.freeze(Object.fromEntries(
    bundle.messageIds
      .filter(id => id.startsWith(prefix) && !dynamicMessages.has(id.slice(prefix.length)))
      .map(id => {
        const key = id.slice(prefix.length);
        const format = key === 'report.privacy.note' ? renderEnglish : render;
        return [key.replaceAll('.', '_').toUpperCase(), format(id)];
      }),
  ));
})();
