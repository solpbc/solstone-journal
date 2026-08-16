// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

(function (global) {
  const MANAGED_DIALOG_SELECTOR = '[role="dialog"][aria-modal="true"]';
  // Assignment §8 items 1-2: shell-owned dialogs stay outside this app workspace layer.
  const UNMANAGED_DIALOG_SELECTOR = '#talentViewModal, .report-error-modal';
  const BODY_ACTIVE_CLASS = 'has-managed-dialog';
  const ACTIVE_DIALOG_ATTR = 'data-convey-active-dialog';
  const ACTIVE_HOST_ATTR = 'data-convey-active-dialog-host';
  const FOCUSABLE_SELECTOR = [
    'a[href]',
    'area[href]',
    'button',
    'input',
    'select',
    'textarea',
    'summary',
    'iframe',
    'object',
    'embed',
    '[contenteditable="true"]',
    '[contenteditable=""]',
    '[tabindex]',
  ].join(',');

  let initialized = false;
  let observer = null;
  let reconcileQueued = false;
  let activeDialog = null;
  let activeHost = null;
  let lastOutsideFocus = null;
  let lastTabWasBackward = false;
  const inertRecords = new Map();
  const temporaryTabIndexDialogs = new Set();

  function bodyElement() {
    if (!document.body) {
      throw new Error('managed modal layer requires document.body');
    }
    return document.body;
  }

  function elementChildren(element) {
    return Array.from(element.children || []);
  }

  function hasHiddenAttribute(element) {
    return Boolean(
      element.hidden
      || (typeof element.hasAttribute === 'function' && element.hasAttribute('hidden'))
    );
  }

  function isInDocumentBody(element) {
    for (let node = element; node; node = node.parentElement) {
      if (node === document.body) return true;
    }
    return false;
  }

  function isRenderedOpen(element) {
    for (let node = element; node; node = node.parentElement) {
      if (hasHiddenAttribute(node)) return false;
      if (node.style && node.style.display === 'none') return false;
      if (global.getComputedStyle(node).display === 'none') return false;
      if (node === document.body) return true;
    }
    return false;
  }

  function isManagedDialog(element) {
    return Boolean(
      element
      && typeof element.matches === 'function'
      && element.matches(MANAGED_DIALOG_SELECTOR)
      && !element.matches(UNMANAGED_DIALOG_SELECTOR)
    );
  }

  function managedDialogs() {
    return Array.from(bodyElement().querySelectorAll(MANAGED_DIALOG_SELECTOR))
      .filter((dialog) => !dialog.matches(UNMANAGED_DIALOG_SELECTOR));
  }

  function findActiveDialog() {
    let visibleDialog = null;
    managedDialogs().forEach((dialog) => {
      if (isRenderedOpen(dialog)) {
        // If multiple managed dialogs are visible, the last in document order is active.
        visibleDialog = dialog;
      }
    });
    return visibleDialog;
  }

  function findActiveHost(dialog) {
    if (global.getComputedStyle(dialog).position !== 'static') {
      return null;
    }
    // Speakers is static inside a positioned backdrop; positioned dialogs need no host.
    for (let node = dialog.parentElement; node && node !== document.body; node = node.parentElement) {
      if (global.getComputedStyle(node).position !== 'static') {
        return node;
      }
    }
    return null;
  }

  function clearMarkers() {
    if (activeDialog) {
      activeDialog.removeAttribute(ACTIVE_DIALOG_ATTR);
    }
    if (activeHost) {
      activeHost.removeAttribute(ACTIVE_HOST_ATTR);
    }
    activeDialog = null;
    activeHost = null;
  }

  function applyMarkers(dialog) {
    if (activeDialog !== dialog) {
      clearMarkers();
    } else if (activeHost) {
      activeHost.removeAttribute(ACTIVE_HOST_ATTR);
      activeHost = null;
    }

    activeDialog = dialog;
    activeHost = findActiveHost(dialog);
    activeDialog.setAttribute(ACTIVE_DIALOG_ATTR, '');
    if (activeHost) {
      activeHost.setAttribute(ACTIVE_HOST_ATTR, '');
    }
  }

  function isElementInert(element) {
    return Boolean(
      element.inert
      || (typeof element.hasAttribute === 'function' && element.hasAttribute('inert'))
    );
  }

  function rememberAndInert(element) {
    if (!inertRecords.has(element)) {
      inertRecords.set(element, { wasInert: isElementInert(element) });
    }
    element.inert = true;
    element.setAttribute('inert', '');
  }

  function restoreInert(element, record) {
    if (record.wasInert) {
      element.inert = true;
      element.setAttribute('inert', '');
      return;
    }
    element.inert = false;
    element.removeAttribute('inert');
  }

  function collectInertSiblings(dialog) {
    const siblings = new Set();
    let branch = dialog;
    for (let parent = dialog.parentElement; parent; parent = parent.parentElement) {
      elementChildren(parent).forEach((child) => {
        if (child !== branch) {
          siblings.add(child);
        }
      });
      if (parent === document.body) {
        return siblings;
      }
      branch = parent;
    }
    return siblings;
  }

  function applyInert(dialog) {
    const desired = dialog ? collectInertSiblings(dialog) : new Set();

    inertRecords.forEach((record, element) => {
      if (!element.isConnected || !desired.has(element)) {
        restoreInert(element, record);
        inertRecords.delete(element);
      }
    });

    desired.forEach((element) => {
      rememberAndInert(element);
    });
  }

  function isInsideManagedDialog(element) {
    for (let node = element; node && node !== document.body; node = node.parentElement) {
      if (isManagedDialog(node)) return true;
    }
    return false;
  }

  function hasInertAncestor(element) {
    for (let node = element; node && node !== document.body; node = node.parentElement) {
      if (isElementInert(node)) return true;
    }
    return false;
  }

  function isDisabled(element) {
    return Boolean(element.disabled || element.getAttribute('aria-disabled') === 'true');
  }

  function hasNegativeTabIndex(element) {
    const value = element.getAttribute('tabindex');
    return value !== null && Number(value) < 0;
  }

  function isFocusableCandidate(element) {
    return Boolean(
      typeof element.focus === 'function'
      && !isDisabled(element)
      && !hasNegativeTabIndex(element)
      && !hasInertAncestor(element)
      && isRenderedOpen(element)
    );
  }

  function focusableCandidates(dialog) {
    return Array.from(dialog.querySelectorAll(FOCUSABLE_SELECTOR))
      .filter(isFocusableCandidate);
  }

  function ensureDialogFocusable(dialog) {
    if (dialog.hasAttribute('tabindex')) return;
    temporaryTabIndexDialogs.add(dialog);
    dialog.setAttribute('tabindex', '-1');
  }

  function restoreDialogTabIndex(keep) {
    temporaryTabIndexDialogs.forEach((dialog) => {
      if (dialog === keep) return;
      dialog.removeAttribute('tabindex');
      temporaryTabIndexDialogs.delete(dialog);
    });
  }

  function focusDialog(dialog, reverse) {
    const candidates = focusableCandidates(dialog);
    const target = reverse ? candidates[candidates.length - 1] : candidates[0];
    if (target) {
      target.focus();
      return;
    }
    ensureDialogFocusable(dialog);
    dialog.focus();
  }

  function shouldWrapFocus(dialog, focused, reverse) {
    if (!focused || !dialog.contains(focused)) return true;
    const candidates = focusableCandidates(dialog);
    if (!candidates.length || focused === dialog) return true;
    return focused === (reverse ? candidates[0] : candidates[candidates.length - 1]);
  }

  function restoreOpener() {
    const opener = lastOutsideFocus;
    if (
      opener
      && opener.isConnected
      && typeof opener.focus === 'function'
      && !isDisabled(opener)
      && !hasInertAncestor(opener)
      && isRenderedOpen(opener)
    ) {
      opener.focus();
    }
  }

  function reconcile() {
    const previousDialog = activeDialog;
    const dialog = findActiveDialog();

    if (!dialog) {
      bodyElement().classList.remove(BODY_ACTIVE_CLASS);
      clearMarkers();
      applyInert(null);
      restoreDialogTabIndex(null);
      if (previousDialog) {
        restoreOpener();
      }
      return;
    }

    bodyElement().classList.add(BODY_ACTIVE_CLASS);
    applyMarkers(dialog);
    applyInert(dialog);
    restoreDialogTabIndex(dialog);

    if (previousDialog !== dialog) {
      const focused = document.activeElement;
      if (!focused || !dialog.contains(focused)) {
        focusDialog(dialog, false);
      }
    }
  }

  function scheduleReconcile() {
    if (reconcileQueued) return;
    reconcileQueued = true;
    global.requestAnimationFrame(() => {
      reconcileQueued = false;
      reconcile();
    });
  }

  function handleFocusIn(event) {
    const target = event.target;
    if (!target || !isInDocumentBody(target)) return;

    if (!activeDialog && !isInsideManagedDialog(target)) {
      lastOutsideFocus = target;
      return;
    }

    if (activeDialog && !activeDialog.contains(target)) {
      focusDialog(activeDialog, lastTabWasBackward);
    }
  }

  function handleKeyDown(event) {
    if (event.key !== 'Tab') return;
    lastTabWasBackward = Boolean(event.shiftKey);
    if (!activeDialog) return;
    if (!shouldWrapFocus(activeDialog, document.activeElement, lastTabWasBackward)) return;
    event.preventDefault();
    event.stopPropagation();
    focusDialog(activeDialog, lastTabWasBackward);
  }

  function init() {
    if (initialized) return;
    initialized = true;

    observer = new MutationObserver(scheduleReconcile);
    observer.observe(bodyElement(), {
      attributes: true,
      attributeFilter: ['hidden', 'style', 'class', 'aria-modal', 'role'],
      childList: true,
      subtree: true,
    });
    document.addEventListener('focusin', handleFocusIn, true);
    document.addEventListener('keydown', handleKeyDown, true);
    document.addEventListener('workspace:mounted', reconcile);
    reconcile();
  }

  global.ConveyModalLayer = { init, reconcile };
  global.whenShellReady(init);
})(window);
