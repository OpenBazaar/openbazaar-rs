// Apply the saved appearance before the stylesheet and app paint their first frame.
(() => {
  let theme;
  try { theme = localStorage.getItem('ob-theme'); } catch {}
  if (theme !== 'light' && theme !== 'dark') {
    theme = window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
  }
  document.documentElement.dataset.obTheme = theme;
})();
