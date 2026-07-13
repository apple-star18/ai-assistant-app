const fieldsElement = document.querySelector('#fields');
const formElement = document.querySelector('#settings-form');
const messageElement = document.querySelector('#message');

const shortcutFields = [
  {
    action: 'shortcutMode1',
    label: 'Mode 1',
    defaultAccelerator: 'Ctrl+Alt+1',
  },
  {
    action: 'shortcutMode2',
    label: 'Mode 2',
    defaultAccelerator: 'Ctrl+Alt+2',
  },
  {
    action: 'shortcutMode3',
    label: 'Mode 3',
    defaultAccelerator: 'Ctrl+Alt+3',
  },
];

let hotkeyState = {
  bindings: [],
  lastError: null,
};

function invoke(command, payload) {
  return window.__TAURI_INTERNALS__.invoke(command, payload);
}

function bindingFor(action) {
  return hotkeyState.bindings.find((binding) => binding.action === action);
}

function acceleratorFor(field) {
  return bindingFor(field.action)?.accelerator || field.defaultAccelerator;
}

function statusFor(field) {
  const binding = bindingFor(field.action);

  if (!binding) {
    return { text: 'Default', isError: false };
  }

  if (binding.error) {
    return { text: binding.error, isError: true };
  }

  return { text: binding.registered ? 'Registered' : 'Not registered', isError: false };
}

function renderFields() {
  fieldsElement.replaceChildren(
    ...shortcutFields.map((field) => {
      const status = statusFor(field);
      const label = document.createElement('label');
      label.className = 'field';

      const labelText = document.createElement('span');
      labelText.className = 'field-label';
      labelText.textContent = field.label;

      const input = document.createElement('input');
      input.name = field.action;
      input.value = acceleratorFor(field);
      input.spellcheck = false;
      input.autocapitalize = 'none';

      const statusText = document.createElement('span');
      statusText.className = status.isError ? 'status error' : 'status';
      statusText.textContent = status.text;

      label.append(labelText, input, statusText);
      return label;
    }),
  );
}

function setMessage(text, isError = false) {
  messageElement.textContent = text;
  messageElement.className = isError ? 'message error' : 'message';
}

async function refreshHotkeys() {
  try {
    hotkeyState = await invoke('hotkeys_get_state');
    renderFields();
    setMessage(hotkeyState.lastError || '');
  } catch (error) {
    setMessage(error?.message || 'Failed to load settings.', true);
  }
}

async function closeSettings() {
  await invoke('browser_set_settings_overlay', {
    request: {
      isOpen: false,
      left: 0,
      top: 0,
      width: 1,
      height: 1,
      indicatorLeft: 14,
    },
  });
}

async function applySettings(event) {
  event.preventDefault();
  setMessage('');

  const formData = new FormData(formElement);
  const bindings = shortcutFields.map((field) => ({
    action: field.action,
    accelerator: String(formData.get(field.action) || '').trim(),
  }));

  try {
    hotkeyState = await invoke('hotkeys_apply_settings', {
      request: {
        bindings,
      },
    });
    renderFields();

    if (hotkeyState.lastError) {
      setMessage(hotkeyState.lastError, true);
      return;
    }

    await closeSettings();
  } catch (error) {
    setMessage(error?.message || 'Failed to apply settings.', true);
  }
}

formElement.addEventListener('submit', (event) => {
  void applySettings(event);
});

renderFields();
window.setSettingsIndicatorLeft = (indicatorLeft) => {
  const nextLeft = Math.max(14, Math.min(document.body.clientWidth - 14, Number(indicatorLeft)));
  formElement.style.setProperty('--indicator-left', `${nextLeft}px`);
};
window.refreshSettings = refreshHotkeys;
void refreshHotkeys();
