const input = document.querySelector('#opacity');
const output = document.querySelector('#opacity-output');
let pendingOpacity = null;
let isApplyingOpacity = false;

function setOpacityPercent(value) {
  const nextValue = Math.max(40, Math.min(100, Number(value) || 100));
  input.value = String(nextValue);
  output.textContent = `${nextValue}%`;
}

function queueOpacityUpdate() {
  const opacityPercent = Number(input.value);
  setOpacityPercent(opacityPercent);
  pendingOpacity = opacityPercent;

  if (!isApplyingOpacity) {
    void flushOpacityUpdates();
  }
}

async function flushOpacityUpdates() {
  isApplyingOpacity = true;

  try {
    while (pendingOpacity !== null) {
      const opacityPercent = pendingOpacity;
      pendingOpacity = null;
      await window.__TAURI_INTERNALS__.invoke('browser_set_window_opacity', {
        request: {
          opacity: opacityPercent / 100,
        },
      });
    }
  } catch {
    // A newer queued value, if present, is still applied below.
  } finally {
    isApplyingOpacity = false;

    if (pendingOpacity !== null) {
      void flushOpacityUpdates();
    }
  }
}

window.setOpacityPercent = setOpacityPercent;
input.addEventListener('input', () => {
  queueOpacityUpdate();
});
