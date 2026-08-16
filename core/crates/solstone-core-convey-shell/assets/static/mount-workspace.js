// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function () {
  function getTarget(options) {
    if (options && options.target) return options.target;
    return document.getElementById('main-content');
  }

  function renderFailure(target, retry) {
    if (window.SurfaceState) {
      target.innerHTML = window.SurfaceState.error({ retry: true });
    } else {
      target.innerHTML =
        '<div class="surface-state surface-state--error" role="alert">' +
        '<h2 class="surface-state-heading">Couldn\'t load this section</h2>' +
        '<p class="surface-state-desc">reload to try again.</p>' +
        '<button type="button" class="surface-state-retry">Try again</button>' +
        '</div>';
    }
    const button = target.querySelector('.surface-state-retry');
    if (button) {
      button.addEventListener('click', retry, { once: true });
    }
  }

  function cloneScript(script) {
    const clone = document.createElement('script');
    for (const attr of script.attributes) {
      clone.setAttribute(attr.name, attr.value);
    }
    if (!script.src) {
      clone.text = script.textContent;
    }
    return clone;
  }

  function runScript(script) {
    return new Promise((resolve, reject) => {
      const clone = cloneScript(script);
      if (clone.src) {
        clone.async = false;
        clone.addEventListener('load', () => resolve(), { once: true });
        clone.addEventListener(
          'error',
          () => reject(new Error(`Failed to load script ${clone.src}`)),
          { once: true }
        );
        script.replaceWith(clone);
        return;
      }
      script.replaceWith(clone);
      resolve();
    });
  }

  async function replayScripts(target) {
    const scripts = Array.from(target.querySelectorAll('script'));
    for (const script of scripts) {
      await runScript(script);
    }
  }

  async function mountWorkspaceFragment(url, options = {}) {
    const target = getTarget(options);
    if (!target) {
      throw new Error('mountWorkspaceFragment requires a target');
    }

    const retry = () => {
      mountWorkspaceFragment(url, options).catch((error) => {
        if (window.logError) {
          window.logError(error, { context: 'workspace-fragment-retry', url });
        }
      });
    };

    try {
      const response = await fetch(url, { credentials: 'same-origin' });
      if (!response.ok) {
        throw new Error(`Request failed (HTTP ${response.status})`);
      }
      const html = await response.text();
      target.innerHTML = html;
      await replayScripts(target);
      document.dispatchEvent(
        new CustomEvent('workspace:mounted', {
          detail: { appName: options.appName || null, url: url }
        })
      );
    } catch (error) {
      renderFailure(target, retry);
      if (typeof options.onRetry === 'function') {
        options.onRetry(error);
      }
      if (window.logError) {
        window.logError(error, { context: 'workspace-fragment-mount', url });
      }
      throw error;
    }
  }

  window.mountWorkspaceFragment = mountWorkspaceFragment;
})();
