const input = document.querySelector('#opacity');
const output = document.querySelector('#opacity-output');

function setOpacityPercent(value) {
  const nextValue = Math.max(40, Math.min(100, Number(value) || 100));
  input.value = String(nextValue);
  output.textContent = `${nextValue}%`;
}

async function applyOpacity() {
  const opacityPercent = Number(input.value);
  setOpacityPercent(opacityPercent);

  try {
    await window.__TAURI_INTERNALS__.invoke('browser_set_window_opacity', {
      request: {
        opacity: opacityPercent / 100,
      },
    });
  } catch {}
}

window.setOpacityPercent = setOpacityPercent;
input.addEventListener('input', () => {
  void applyOpacity();
});
