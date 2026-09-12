// WinSpaces Website Script

document.addEventListener('DOMContentLoaded', () => {
  initTheme();
  initCopyButtons();
  initLightbox();
});

/* Theme Handling */
function initTheme() {
  const toggleBtn = document.getElementById('theme-toggle');
  const prefersDark = window.matchMedia('(prefers-color-scheme: dark)');

  // The inline script in <head> already set data-theme; only sync the button here.
  function applyTheme(theme, persist) {
    document.documentElement.setAttribute('data-theme', theme);
    if (persist) localStorage.setItem('winspaces-theme', theme);
    if (toggleBtn) {
      toggleBtn.innerHTML = theme === 'dark'
        ? '<span>☀️</span> Light'
        : '<span>🌙</span> Dark';
    }
  }

  applyTheme(document.documentElement.getAttribute('data-theme') || 'dark', false);

  if (toggleBtn) {
    toggleBtn.addEventListener('click', () => {
      const current = document.documentElement.getAttribute('data-theme') || 'dark';
      applyTheme(current === 'dark' ? 'light' : 'dark', true);
    });
  }

  prefersDark.addEventListener('change', (e) => {
    if (!localStorage.getItem('winspaces-theme')) {
      applyTheme(e.matches ? 'dark' : 'light', false);
    }
  });
}

/* Copy Buttons */
function initCopyButtons() {
  const copyBtns = document.querySelectorAll('.copy-btn');

  copyBtns.forEach(btn => {
    btn.addEventListener('click', () => {
      const textToCopy = btn.getAttribute('data-copy');
      if (!textToCopy) return;

      navigator.clipboard.writeText(textToCopy).then(() => {
        const originalText = btn.textContent;
        btn.textContent = 'Copied!';
        setTimeout(() => {
          btn.textContent = originalText;
        }, 1500);
      });
    });
  });
}

/* Lightbox: click a .zoomable image to view it full size in a <dialog> */
function initLightbox() {
  const dialog = document.getElementById('lightbox');
  if (!dialog || !dialog.showModal) return;
  const big = dialog.querySelector('img');
  const caption = dialog.querySelector('figcaption');
  const open = (img) => {
    big.src = img.dataset.full || img.currentSrc || img.src;
    const fig = img.closest('figure');
    const cap = fig && fig.querySelector('figcaption');
    caption.textContent = cap ? cap.textContent : '';
    dialog.showModal();
  };
  document.querySelectorAll('img.zoomable').forEach(img => {
    img.addEventListener('click', () => open(img));
    img.addEventListener('keydown', (e) => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); open(img); } });
  });
  // Close on the backdrop or the button, not on the image itself.
  dialog.addEventListener('click', (e) => { if (e.target !== big) dialog.close(); });
}
