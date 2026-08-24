// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

/**
 * App System JavaScript
 * Handles facet selection and shared shell UI behavior
 *
 * Requires:
 * - window.facetsData - Array of facet objects from the hosting shell
 * - window.selectedFacet - Currently selected facet name or null
 * - window.appFacetCounts - Object mapping facet names to counts
 *
 * Public API:
 * - window.selectedFacet - Current facet selection (read/write)
 * - window.selectFacet(name) - Change facet selection programmatically
 * - 'facet.switch' event - Dispatched when selection changes
 */

(function(){
  // Facet filtering state
  let activeFacets = [];

  // Save facet selection to cookie (server-driven)
  function saveSelectedFacetToCookie(facet) {
    if (facet) {
      const expires = new Date();
      expires.setFullYear(expires.getFullYear() + 1);
      document.cookie = `selectedFacet=${facet}; expires=${expires.toUTCString()}; path=/; SameSite=Lax`;
    } else {
      document.cookie = 'selectedFacet=; expires=Thu, 01 Jan 1970 00:00:00 UTC; path=/; SameSite=Lax';
    }
  }

  // Convert hex color to rgba with opacity
  function hexToRgba(hex, alpha) {
    if (!hex || hex.length < 6) return `rgba(128,128,128,${alpha})`;
    const r = parseInt(hex.substring(1,3), 16);
    const g = parseInt(hex.substring(3,5), 16);
    const b = parseInt(hex.substring(5,7), 16);
    return `rgba(${r},${g},${b},${alpha})`;
  }

  // Apply global theme CSS variables based on selected facet
  function applyFacetTheme(selectedFacetData) {
    if (selectedFacetData && selectedFacetData.color) {
      const color = selectedFacetData.color;
      document.documentElement.style.setProperty('--facet-color', color);
      document.documentElement.style.setProperty('--facet-bg', color + '1a');
      document.documentElement.style.setProperty('--facet-border', color);
    } else {
      document.getElementById('facet-theme')?.remove();
      document.documentElement.style.removeProperty('--facet-color');
      document.documentElement.style.removeProperty('--facet-bg');
      document.documentElement.style.removeProperty('--facet-border');
    }
  }

  // Apply pill styling based on selection state
  function applyPillStyle(pill, facet, isSelected) {
    // Store color for CSS hover effects
    if (facet.color) {
      pill.style.setProperty('--pill-color', facet.color);
      pill.style.setProperty('--pill-bg', hexToRgba(facet.color, 0.2));
      pill.style.setProperty('--pill-bg-rest', hexToRgba(facet.color, 0.08));
    }

    if (isSelected) {
      pill.classList.add('selected');
      pill.style.background = '';
      pill.style.color = '';
      pill.style.borderColor = '';
      pill.title = 'show all facets';
    } else {
      pill.classList.remove('selected');
      pill.style.background = '';
      pill.style.color = '';
      pill.style.borderColor = '';
      pill.title = `filter by ${facet.title}`;
    }
    pill.setAttribute('aria-pressed', String(isSelected));
    pill.style.boxShadow = '';
  }

  // Load facets from embedded data
  function loadFacetChooser() {
    activeFacets = window.facetsData || [];

    // Enrich facets with app-specific counts (injected by app.html)
    const appCounts = window.appFacetCounts || {};
    activeFacets.forEach(facet => {
      facet.count = appCounts[facet.name] || 0;
    });

    renderFacetChooser();
  }

  // Render facet pills in top bar
  function renderFacetChooser() {
    const facetPillsContainer = document.querySelector('.facet-pills-container');
    if (!facetPillsContainer) return;

    facetPillsContainer.innerHTML = '';

    // Check if facets are disabled for this app
    const facetBar = document.querySelector('.facet-bar');
    const facetsDisabled = facetBar?.classList.contains('facets-disabled');
    if (facetsDisabled) {
      facetPillsContainer.setAttribute('aria-hidden', 'true');
      facetPillsContainer.removeAttribute('role');
      facetPillsContainer.removeAttribute('aria-label');
      updateFacetScrollAffordance();
      return;
    } else if (activeFacets.length === 0) {
      facetPillsContainer.removeAttribute('aria-hidden');
      facetPillsContainer.removeAttribute('role');
      facetPillsContainer.removeAttribute('aria-label');

      if (facetBar) {
        const statusText = 'no facets configured';
        const existingStatus = document.querySelector('.facet-filter-status');
        if (existingStatus) {
          existingStatus.textContent = statusText;
        } else {
          const statusEl = document.createElement('span');
          statusEl.className = 'facet-filter-status visually-hidden';
          statusEl.setAttribute('aria-live', 'polite');
          statusEl.setAttribute('aria-atomic', 'true');
          statusEl.textContent = statusText;
          facetBar.appendChild(statusEl);
        }
      }

      const emptyState = document.createElement('span');
      emptyState.className = 'facet-empty-state';
      emptyState.textContent = 'no facets yet \u2014 ';

      const createBtn = document.createElement('button');
      createBtn.className = 'facet-empty-create';
      createBtn.textContent = 'create one';
      createBtn.onclick = () => openFacetCreateModal();

      emptyState.appendChild(createBtn);
      facetPillsContainer.appendChild(emptyState);
      updateFacetScrollAffordance();
      return;
    } else {
      facetPillsContainer.setAttribute('role', 'toolbar');
      facetPillsContainer.setAttribute('aria-label', 'facet filter');
      facetPillsContainer.removeAttribute('aria-hidden');
    }

    // Find selected facet data and apply theme
    const selectedFacetData = window.selectedFacet ? activeFacets.find(f => f.name === window.selectedFacet) : null;

    if (!facetsDisabled && facetBar) {
      const statusText = selectedFacetData
        ? 'filtered to ' + selectedFacetData.title
        : 'viewing all facets';
      const existingStatus = document.querySelector('.facet-filter-status');
      if (existingStatus) {
        existingStatus.textContent = statusText;
      } else {
        const statusEl = document.createElement('span');
        statusEl.className = 'facet-filter-status visually-hidden';
        statusEl.setAttribute('aria-live', 'polite');
        statusEl.setAttribute('aria-atomic', 'true');
        statusEl.textContent = statusText;
        facetBar.appendChild(statusEl);
      }
    }

    if (!facetsDisabled) {
      applyFacetTheme(selectedFacetData);

      const allLabel = document.createElement('span');
      allLabel.className = 'facet-all-label';
      allLabel.textContent = 'all';
      allLabel.setAttribute('aria-hidden', 'true');
      if (selectedFacetData) {
        allLabel.style.display = 'none';
      }
      facetPillsContainer.appendChild(allLabel);
    }

    // Facet pills
    activeFacets.forEach(facet => {
      const pill = document.createElement('button');
      pill.className = 'facet-pill';

      if (facet.icon_svg || facet.emoji) {
        const emojiContainer = document.createElement('div');
        emojiContainer.className = 'emoji-container';

        const emoji = document.createElement('span');
        emoji.className = 'emoji';
        if (facet.icon_svg) {
          emoji.innerHTML = facet.icon_svg;
        } else {
          emoji.textContent = facet.emoji;
        }
        emojiContainer.appendChild(emoji);

        // Add badge if count > 0
        const count = facet.count || 0;
        if (count > 0) {
          const badge = document.createElement('span');
          badge.className = 'facet-badge';
          badge.textContent = count;
          badge.setAttribute('aria-label', count + ' pending');
          emojiContainer.appendChild(badge);
        }

        pill.appendChild(emojiContainer);
      }

      const label = document.createElement('span');
      label.className = 'label';
      label.textContent = facet.title;
      pill.appendChild(label);

      // Apply styling and interactivity (only if facets enabled)
      if (!facetsDisabled) {
        const isSelected = window.selectedFacet === facet.name;
        applyPillStyle(pill, facet, isSelected);
        pill.tabIndex = isSelected ? 0 : -1;

        // Click to select, or click again to deselect (show all facets)
        pill.onclick = () => {
          if (window.selectedFacet === facet.name) {
            selectFacet(null);  // Deselect to show all
          } else {
            selectFacet(facet.name);
          }
        };

        // Setup for drag-and-drop (attributes added here, listeners added in init)
        pill.dataset.facetName = facet.name;
        pill.draggable = true;
      }

      facetPillsContainer.appendChild(pill);
    });

    // Ensure at least one pill is tabbable (handles null selection and stale facet names)
    if (!facetsDisabled && !facetPillsContainer.querySelector('.facet-pill[tabindex="0"]')) {
      const firstPill = facetPillsContainer.querySelector('.facet-pill');
      if (firstPill) firstPill.tabIndex = 0;
    }

    // Add "+" button to create new facets (only in settings app)
    const currentApp = window.location.pathname.split('/')[2];
    if (!facetsDisabled && currentApp === 'settings') {
      const addButton = document.createElement('button');
      addButton.className = 'facet-add-pill';
      addButton.textContent = '+';
      addButton.title = 'create new facet';
      addButton.setAttribute('aria-label', 'add facet');
      addButton.onclick = () => openFacetCreateModal();
      facetPillsContainer.appendChild(addButton);
    }

    // Re-apply dynamic badge counts that survived in memory
    const badgeSvc = window.AppServices?.badges?.facet;
    if (badgeSvc) {
      for (const name of Object.keys(badgeSvc._data)) {
        badgeSvc._render(name);
      }
    }

    updateFacetScrollAffordance();
  }

  // Update selection styles without re-rendering
  function updateFacetSelection() {
    const container = document.querySelector('.facet-pills-container');
    if (!container) return;

    const pills = container.querySelectorAll('.facet-pill');
    const facetsDisabled = document.querySelector('.facet-bar')?.classList.contains('facets-disabled');

    if (facetsDisabled) return;

    // Apply theme
    const selectedFacetData = window.selectedFacet ? activeFacets.find(f => f.name === window.selectedFacet) : null;
    applyFacetTheme(selectedFacetData);

    const statusEl = document.querySelector('.facet-filter-status');
    if (statusEl) {
      if (window.selectedFacet) {
        const facetData = activeFacets.find(f => f.name === window.selectedFacet);
        statusEl.textContent = 'filtered to ' + (facetData ? facetData.title : window.selectedFacet);
      } else {
        statusEl.textContent = 'viewing all facets';
      }
    }

    const allLabel = container.querySelector('.facet-all-label');
    if (allLabel) {
      allLabel.style.display = window.selectedFacet ? 'none' : '';
    }

    // Update each pill
    pills.forEach((pill, index) => {
      const facet = activeFacets[index];
      if (!facet) return;

      const isSelected = window.selectedFacet === facet.name;
      applyPillStyle(pill, facet, isSelected);
      pill.tabIndex = isSelected ? 0 : -1;
    });

    // Ensure at least one pill is tabbable (handles null selection and stale facet names)
    if (!container.querySelector('.facet-pill[tabindex="0"]')) {
      const firstPill = container.querySelector('.facet-pill');
      if (firstPill) firstPill.tabIndex = 0;
    }
  }

  // Handle facet selection
  // fromPopState: true when called from browser back/forward navigation
  function selectFacet(facet, fromPopState) {
    window.selectedFacet = facet;
    saveSelectedFacetToCookie(facet);

    // Notify backend immediately (non-blocking, best-effort)
    fetch('/api/config/facets/select', {
      method: 'POST',
      headers: {'Content-Type': 'application/json'},
      body: JSON.stringify({facet: facet})
    }).catch(() => {}); // Ignore errors - cookie sync is fallback

    updateFacetSelection();

    // Push to history for back button support (unless restoring from popstate)
    // Note: Explicitly pass location.href to preserve hash fragments
    if (!fromPopState) {
      history.pushState({facet: facet}, '', location.href);
    }

    // Dispatch custom event for apps to listen to facet changes
    const facetData = facet ? activeFacets.find(f => f.name === facet) : null;
    window.dispatchEvent(new CustomEvent('facet.switch', {
      detail: {
        facet: facet,
        facetData: facetData
      }
    }));
  }

  // Generic drag-and-drop setup for reordering items
  function setupDragDrop(config) {
    const {
      container,           // Container element
      itemSelector,        // '.facet-pill'
      dataAttribute,       // 'facetName'
      onReorder,          // Callback with new order array
      preventDefault      // Optional click prevention
    } = config;

    let draggedItem = null;
    let touchedItem = null;
    let touchDragActive = false;
    let isDragging = false;

    // Resolve the movable DOM node: if the item is inside a wrapper (e.g. <li>),
    // move the wrapper; otherwise move the item directly.
    function movable(item) {
      return item.parentElement === container ? item : item.parentElement;
    }

    // Helper: Get current order and trigger callback
    function triggerReorder() {
      const items = Array.from(container.querySelectorAll(itemSelector));
      const order = items.map(item => item.dataset[dataAttribute]);
      onReorder(order);
    }

    // Prevent text selection during drag (but allow drag to start)
    container.addEventListener('selectstart', (e) => {
      const target = e.target.closest(itemSelector);
      if (target) {
        e.preventDefault(); // Prevent text selection
      }
    });

    // Click prevention (if needed)
    if (preventDefault) {
      container.addEventListener('click', (e) => {
        if (isDragging) {
          e.preventDefault();
          e.stopPropagation();
          isDragging = false;
        }
      }, true);
    }

    // Mouse drag-and-drop
    container.addEventListener('dragstart', (e) => {
      const target = e.target.closest(itemSelector);
      if (!target) return;

      draggedItem = target;
      isDragging = true;

      target.classList.add('dragging');
      e.dataTransfer.effectAllowed = 'move';
      e.dataTransfer.setData('text/plain', '');

      // Create a better drag image
      const dragImage = target.cloneNode(true);
      dragImage.style.position = 'absolute';
      dragImage.style.top = '-9999px';
      dragImage.style.left = '-9999px';
      dragImage.style.opacity = '0.8';
      dragImage.style.transform = 'rotate(3deg)';
      dragImage.style.pointerEvents = 'none';
      document.body.appendChild(dragImage);

      const rect = target.getBoundingClientRect();
      e.dataTransfer.setDragImage(dragImage, rect.width / 2, rect.height / 2);

      // Remove the clone after drag image is captured
      setTimeout(() => dragImage.remove(), 0);
    });

    container.addEventListener('dragover', (e) => {
      e.preventDefault();
      let target = e.target.closest(itemSelector);
      if (!target || target === draggedItem) return;

      const items = Array.from(container.querySelectorAll(itemSelector));

      // Remove drag-over from all items
      container.querySelectorAll(itemSelector).forEach(item => item.classList.remove('drag-over'));
      target.classList.add('drag-over');

      // Live reordering: move the dragged item in DOM as we drag over targets
      const draggedIndex = items.indexOf(draggedItem);
      const targetIndex = items.indexOf(target);

      if (draggedIndex !== -1 && targetIndex !== -1 && draggedIndex !== targetIndex) {
        if (draggedIndex < targetIndex) {
          // Moving down/right: insert after target
          container.insertBefore(movable(draggedItem), movable(target).nextSibling);
        } else {
          // Moving up/left: insert before target
          container.insertBefore(movable(draggedItem), movable(target));
        }
      }
    });

    container.addEventListener('drop', (e) => {
      e.preventDefault();

      // DOM already reordered during dragover, just trigger callback
      triggerReorder();
    });

    container.addEventListener('dragend', (e) => {
      const target = e.target.closest(itemSelector);
      if (!target) return;

      target.classList.remove('dragging');
      container.querySelectorAll(itemSelector).forEach(item => item.classList.remove('drag-over'));

      draggedItem = null;

      // Reset isDragging after a short delay to allow click prevention
      setTimeout(() => { isDragging = false; }, 100);
    });

    // Touch drag-and-drop
    container.addEventListener('touchstart', (e) => {
      const target = e.target.closest(itemSelector);
      if (!target) return;

      touchedItem = target;
      touchDragActive = false;

      // Wait 200ms to distinguish tap from drag
      setTimeout(() => {
        if (touchedItem === target) {
          touchDragActive = true;
          isDragging = true;
          target.classList.add('dragging');
        }
      }, 200);
    }, { passive: true });

    container.addEventListener('touchmove', (e) => {
      if (!touchDragActive || !touchedItem) return;
      e.preventDefault();

      const touch = e.touches[0];
      const elementAtPoint = document.elementFromPoint(touch.clientX, touch.clientY);
      let target = elementAtPoint?.closest(itemSelector);

      if (target && target !== touchedItem) {
        const items = Array.from(container.querySelectorAll(itemSelector));

        // Remove drag-over from all items
        container.querySelectorAll(itemSelector).forEach(item => item.classList.remove('drag-over'));
        target.classList.add('drag-over');

        // Live reordering during touch drag
        const draggedIndex = items.indexOf(touchedItem);
        const targetIndex = items.indexOf(target);

        if (draggedIndex !== -1 && targetIndex !== -1 && draggedIndex !== targetIndex) {
          if (draggedIndex < targetIndex) {
            container.insertBefore(movable(touchedItem), movable(target).nextSibling);
          } else {
            container.insertBefore(movable(touchedItem), movable(target));
          }
        }
      }
    }, { passive: false });

    container.addEventListener('touchend', (e) => {
      if (!touchDragActive || !touchedItem) {
        touchedItem = null;
        touchDragActive = false;
        return;
      }

      const touch = e.changedTouches[0];
      const elementAtPoint = document.elementFromPoint(touch.clientX, touch.clientY);
      const target = elementAtPoint?.closest(itemSelector);

      // DOM already reordered during touchmove, just trigger callback
      triggerReorder();

      // Cleanup
      touchedItem.classList.remove('dragging');
      container.querySelectorAll(itemSelector).forEach(item => item.classList.remove('drag-over'));
      touchedItem = null;
      touchDragActive = false;

      // Reset isDragging after a short delay
      setTimeout(() => { isDragging = false; }, 100);
    }, { passive: true });
  }

  // Fade the facet strip's trailing edge while there is more of the row to scroll to.
  function updateFacetScrollAffordance() {
    const container = document.querySelector('.facet-bar .facet-pills-container');
    if (!container) return;
    const { scrollLeft, scrollWidth, clientWidth } = container;
    container.classList.toggle('scroll-fade-end', scrollLeft + clientWidth < scrollWidth - 1);
  }

  // Initialize
  function init() {
    // window.selectedFacet is initialized by the hosting shell before init runs.
    loadFacetChooser();

    // Setup facet pill drag-and-drop
    const facetPillsContainer = document.querySelector('.facet-pills-container');
    if (facetPillsContainer) {
      setupDragDrop({
        container: facetPillsContainer,
        itemSelector: '.facet-pill',
        dataAttribute: 'facetName',
        preventDefault: true,  // Prevent facet selection on drag
        onReorder: async (order) => {
          // Update local array to match new order
          activeFacets.sort((a, b) => {
            return order.indexOf(a.name) - order.indexOf(b.name);
          });

          // Re-render pills (maintains selection state)
          renderFacetChooser();

          // Save to backend
          try {
            const response = await fetch('/api/config/facets/order', {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify({ order })
            });

            if (!response.ok) throw new Error('Failed to save facet order');
          } catch (error) {
            console.error('Failed to save facet order:', error);
            if (window.AppServices?.notifications) {
              window.AppServices.notifications.show({
                app: 'system',
                title: 'Failed to save facet order',
                message: error.message,
                autoDismiss: 5000
              });
            }
          }
        }
      });

      // Keyboard navigation for facet pills (toolbar pattern)
      facetPillsContainer.addEventListener('keydown', (e) => {
        const pill = e.target.closest('.facet-pill');
        if (!pill) return;

        let nextIndex;
        const pills = Array.from(facetPillsContainer.querySelectorAll('.facet-pill'));
        const currentIndex = pills.indexOf(pill);

        if (e.key === 'ArrowRight') {
          nextIndex = (currentIndex + 1) % pills.length;
        } else if (e.key === 'ArrowLeft') {
          nextIndex = (currentIndex - 1 + pills.length) % pills.length;
        } else if (e.key === 'Home') {
          nextIndex = 0;
        } else if (e.key === 'End') {
          nextIndex = pills.length - 1;
        } else {
          return;
        }

        e.preventDefault();
        pill.tabIndex = -1;
        pills[nextIndex].tabIndex = 0;
        pills[nextIndex].focus();
      });
    }

    // Facet strip horizontal fade listeners
    const facetPillsScroll = document.querySelector('.facet-bar .facet-pills-container');
    if (facetPillsScroll) {
      facetPillsScroll.addEventListener('scroll', updateFacetScrollAffordance, { passive: true });
      window.addEventListener('resize', updateFacetScrollAffordance);
      updateFacetScrollAffordance();
    }

    // Initialize history state for back button support
    history.replaceState({facet: window.selectedFacet}, '');

    // Listen for browser back/forward navigation
    // Only change facet if state explicitly contains facet property
    // Hash-only navigation (state=null) should not affect facet selection
    window.addEventListener('popstate', (e) => {
      if (e.state && 'facet' in e.state) {
        selectFacet(e.state.facet, true);  // true = from popstate, don't push new state
      }
    });
  }

  // Expose selectFacet globally
  window.selectFacet = selectFacet;

  // ========== FACET CREATION MODAL ==========

  // Create modal element (once)
  function ensureFacetCreateModal() {
    if (document.getElementById('facetCreateModal')) return;

    const modal = document.createElement('div');
    modal.id = 'facetCreateModal';
    modal.className = 'facet-create-modal';
    modal.innerHTML = `
      <div class="facet-create-content">
        <h3>create new facet</h3>
        <div class="facet-create-field">
          <label for="facetCreateTitle">title</label>
          <input type="text" id="facetCreateTitle" placeholder="e.g. work projects" autofocus>
          <div class="facet-create-slug" id="facetCreateSlug"></div>
          <div class="facet-create-error" id="facetCreateError"></div>
        </div>
        <div class="facet-create-buttons">
          <button class="facet-create-cancel" id="facetCreateCancel">cancel</button>
          <button class="facet-create-submit" id="facetCreateSubmit" disabled>create</button>
        </div>
      </div>
    `;
    document.body.appendChild(modal);

    // Wire up events
    const titleInput = document.getElementById('facetCreateTitle');
    const slugDisplay = document.getElementById('facetCreateSlug');
    const submitBtn = document.getElementById('facetCreateSubmit');
    const cancelBtn = document.getElementById('facetCreateCancel');
    const errorDisplay = document.getElementById('facetCreateError');

    // Live slug generation as user types
    titleInput.addEventListener('input', () => {
      const title = titleInput.value.trim();
      const slug = titleToSlug(title);
      if (slug) {
        slugDisplay.textContent = slug;
        slugDisplay.classList.add('has-slug');
      } else {
        slugDisplay.textContent = '';
        slugDisplay.classList.remove('has-slug');
      }
      submitBtn.disabled = !slug;
      errorDisplay.classList.remove('visible');
    });

    // Enter to submit
    titleInput.addEventListener('keydown', (e) => {
      if (e.key === 'Enter' && !submitBtn.disabled) {
        e.preventDefault();
        submitFacetCreate();
      } else if (e.key === 'Escape') {
        closeFacetCreateModal();
      }
    });

    // Cancel button
    cancelBtn.addEventListener('click', closeFacetCreateModal);

    // Submit button
    submitBtn.addEventListener('click', submitFacetCreate);

    // Click outside to close
    modal.addEventListener('click', (e) => {
      if (e.target === modal) {
        closeFacetCreateModal();
      }
    });
  }

  // Convert title to slug (kebab-case)
  function titleToSlug(title) {
    if (!title) return '';
    return title
      .toLowerCase()
      .replace(/[^a-z0-9]+/g, '-')
      .replace(/^-+|-+$/g, '');
  }

  // Open the modal
  function openFacetCreateModal() {
    ensureFacetCreateModal();
    const modal = document.getElementById('facetCreateModal');
    const titleInput = document.getElementById('facetCreateTitle');
    const slugDisplay = document.getElementById('facetCreateSlug');
    const submitBtn = document.getElementById('facetCreateSubmit');
    const errorDisplay = document.getElementById('facetCreateError');

    // Reset form
    titleInput.value = '';
    slugDisplay.textContent = '';
    slugDisplay.classList.remove('has-slug');
    submitBtn.disabled = true;
    errorDisplay.classList.remove('visible');

    modal.classList.add('visible');
    titleInput.focus();
  }

  // Close the modal
  function closeFacetCreateModal() {
    const modal = document.getElementById('facetCreateModal');
    if (modal) {
      modal.classList.remove('visible');
    }
  }

  // Submit facet creation
  async function submitFacetCreate() {
    const titleInput = document.getElementById('facetCreateTitle');
    const submitBtn = document.getElementById('facetCreateSubmit');
    const errorDisplay = document.getElementById('facetCreateError');

    const title = titleInput.value.trim();
    if (!title) return;

    submitBtn.disabled = true;
    submitBtn.textContent = 'creating...';

    try {
      const response = await fetch('/app/settings/api/facet', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ title })
      });

      const data = await response.json();

      if (!response.ok) {
        throw new Error(data.error || 'Failed to create facet');
      }

      // Success - close modal, select new facet, navigate to settings
      closeFacetCreateModal();

      // Add new facet to local data
      const newFacet = {
        name: data.facet,
        title: data.config.title,
        color: data.config.color,
        emoji: data.config.emoji,
        muted: false,
        count: 0
      };
      activeFacets.push(newFacet);
      window.facetsData = activeFacets;

      // Re-render facet bar
      renderFacetChooser();

      // Select the new facet
      selectFacet(data.facet);

      // Navigate to settings app to customize
      window.location.href = '/app/settings';

    } catch (error) {
      errorDisplay.textContent = error.message;
      errorDisplay.classList.add('visible');
      submitBtn.disabled = false;
      submitBtn.textContent = 'create';
    }
  }

  // Run initialization after DOM and shell seed data are both ready.
  window.whenShellReady(init);
})();

function copyToClipboard(text) {
  if (navigator.clipboard && navigator.clipboard.writeText) {
    return navigator.clipboard.writeText(text);
  }
  const textarea = document.createElement('textarea');
  textarea.value = text;
  textarea.style.position = 'fixed';
  textarea.style.opacity = '0';
  document.body.appendChild(textarea);
  textarea.select();
  document.execCommand('copy');
  document.body.removeChild(textarea);
  return Promise.resolve();
}

window.convey = window.convey || {};
window.convey.copyToClipboard = copyToClipboard;

const REPORT_KEY_CAP = 100;
const reportContexts = new Map();
let reportKeyCounter = 0;

function reportingEnabled() {
  return !(window.CONVEY_SETTINGS && window.CONVEY_SETTINGS.reportingEnabled === false);
}

function captureReportContext({ heading, apiError, customDetail }) {
  const key = `rk-${reportKeyCounter}`;
  reportKeyCounter += 1;
  if (reportContexts.size >= REPORT_KEY_CAP) {
    reportContexts.delete(reportContexts.keys().next().value);
  }
  reportContexts.set(key, {
    heading,
    apiError: apiError || null,
    customDetail: customDetail || ''
  });
  return key;
}

/**
 * Shared loading / empty / error surface-state renderer.
 * Examples: SurfaceState.loading({ text: 'Loading…' }), SurfaceState.empty({ icon: window.ConveyIcons.svg('search'), heading: 'No results' }), SurfaceState.error({ heading: 'Request failed', retry: true }).
 * Load order: call only after DOMContentLoaded or from later event/callback code.
 */
window.SurfaceState = (() => {
  const HEADING_LEVELS = new Set(['h1', 'h2', 'h3', 'h4', 'h5', 'h6']);
  const ERROR_ICON = window.ConveyIcons.svg('triangle-alert');
  const STRIP_LAST_KNOWN = /\s*[—-]\s*showing last known state\.?\s*$/i;

  function escapeHtml(value) {
    return String(value ?? '')
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#39;');
  }

  function normalizeHeadingLevel(level) {
    return HEADING_LEVELS.has(level) ? level : 'h2';
  }

  function hasValue(value) {
    return value !== undefined && value !== null && value !== '';
  }

  function formatDetailTimestamp(timestamp) {
    if (!hasValue(timestamp)) {
      return '';
    }
    const date = new Date(timestamp);
    if (Number.isNaN(date.getTime())) {
      return '';
    }
    return date.toLocaleString();
  }

  function renderErrorActions({ retry, retryLabel, secondary, reportable, heading, apiError }) {
    const parts = [];
    if (retry) {
      parts.push(`<button type="button" class="surface-state-retry">${escapeHtml(retryLabel)}</button>`);
    }
    if (secondary && hasValue(secondary.label)) {
      if (hasValue(secondary.href)) {
        parts.push(`<a class="surface-state-secondary" href="${escapeHtml(secondary.href)}">${escapeHtml(secondary.label)}</a>`);
      } else {
        parts.push(`<button type="button" class="surface-state-secondary">${escapeHtml(secondary.label)}</button>`);
      }
    }
    if (reportable && reportingEnabled()) {
      const reportKey = captureReportContext({ heading, apiError, customDetail: '' });
      const label = window.CONVEY_COPY.REPORT_BUTTON_LABEL;
      parts.push(`<button type="button" class="surface-state-report" data-report-key="${escapeHtml(reportKey)}">${escapeHtml(label)}</button>`);
    }
    return parts.length ? `<div class="surface-state-action-row">${parts.join('')}</div>` : '';
  }

  function renderErrorDetail(detail, serverMessage) {
    if (!detail) {
      return '';
    }

    const lines = [];
    if (hasValue(detail.status) && hasValue(detail.statusText) && hasValue(detail.url)) {
      lines.push(`<div>HTTP ${escapeHtml(detail.status)} ${escapeHtml(detail.statusText)} · ${escapeHtml(detail.url)}</div>`);
    }

    const reason = hasValue(detail.rawDetail)
      ? detail.rawDetail
      : (hasValue(detail.serverMessage) ? detail.serverMessage : serverMessage);
    if (hasValue(reason)) {
      lines.push(`<div>Server reason: ${escapeHtml(reason)}</div>`);
    }

    const timestamp = formatDetailTimestamp(detail.timestamp);
    if (timestamp) {
      lines.push(`<div>time: ${escapeHtml(timestamp)}</div>`);
    }

    if (hasValue(detail.correlationId)) {
      const correlationId = String(detail.correlationId);
      lines.push(
        `<div>reference: <button type="button" class="surface-state-copy-reference" data-copy-value="${escapeHtml(correlationId)}">`
        + `${escapeHtml(correlationId)} <span class="surface-state-copy-affordance">(click to copy)</span>`
        + `</button></div>`
      );
    }

    if (hasValue(detail.reasonCode)) {
      lines.push(`<div>reason code: ${escapeHtml(detail.reasonCode)}</div>`);
    }

    if (!lines.length) {
      return '';
    }
    return `<details class="surface-state-detail"><summary>show details</summary>${lines.join('')}</details>`;
  }

  document.addEventListener('click', event => {
    const target = event.target instanceof Element ? event.target : null;
    const trigger = target ? target.closest('.surface-state-copy-reference') : null;
    if (!trigger) {
      return;
    }
    const value = trigger.getAttribute('data-copy-value') || '';
    if (!value) {
      return;
    }
    copyToClipboard(value).then(() => {
      const affordance = trigger.querySelector('.surface-state-copy-affordance');
      if (affordance) {
        affordance.textContent = '(copied)';
      }
    }).catch(error => {
      if (window.logError) {
        window.logError(error, { context: 'surface-state copy reference failed' });
      }
    });
  });

  document.addEventListener('click', event => {
    const target = event.target instanceof Element ? event.target : null;
    const trigger = target ? target.closest('.surface-state-report') : null;
    if (!trigger) {
      return;
    }
    const key = trigger.getAttribute('data-report-key') || '';
    const context = reportContexts.get(key) || {
      heading: window.CONVEY_COPY.REPORT_DEFAULT_SUBJECT,
      apiError: null,
      customDetail: ''
    };
    if (window.convey && typeof window.convey.reportError === 'function') {
      window.convey.reportError({
        source: 'auto',
        heading: context.heading,
        apiError: context.apiError,
        customDetail: context.customDetail
      });
    } else if (window.logError) {
      window.logError(new Error('report-error handler unavailable'), { context: 'surface-state report failed' });
    }
  });

  function render(kind, {
    icon = '',
    heading = '',
    desc = '',
    action = '',
    headingLevel = 'h2',
    role = ''
  } = {}) {
    const tag = normalizeHeadingLevel(headingLevel);
    const roleAttr = role ? ` role="${role}"` : '';

    return `<div class="surface-state surface-state--${kind}"${roleAttr}>`
      + `${icon ? `<div class="surface-state-icon" aria-hidden="true">${icon}</div>` : ''}`
      + `${heading ? `<${tag} class="surface-state-heading">${escapeHtml(heading)}</${tag}>` : ''}`
      + `${desc ? `<p class="surface-state-desc">${escapeHtml(desc)}</p>` : ''}`
      + `${action ? `<div class="surface-state-action">${action}</div>` : ''}`
      + `</div>`;
  }

  function stripLastKnownFromHeading(errorHtml) {
    const template = document.createElement('template');
    template.innerHTML = errorHtml;
    const headingEl = template.content.querySelector('.surface-state-heading');
    if (headingEl) {
      headingEl.textContent = headingEl.textContent.replace(STRIP_LAST_KNOWN, '');
    }
    return template.innerHTML;
  }

  return {
    loading({ text = '' } = {}) {
      return `<div class="surface-state surface-state--loading" role="status" aria-busy="true">`
        + `<div class="surface-state-spinner" aria-hidden="true"></div>`
        + `${text ? `<span class="surface-state-text" data-role="loading-status">${escapeHtml(text)}</span>` : ''}`
        + `</div>`;
    },

    empty(options = {}) {
      return render('empty', options);
    },

    error({
      heading = 'Couldn\'t load this section',
      desc = window.CONVEY_COPY?.RELOAD_HINT || 'reload to try again.',
      serverMessage = '',
      retry = false,
      retryLabel = 'Try again',
      secondary = null,
      detail = null,
      reportable = true,
      headingLevel = 'h2'
    } = {}) {
      const tag = normalizeHeadingLevel(headingLevel);
      return `<div class="surface-state surface-state--error" role="alert">`
        + `<div class="surface-state-icon" aria-hidden="true">${ERROR_ICON}</div>`
        + `<${tag} class="surface-state-heading">${escapeHtml(heading)}</${tag}>`
        + `<p class="surface-state-desc">${escapeHtml(desc)}</p>`
        + `${serverMessage ? `<p class="surface-state-server-message">${escapeHtml(serverMessage)}</p>` : ''}`
        + renderErrorActions({ retry, retryLabel, secondary, reportable, heading, apiError: detail })
        + renderErrorDetail(detail, serverMessage)
        + `</div>`;
    },

    /**
     * Replace an initial loading scaffold or append a singleton refresh error beside it.
     * Prevents the apps/entities anti-pattern where an `.error-message` is stuffed inside
     * the loading scaffold (`apps/entities/workspace.html:2671-2674`).
     * On first-paint, strips a trailing `— showing last known state` tail from the
     * rendered heading so callers can pass the same heading on first-paint and refresh
     * paths without leaking refresh-only language to first-paint owners.
     *
     * @param {string} containerId
     * @param {string} errorHtml
     * @returns {HTMLElement|null}
     */
    replaceLoading(containerId, errorHtml) {
      const container = document.getElementById(containerId);
      if (!container) {
        return null;
      }

      const isFirstPaint = container.querySelector('.surface-state--loading');
      if (isFirstPaint) {
        container.innerHTML = stripLastKnownFromHeading(errorHtml);
        return container;
      }

      const parent = container.parentElement;
      if (parent) {
        Array.from(parent.children).forEach(child => {
          if (child !== container && child.classList.contains('surface-state-refresh-error')) {
            child.remove();
          }
        });
      }

      if (container.nextElementSibling?.classList.contains('surface-state-refresh-error')) {
        container.nextElementSibling.remove();
      }

      const wrapper = document.createElement('div');
      wrapper.className = 'surface-state-refresh-error';
      wrapper.innerHTML = errorHtml;
      container.insertAdjacentElement('afterend', wrapper);
      return container;
    }
  };
})();

/**
 * App Services Framework
 * Global API for apps to register background services, update badges, and show notifications
 */
window.AppServices = {
  services: {},
  _tasks: {},

  /**
   * Register an app background service
   * @param {string} appName - Name of the app
   * @param {object} service - Service object with initialize() method
   */
  register(appName, service) {
    this.services[appName] = service;
    if (service.initialize) {
      try {
        service.initialize();
      } catch (err) {
        console.error(`[AppServices] Failed to initialize ${appName} service:`, err);
      }
    }
  },

  markBackgroundFailing(_appName, _error) {},

  registerTask(appName, taskName, {
    run,
    intervalMs,
    onSuccess,
    onError,
    failuresBeforeFailing = 3
  }) {
    if (typeof run !== 'function') {
      throw new Error('AppServices.registerTask requires a run() function');
    }

    if (!this._tasks[appName]) {
      this._tasks[appName] = {};
    }

    const health = {
      disabled: false,
      failing: false,
      lastError: '',
      lastRunAt: null,
      lastSuccessAt: null,
      consecutiveFailures: 0,
      intervalId: null
    };
    this._tasks[appName][taskName] = health;

    const apiJsonForTask = (url, opts) => window.apiJson(url, { ...(opts || {}), noAuthRedirect: true });

    const runTask = async () => {
      health.lastRunAt = Date.now();

      try {
        const result = await run({ apiJson: apiJsonForTask });
        health.disabled = false;
        health.lastError = '';
        health.lastSuccessAt = Date.now();
        health.consecutiveFailures = 0;
        if (health.failing) {
          health.failing = false;
        }
        if (typeof onSuccess === 'function') {
          onSuccess(result);
        }
        return result;
      } catch (error) {
        const message = error?.message || 'Request failed';
        health.lastError = message;

        if (error instanceof window.ApiError && error.status === 403) {
          health.disabled = true;
          health.failing = false;
          if (health.intervalId) {
            window.clearInterval(health.intervalId);
            health.intervalId = null;
          }
          if (typeof onError === 'function') {
            onError(error);
          }
          return undefined;
        }

        health.disabled = false;
        health.consecutiveFailures += 1;
        if (typeof onError === 'function') {
          onError(error);
        }

        if (health.consecutiveFailures >= failuresBeforeFailing && !health.failing) {
          health.failing = true;
          this.notifications.show({
            app: 'system',
            title: `${String(appName).toLowerCase()} background task`,
            message,
            dismissible: true,
            autoDismiss: false,
            buttons: [
              {
                label: 'Try now',
                onClick: () => runNow(),
                dismiss: false
              },
              {
                label: 'Disable',
                onClick: () => {
                  health.disabled = true;
                  if (health.intervalId) {
                    window.clearInterval(health.intervalId);
                    health.intervalId = null;
                  }
                }
              }
            ]
          });
        }

        throw error;
      }
    };

    const stop = () => {
      if (health.intervalId) {
        window.clearInterval(health.intervalId);
        health.intervalId = null;
      }
    };

    const runNow = () => runTask();
    const ignoreTaskRejection = () => {
      // runTask already updates task health and owner-visible failure state.
    };

    if (Number.isFinite(intervalMs) && intervalMs > 0) {
      health.intervalId = window.setInterval(() => {
        runTask().catch(ignoreTaskRejection);
      }, intervalMs);
    }

    runTask().catch(ignoreTaskRejection);

    return {
      stop,
      runNow,
      getHealth() {
        return { ...health };
      }
    };
  },

  getTaskHealth(appName) {
    return { ...(this._tasks[appName] || {}) };
  },

  /**
   * Notification system
   */
  notifications: {
    _stack: [],
    _history: JSON.parse(localStorage.getItem('solstone:notification_history') || '[]'),
    _nextId: 1,
    _container: null,
    _dismissTimers: {},
    _defaultIconName: 'mailbox',
    _iconSvgByName: Object.freeze({
      'trash-2': window.ConveyIcons.svg('trash-2'),
      'undo-2': window.ConveyIcons.svg('undo-2'),
      'timer': window.ConveyIcons.svg('timer'),
      'triangle-alert': window.ConveyIcons.svg('triangle-alert'),
      'mailbox': window.ConveyIcons.svg('mailbox'),
      'life-buoy': window.ConveyIcons.svg('life-buoy'),
      'bot': window.ConveyIcons.svg('bot'),
      'circle-x': window.ConveyIcons.svg('circle-x'),
      'circle-check': window.ConveyIcons.svg('circle-check'),
      'refresh-cw': window.ConveyIcons.svg('refresh-cw'),
      'check': window.ConveyIcons.svg('check'),
      'mic-vocal': window.ConveyIcons.svg('mic-vocal'),
      'eye': window.ConveyIcons.svg('eye')
    }),
    _legacyIconNameByGlyph: Object.freeze({
      '🗑️': 'trash-2',
      '↩️': 'undo-2',
      '⏱️': 'timer',
      '⚠️': 'triangle-alert',
      '📬': 'mailbox',
      '🛟': 'life-buoy',
      '🤖': 'bot',
      '❌': 'circle-x',
      '✅': 'circle-check',
      '🔄': 'refresh-cw',
      '✓': 'check',
      '🎙️': 'mic-vocal',
      '👁️': 'eye'
    }),

    _iconNameFor(value) {
      if (value === undefined || value === null || value === '') {
        return this._defaultIconName;
      }
      const text = String(value);
      if (Object.prototype.hasOwnProperty.call(this._iconSvgByName, text)) {
        return text;
      }
      return this._legacyIconNameByGlyph[text] || null;
    },

    _normalizeIconName(value) {
      const name = this._iconNameFor(value);
      if (name) {
        return name;
      }
      console.warn('[Notifications] unsupported icon value:', String(value));
      return this._defaultIconName;
    },

    _resolveIcon(value) {
      return this._iconSvgByName[this._iconNameFor(value) || this._defaultIconName];
    },

    /**
     * Show a persistent notification card
     * @param {object} options - {app, icon, title, message, action, facet, dismissible, badge, autoDismiss, buttons, key, work_key}
     * @returns {number} Notification ID
     */
    show(options) {
      const key = options.key ? String(options.key) : null;
      const workKey = options.work_key ? String(options.work_key) : null;
      const buttons = this._normalizeButtons(options.buttons);
      const hasIcon = Object.prototype.hasOwnProperty.call(options, 'icon');
      const normalizedIcon = hasIcon ? this._normalizeIconName(options.icon) : this._defaultIconName;

      if (key) {
        const existing = this._stack.find(n => n.key === key);
        if (existing) {
          if (!existing._workKeys) existing._workKeys = new Set();
          if (workKey) existing._workKeys.add(workKey);
          existing.count = existing._workKeys.size || 1;
          existing.lastSeen = Date.now();
          existing.app = options.app || existing.app;
          if (hasIcon) existing.icon = normalizedIcon;
          existing.title = options.title || existing.title;
          existing.message = options.message || '';
          existing.action = options.action || null;
          existing.facet = options.facet || null;
          existing.dismissible = options.dismissible !== false;
          existing.badge = options.badge || this._countBadge(existing.count);
          existing.autoDismiss = options.autoDismiss || null;
          existing.buttons = buttons;
          this._render();
          return existing.id;
        }
      }

      const timestamp = Date.now();
      const notif = {
        id: this._nextId++,
        app: options.app || 'system',
        icon: normalizedIcon,
        title: options.title || 'Notification',
        message: options.message || '',
        action: options.action || null,
        facet: options.facet || null,
        dismissible: options.dismissible !== false,
        badge: options.badge || null,
        timestamp,
        lastSeen: timestamp,
        autoDismiss: options.autoDismiss || null,
        buttons
      };

      if (key) {
        notif.key = key;
        notif._workKeys = new Set(workKey ? [workKey] : []);
        notif.count = notif._workKeys.size || 1;
        notif.badge = options.badge || this._countBadge(notif.count);
      }

      this._stack.push(notif);
      this._addToHistory(notif);
      this._render();

      // Browser notification if permitted
      if ('Notification' in window && Notification.permission === 'granted') {
        new Notification(notif.title, {
          body: notif.message,
          tag: `${notif.app}-${notif.id}`
        });
      }

      // Auto-dismiss timer
      if (notif.autoDismiss) {
        this._startDismissTimer(notif);
      }

      return notif.id;
    },

    /**
     * Dismiss a specific notification
     * @param {number} id - Notification ID
     */
    dismiss(id) {
      this._clearDismissTimer(id);
      this._stack = this._stack.filter(n => n.id !== id);
      this._render();
    },

    /**
     * Dismiss all notifications for an app
     * @param {string} appName - App name
     */
    dismissApp(appName) {
      this._stack.filter(n => n.app === appName).forEach(n => this._clearDismissTimer(n.id));
      this._stack = this._stack.filter(n => n.app !== appName);
      this._render();
    },

    /**
     * Dismiss all notifications
     */
    dismissAll() {
      Object.keys(this._dismissTimers).forEach(id => this._clearDismissTimer(id));
      this._stack = [];
      this._render();
    },

    _startDismissTimer(notif) {
      // Clear any existing timer for this notification
      if (this._dismissTimers[notif.id]) {
        clearTimeout(this._dismissTimers[notif.id]);
      }
      this._dismissTimers[notif.id] = setTimeout(() => {
        delete this._dismissTimers[notif.id];
        this.dismiss(notif.id);
      }, notif.autoDismiss);

      // Reset the progress bar animation
      const card = this._container && this._container.querySelector(`.notification-card[data-id="${notif.id}"]`);
      if (card) {
        const bar = card.querySelector('.notification-countdown');
        if (bar) {
          bar.style.animation = 'none';
          // Force reflow to restart animation
          bar.offsetHeight;
          bar.style.animation = '';
          bar.style.animationDuration = notif.autoDismiss + 'ms';
        }
      }
    },

    _pauseDismissTimer(id) {
      if (this._dismissTimers[id]) {
        clearTimeout(this._dismissTimers[id]);
        delete this._dismissTimers[id];
      }
      const card = this._container && this._container.querySelector(`.notification-card[data-id="${id}"]`);
      if (card) {
        const bar = card.querySelector('.notification-countdown');
        if (bar) {
          bar.style.animationPlayState = 'paused';
        }
      }
    },

    _clearDismissTimer(id) {
      if (this._dismissTimers[id]) {
        clearTimeout(this._dismissTimers[id]);
        delete this._dismissTimers[id];
      }
    },

    _normalizeButtons(buttons) {
      return Array.isArray(buttons)
        ? buttons
            .filter(button => button && button.label)
            .map(button => ({
              label: String(button.label),
              onClick: typeof button.onClick === 'function' ? button.onClick : null,
              dismiss: button.dismiss !== false
            }))
        : [];
    },

    _countBadge(count) {
      return count > 1 ? `${count} segments` : null;
    },

    /**
     * Get count of active notifications
     * @returns {number}
     */
    count() {
      // Keyed notifications are deduped in _stack, so length is active groups
      // plus non-keyed cards rather than raw repeated events.
      return this._stack.length;
    },

    /**
     * Update existing notification
     * @param {number} id - Notification ID
     * @param {object} options - Fields to update
     */
    update(id, options) {
      const notif = this._stack.find(n => n.id === id);
      if (!notif) return;

      Object.assign(notif, options);
      this._render();
    },

    /**
     * Get notification history (most recent first)
     * @returns {Array} Array of notification objects
     */
    getHistory() {
      return [...this._history].reverse();
    },

    /**
     * Add notification to history and persist
     * @private
     */
    _addToHistory(notif) {
      // Store minimal data for history (exclude runtime fields)
      const historyEntry = {
        app: notif.app,
        icon: notif.icon,
        title: notif.title,
        message: notif.message,
        action: notif.action,
        facet: notif.facet,
        timestamp: notif.timestamp
      };

      this._history.push(historyEntry);

      // Cap at 10 items
      if (this._history.length > 10) {
        this._history = this._history.slice(-10);
      }

      // Persist to localStorage
      try {
        localStorage.setItem('solstone:notification_history', JSON.stringify(this._history));
      } catch (e) {
        // localStorage may be full or disabled
        console.warn('[Notifications] Failed to persist history:', e);
      }
    },

    /**
     * Render notification cards
     * @private
     */
    _render() {
      if (!this._container) {
        this._container = document.getElementById('notification-center');
        if (!this._container) return;
      }

      // Limit to 5 most recent
      const visible = this._stack.slice(-5);
      const visibleIds = visible.map(n => n.id);

      // Get existing card IDs
      const existingCards = Array.from(this._container.querySelectorAll('.notification-card'));
      const existingIds = existingCards.map(card => parseInt(card.getAttribute('data-id')));

      // Remove cards that are no longer in visible stack
      existingCards.forEach(card => {
        const id = parseInt(card.getAttribute('data-id'));
        if (!visibleIds.includes(id) && !card.classList.contains('notification-card--dismissing')) {
          card.classList.add('notification-card--dismissing');
          const onEnd = () => card.remove();
          card.addEventListener('transitionend', onEnd, { once: true });
          setTimeout(onEnd, 250);
        }
      });

      // Add or update cards
      visible.forEach(n => {
        let card = this._container.querySelector(`.notification-card[data-id="${n.id}"]`);

        if (!card) {
          // New card - create and animate
          card = this._createCard(n);
          this._container.appendChild(card);
        } else {
          // Existing card - update content (no animation)
          this._updateCard(card, n);
        }
      });

      // Start timestamp updater if not already running
      if (visible.length > 0 && !this._updateInterval) {
        this._updateInterval = setInterval(() => this._updateTimestamps(), 60000);
      } else if (visible.length === 0 && this._updateInterval) {
        clearInterval(this._updateInterval);
        this._updateInterval = null;
      }
    },

    /**
     * Attach click handler to notification card
     * @private
     */
	    _attachClickHandler(card, n) {
	      if (!n.action) return;

	      card.onclick = (e) => {
	        // Ignore clicks on controls inside the card
	        if (e.target.closest('.notification-close, .notification-action')) {
	          return;
	        }

        // Prevent default for anchor tags
        if (card.tagName === 'A') {
          e.preventDefault();
        }

        // Select facet if specified (for facet-aware navigation)
        if (n.facet && window.selectFacet) {
          window.selectFacet(n.facet);
        }

        // Navigate to the path
        window.location.href = n.action;
	      };
	    },

	    _syncButtons(card, n) {
	      const footer = card.querySelector('.notification-footer');
	      if (!footer) return;

	      let actionsEl = footer.querySelector('.notification-actions');
	      if (!n.buttons || n.buttons.length === 0) {
	        if (actionsEl) actionsEl.remove();
	        return;
	      }

	      if (!actionsEl) {
	        actionsEl = document.createElement('div');
	        actionsEl.className = 'notification-actions';
	        footer.appendChild(actionsEl);
	      }

	      actionsEl.replaceChildren();
	      n.buttons.forEach((button, idx) => {
	        const buttonEl = document.createElement('button');
	        buttonEl.type = 'button';
	        buttonEl.className = 'notification-action';
	        buttonEl.dataset.btn = String(idx);
	        buttonEl.textContent = button.label;
	        actionsEl.appendChild(buttonEl);
	      });

	      actionsEl.querySelectorAll('.notification-action').forEach((buttonEl) => {
	        buttonEl.onclick = (event) => {
	          event.preventDefault();
	          event.stopPropagation();
	          const button = n.buttons[Number(buttonEl.dataset.btn)];
	          if (!button) return;
	          if (button.onClick) {
	            button.onClick(n);
	          }
	          if (button.dismiss !== false) {
	            this.dismiss(n.id);
	          }
	        };
	      });
	    },

	    /**
	     * Create a new notification card element
	     * @private
     */
    _createCard(n) {
      // Use anchor tag for semantic HTML when action exists
      const card = document.createElement(n.action ? 'a' : 'div');
      card.className = 'notification-card';
      card.setAttribute('data-id', n.id);
      card.setAttribute('data-app', n.app);

      if (n.action) {
        card.href = n.action;
        if (n.facet) {
          card.setAttribute('data-facet', n.facet);
        }
      }

      if (n.autoDismiss) {
        card.setAttribute('tabindex', '0');
      }

      const relativeTime = this._getRelativeTime(n.lastSeen || n.timestamp);
      card.innerHTML = `
        <div class="notification-header">
          <span class="notification-app-icon icon-slot" aria-hidden="true">${this._resolveIcon(n.icon)}</span>
          <span class="notification-app-name">${window.AppServices.escapeHtml(n.app)}</span>
          ${n.dismissible ? `<button class="notification-close" onclick="event.preventDefault(); event.stopPropagation(); window.AppServices.notifications.dismiss(${n.id});">×</button>` : ''}
        </div>
        <div class="notification-body">
          <div class="notification-title">${window.AppServices.escapeHtml(n.title)}</div>
          ${n.message ? `<div class="notification-message">${window.AppServices.escapeHtml(n.message)}</div>` : ''}
          ${n.badge ? `<span class="notification-badge">${window.AppServices.escapeHtml(n.badge)}</span>` : ''}
	        </div>
	        <div class="notification-footer">
	          <span class="notification-time">${relativeTime}</span>
	        </div>
	        ${n.autoDismiss ? `<div class="notification-countdown" style="animation-duration: ${n.autoDismiss}ms"></div>` : ''}
	      `;
	      this._syncButtons(card, n);

	      // Attach click handler
	      this._attachClickHandler(card, n);

      if (n.autoDismiss) {
        const self = this;
        card.addEventListener('mouseenter', () => self._pauseDismissTimer(n.id));
        card.addEventListener('focusin', () => self._pauseDismissTimer(n.id));
        card.addEventListener('mouseleave', () => {
          if (card.matches(':focus-within')) return;
          const notif = self._stack.find(s => s.id === n.id);
          if (notif) self._startDismissTimer(notif);
        });
        card.addEventListener('focusout', (e) => {
          if (!card.contains(e.relatedTarget) && !card.matches(':hover')) {
            const notif = self._stack.find(s => s.id === n.id);
            if (notif) self._startDismissTimer(notif);
          }
        });
      }

      return card;
    },

    /**
     * Update existing notification card content
     * @private
     */
    _updateCard(card, n) {
      const iconEl = card.querySelector('.notification-app-icon');
      if (iconEl) {
        iconEl.innerHTML = this._resolveIcon(n.icon);
      }

      // Update title
      const titleEl = card.querySelector('.notification-title');
      if (titleEl) {
        titleEl.textContent = n.title;
      }

      // Update message
      const messageEl = card.querySelector('.notification-message');
      if (n.message) {
        if (messageEl) {
          messageEl.textContent = n.message;
        } else {
          const bodyEl = card.querySelector('.notification-body');
          const newMessage = document.createElement('div');
          newMessage.className = 'notification-message';
          newMessage.textContent = n.message;
          bodyEl.insertBefore(newMessage, bodyEl.querySelector('.notification-badge'));
        }
      } else if (messageEl) {
        messageEl.remove();
      }

      // Update badge
      const badgeEl = card.querySelector('.notification-badge');
      if (n.badge) {
        if (badgeEl) {
          badgeEl.textContent = n.badge;
        } else {
          const bodyEl = card.querySelector('.notification-body');
          const newBadge = document.createElement('span');
          newBadge.className = 'notification-badge';
          newBadge.textContent = n.badge;
          bodyEl.appendChild(newBadge);
        }
      } else if (badgeEl) {
        badgeEl.remove();
      }

      // Update time
	      const timeEl = card.querySelector('.notification-time');
	      if (timeEl) {
	        timeEl.textContent = this._getRelativeTime(n.lastSeen || n.timestamp);
	      }
	      this._syncButtons(card, n);

	      // Update action and facet
	      if (n.action) {
        if (card.tagName === 'A') {
          card.href = n.action;
        }

        if (n.facet) {
          card.setAttribute('data-facet', n.facet);
        } else {
          card.removeAttribute('data-facet');
        }

        // Recreate click handler with new action/facet values
        this._attachClickHandler(card, n);
      } else {
        card.style.cursor = 'default';
        card.onclick = null;
      }
    },

    /**
     * Update timestamps on visible notifications
     * @private
     */
    _updateTimestamps() {
      const cards = this._container?.querySelectorAll('.notification-card');
      if (!cards) return;

      cards.forEach(card => {
        const id = parseInt(card.getAttribute('data-id'));
        const notif = this._stack.find(n => n.id === id);
        if (notif) {
          const timeEl = card.querySelector('.notification-time');
          if (timeEl) {
            timeEl.textContent = this._getRelativeTime(notif.timestamp);
          }
        }
      });
    },

    /**
     * Get relative time string
     * @private
     */
    _getRelativeTime(timestamp) {
      const seconds = Math.floor((Date.now() - timestamp) / 1000);
      if (seconds < 60) return 'now';
      const minutes = Math.floor(seconds / 60);
      if (minutes < 60) return `${minutes}m`;
      const hours = Math.floor(minutes / 60);
      if (hours < 24) return `${hours}h`;
      const days = Math.floor(hours / 24);
      return `${days}d`;
    }
  },

  quietNotifs: (() => {
    let stored;
    try { stored = JSON.parse(localStorage.getItem('solstone:quiet_notifs') || '[]'); }
    catch(e) { stored = []; }
    return {
      _notifs: stored,
      _unviewed: stored.length,
      _nextId: stored.length ? Math.max(...stored.map(n => n.id || 0)) + 1 : 1,

      add({ source, message, ts }) {
        const notif = { id: this._nextId++, source, message: message || '', ts: ts || Date.now() };
        this._notifs.push(notif);
        if (this._notifs.length > 20) this._notifs.shift();
        this._unviewed++;
        this._persist();
        this._updateBadge();
      },

      markViewed() {
        this._unviewed = 0;
        this._updateBadge();
      },

      getAll() {
        return [...this._notifs].reverse();
      },

      _persist() {
        try {
          localStorage.setItem('solstone:quiet_notifs', JSON.stringify(this._notifs));
        } catch(e) {}
      },

      _updateBadge() {
        const badge = document.getElementById('quiet-notif-badge');
        if (!badge) return;
        if (this._unviewed > 0) {
          badge.textContent = String(this._unviewed);
          badge.style.display = 'flex';
        } else {
          badge.style.display = 'none';
        }
      }
    };
  })(),

  /**
   * Request browser notification permission
   * @returns {Promise<string>} Permission state
   */
  async requestNotificationPermission() {
    if ('Notification' in window && Notification.permission === 'default') {
      return await Notification.requestPermission();
    }
    return Notification.permission;
  },

  /**
   * Escape a value for safe interpolation into HTML. DOM-based (routes
   * through textContent/innerHTML). Nullish-safe: null/undefined become ''.
   */
  escapeHtml(value) {
    const div = document.createElement('div');
    div.textContent = String(value ?? '');
    return div.innerHTML;
  },

  /**
   * Render user-supplied markdown into sanitized HTML. Calls marked + DOMPurify.
   * Throws if `marked` or `DOMPurify` isn't loaded (shell is broken; fail loudly).
   */
  renderMarkdown(raw) {
    return DOMPurify.sanitize(marked.parse(String(raw || ''), { breaks: true, gfm: true }));
  },

  /**
   * Badge system for apps and facet pills
   * Unified API with parallel app/facet namespaces
   */
  badges: {
    /**
     * App badge state for background producers.
     */
    app: {
      _data: {},  // {appName: count}

      /**
       * Set badge count for an app
       * @param {string} appName - Name of the app
       * @param {number} count - Badge count (0 or falsy to hide)
       */
      set(appName, count) {
        if (count && count > 0) {
          this._data[appName] = count;
        } else {
          delete this._data[appName];
        }
      },

      /**
       * Clear badge for an app
       * @param {string} appName - Name of the app
       */
      clear(appName) {
        delete this._data[appName];
      },

      /**
       * Get badge count for an app
       * @param {string} appName - Name of the app
       * @returns {number} Badge count (0 if not set)
       */
      get(appName) {
        return this._data[appName] || 0;
      },
    },

    /**
     * Facet pill badges in the facet bar
     */
    facet: {
      _data: {},  // {facetName: count}

      /**
       * Set badge count for a facet
       * @param {string} facetName - Name of the facet
       * @param {number} count - Badge count (0 or falsy to hide)
       */
      set(facetName, count) {
        if (count && count > 0) {
          this._data[facetName] = count;
        } else {
          delete this._data[facetName];
        }
        this._render(facetName);
      },

      /**
       * Clear badge for a facet
       * @param {string} facetName - Name of the facet
       */
      clear(facetName) {
        delete this._data[facetName];
        this._render(facetName);
      },

      /**
       * Get badge count for a facet
       * @param {string} facetName - Name of the facet
       * @returns {number} Badge count (0 if not set)
       */
      get(facetName) {
        return this._data[facetName] || 0;
      },

      /**
       * Render badge for a facet
       * @private
       */
      _render(facetName) {
        // Defer render if DOM not ready
        if (document.readyState === 'loading') {
          const self = this;
          document.addEventListener('DOMContentLoaded', function() {
            self._render(facetName);
          });
          return;
        }

        const facetPill = document.querySelector(`.facet-pill[data-facet-name="${facetName}"]`);
        if (!facetPill) return;

        let badge = facetPill.querySelector('.facet-badge');
        const count = this._data[facetName];

        if (!count || count <= 0) {
          // Hide or remove badge
          if (badge) {
            badge.style.display = 'none';
          }
          return;
        }

        // Create badge if needed
        if (!badge) {
          badge = document.createElement('span');
          badge.className = 'facet-badge';
          const emojiContainer = facetPill.querySelector('.emoji-container');
          if (emojiContainer) {
            emojiContainer.appendChild(badge);
          }
        }

        badge.textContent = count;
        badge.setAttribute('aria-label', count + ' pending');
        badge.style.display = '';
      }
    }
  }
};
