// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  let pending = false;

  // A question belongs to the view that asked it. Leaving is always cancel;
  // only an answer on that same page can authorize its write.
  function ask({ message, action, value, signal, opener = document.activeElement }) {
    if (pending || signal?.aborted) return Promise.resolve(null);
    pending = true;
    const address = global.location.href;
    const editing = value !== undefined;
    const overlay = document.createElement('div');
    overlay.className = 'modal convey-question';
    overlay.setAttribute('role', 'dialog');
    overlay.setAttribute('aria-modal', 'true');
    overlay.setAttribute('aria-labelledby', 'convey-question-title');
    const form = document.createElement('form');
    form.className = 'modal-content';
    const title = document.createElement('h2');
    title.id = 'convey-question-title';
    title.textContent = message;
    form.appendChild(title);
    let input;
    if (editing) {
      const label = document.createElement('label');
      label.setAttribute('for', 'convey-question-input');
      label.textContent = window.JournalI18n.render('journal.window.shared.question.visible.text.name');
      input = document.createElement('input');
      input.id = 'convey-question-input';
      input.type = 'text';
      input.value = value;
      form.appendChild(label);
      form.appendChild(input);
    }
    const actions = document.createElement('div');
    actions.className = 'convey-question-actions';
    const cancel = document.createElement('button');
    cancel.type = 'button';
    cancel.setAttribute('data-question-cancel', '');
    cancel.textContent = window.JournalI18n.render('journal.window.shared.question.visible.text.cancel');
    const accept = document.createElement('button');
    accept.type = 'submit';
    accept.setAttribute('data-question-accept', '');
    accept.textContent = action;
    actions.appendChild(cancel);
    actions.appendChild(accept);
    form.appendChild(actions);
    overlay.appendChild(form);

    return new Promise(resolve => {
      let finished = false;
      function finish(answer, moving = false) {
        if (finished) return;
        finished = true;
        pending = false;
        signal?.removeEventListener('abort', leave);
        global.removeEventListener('popstate', leave);
        global.removeEventListener('hashchange', leave);
        global.removeEventListener('pagehide', leave);
        overlay.remove();
        global.ConveyModalLayer.reconcile({ restore: false });
        if (!moving) {
          const target = typeof opener === 'function' ? opener() : opener;
          if (target?.isConnected && !target.disabled) target.focus({ preventScroll: true });
          else document.getElementById('main-content')?.focus({ preventScroll: true });
        }
        resolve(moving || signal?.aborted || global.location.href !== address ? null : answer);
      }
      function leave() { finish(null, true); }
      function submit(event) {
        event.preventDefault();
        finish(editing ? input.value : true);
      }
      cancel.addEventListener('click', () => finish(null));
      accept.addEventListener('click', submit);
      form.addEventListener('submit', submit);
      overlay.addEventListener('click', event => { if (event.target === overlay) finish(null); });
      // A covered question never hears Escape from the dialog above it.
      overlay.addEventListener('keydown', event => {
        if (event.key !== 'Escape' || event.defaultPrevented) return;
        event.preventDefault();
        event.stopPropagation();
        finish(null);
      });
      signal?.addEventListener('abort', leave, { once: true });
      global.addEventListener('popstate', leave);
      global.addEventListener('hashchange', leave);
      global.addEventListener('pagehide', leave);
      document.body.appendChild(overlay);
      global.ConveyModalLayer.reconcile();
      (editing ? input : cancel).focus();
    });
  }

  global.SolQuestion = { ask };
})(window);
