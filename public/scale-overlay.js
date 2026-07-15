const input = document.querySelector('#browser-scale');
const output = document.querySelector('#browser-scale-output');
let pendingScale = null;
let isApplyingScale = false;

function setBrowserScalePercent(value) {
  const nextValue = Math.max(50, Math.min(200, Number(value) || 100));
  input.value = String(nextValue);
  output.textContent = `${nextValue}%`;
}

function queueScaleUpdate() {
  const scalePercent = Number(input.value);
  setBrowserScalePercent(scalePercent);
  pendingScale = scalePercent;

  if (!isApplyingScale) {
    void flushScaleUpdates();
  }
}

async function flushScaleUpdates() {
  isApplyingScale = true;

  try {
    while (pendingScale !== null) {
      const scalePercent = pendingScale;
      pendingScale = null;
      await window.__TAURI_INTERNALS__.invoke('browser_set_scale', {
        request: { scale: scalePercent / 100 },
      });
    }
  } catch {
    // A newer queued value, if present, is still applied below.
  } finally {
    isApplyingScale = false;

    if (pendingScale !== null) {
      void flushScaleUpdates();
    }
  }
}

window.setBrowserScalePercent = setBrowserScalePercent;
input.addEventListener('input', () => {
  queueScaleUpdate();
});
